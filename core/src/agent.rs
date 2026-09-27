/**
 * Comrade Core agent loop: plan → tool-call → verify → report.
 * Guardrails: max_steps, timeout, cancellation, bounded error recovery.
 */
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::environment::{detect_environment, environment_prompt};
use crate::intent::{classify_intent, Intent};
use crate::llm::{ChatMessage, ChatOptions, Embedder, LlmProvider, ToolDefinition};
use crate::logger::{log, Level};
use crate::memory::{MemoryStore, ScoredMemory};
use crate::permissions::{describe_tool_call, tool_risk};
use crate::task_state::{add_step, create_task, finish_step, StepStatus, TaskState, TaskStatus};
use crate::tools::types::{Risk, Tool, ToolContext};

const SYSTEM_PROMPT: &str = "You are Comrade, a local-first desktop AI agent. Your job: understand what the user wants, choose tools, execute, VERIFY the result, and report clearly.

Rules:
- Use tools for actions. Never claim you did something you did not call a tool for.
- CODING tasks (modify/create/debug source code) MUST go through opencode.executeTask — never edit code with filesystem.write directly.
- BROWSER_* tools read/navigate sites only; they cannot change code.
- After each action, verify: re-read, re-list, check output, then report what actually happened.
- Keep responses short and factual. No emojis.
- Never print secrets, keys, tokens, or cookies.
- If a tool fails, retry at most once with a fix, then report the error honestly.";

#[allow(async_fn_in_trait)]
pub trait AgentCallbacks: Send {
    fn on_ui_state(&self, state: &str);
    fn on_step(&self, label: &str, status: &str, detail: Option<&str>);
    fn on_token(&self, token: &str);
    fn is_cancelled(&self) -> bool;
    async fn request_approval(&self, summary: &str) -> bool;
}

pub struct AgentDeps<P, E> {
    pub llm: Arc<P>,
    pub embedder: Arc<E>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub memory: Arc<std::sync::Mutex<MemoryStore>>,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub max_steps: usize,
    pub timeout_ms: u64,
}

pub struct Agent<P, E> {
    deps: AgentDeps<P, E>,
}

fn format_memories(hits: &[ScoredMemory]) -> String {
    if hits.is_empty() {
        return "(none)".to_string();
    }
    hits.iter()
        .map(|m| {
            let text: String = m.text.chars().take(300).collect();
            format!("[{}:{}] {text}", m.kind, m.source)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse extractor output: one memory per "- " line; "NONE" means nothing durable.
pub fn parse_memories(text: &str) -> Vec<String> {
    if text.trim().eq_ignore_ascii_case("none") {
        return vec![];
    }
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")))
        .map(|s| s.trim().chars().take(300).collect::<String>())
        .filter(|s| !s.is_empty())
        .take(5)
        .collect()
}

/// Ask the brain what (if anything) from this turn is worth remembering across
/// sessions. Greetings and task mechanics yield nothing — only durable user
/// facts, preferences, projects, and constraints.
async fn extract_memories<P: LlmProvider>(
    llm: &P,
    user_request: &str,
    result: &str,
) -> Vec<String> {
    let req: String = user_request.chars().take(500).collect();
    let res: String = result.chars().take(1500).collect();
    let opts = ChatOptions {
        model: None,
        max_tokens: 256,
        temperature: 0.0,
        tools: vec![],
    };
    let messages = vec![
        ChatMessage::system(
            "You extract long-term memories from a conversation. List durable facts \
             ABOUT THE USER worth remembering across sessions (identity, preferences, \
             projects and their paths, constraints, goals) — one per line starting \
             with '- '. Ignore greetings, chit-chat, pleasantries, and task mechanics. \
             If nothing is durably worth remembering, reply with exactly: NONE",
        ),
        ChatMessage::user(format!("User: {req}\nAssistant: {res}")),
    ];
    match llm.chat(&messages, &opts).await {
        Ok(r) => parse_memories(&r.content),
        Err(_) => vec![],
    }
}

impl<P: LlmProvider, E: Embedder> Agent<P, E> {
    pub fn new(deps: AgentDeps<P, E>) -> Self {
        Self { deps }
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.deps
            .tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect()
    }

    pub async fn run_task<C: AgentCallbacks>(&self, user_request: &str, cb: &C) -> TaskState {
        let deadline = Instant::now() + Duration::from_millis(self.deps.timeout_ms);
        cb.on_ui_state("thinking");

        let (intent, via) = classify_intent(self.deps.llm.as_ref(), user_request).await;
        log(
            Level::Agent,
            "intent",
            &format!("{} (via {via})", intent.as_str()),
            Some(&serde_json::json!({ "request": user_request.chars().take(200).collect::<String>() })),
        );
        let _ = Intent::Chat; // keep enum used in signature context

        let mut task = create_task(user_request, intent.as_str());
        task.status = TaskStatus::Running;

        let env = detect_environment(&self.deps.cwd).await;
        let work_cwd = env
            .git_repo
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.deps.cwd.clone());
        let projects = {
            let mem = self.deps.memory.lock().unwrap();
            mem.project_lines().join("\n")
        };
        // Vector recall: relevant long-term memories for this request.
        let remembered = match self.deps.embedder.embed(&[user_request.to_string()]).await {
            Ok(vecs) => {
                let mem = self.deps.memory.lock().unwrap();
                mem.recall(&vecs[0], 5, 0.3)
            }
            Err(e) => {
                let msg: String = format!("{e}").chars().take(200).collect();
                log(Level::Warn, "MEMORY", "recall embedding failed; continuing without memories",
                    Some(&serde_json::json!({ "error": msg })));
                vec![]
            }
        };

        let mut messages = vec![
            ChatMessage::system(SYSTEM_PROMPT),
            ChatMessage::system(format!(
                "Environment:\n{}\nKnown projects:\n{}\nRelevant memories:\n{}\nIntent: {}",
                environment_prompt(&env),
                if projects.is_empty() { "(none)".to_string() } else { projects },
                format_memories(&remembered),
                intent.as_str()
            )),
            ChatMessage::user(user_request),
        ];

        let tool_ctx = ToolContext { cwd: work_cwd };
        let opts = ChatOptions {
            model: self.deps.model.clone(),
            tools: self.tool_definitions(),
            ..ChatOptions::default()
        };

        let mut steps = 0usize;
        loop {
            if cb.is_cancelled() {
                task.status = TaskStatus::Cancelled;
                task.result = Some("Task cancelled by user.".into());
                return task;
            }
            if Instant::now() > deadline {
                task.status = TaskStatus::Failed;
                task.error = Some("Task timed out.".into());
                cb.on_ui_state("error");
                return task;
            }
            if steps >= self.deps.max_steps {
                task.status = TaskStatus::Failed;
                task.error = Some(format!("Stopped after {} steps without completing.", self.deps.max_steps));
                cb.on_ui_state("error");
                return task;
            }
            steps += 1;
            cb.on_ui_state("thinking");

            let response = match self.deps.llm.chat(&messages, &opts).await {
                Ok(r) => r,
                Err(e) => {
                    task.status = TaskStatus::Failed;
                    let msg: String = format!("{e}").chars().take(500).collect();
                    task.error = Some(msg);
                    cb.on_ui_state("error");
                    return task;
                }
            };

            if response.tool_calls.is_empty() {
                task.status = TaskStatus::Done;
                let result = if response.content.is_empty() { "(empty response)".to_string() } else { response.content };
                cb.on_ui_state("speaking");
                cb.on_token(&result);
                task.result = Some(result.clone());
                // Learn durable facts only — never store raw chat. history.db
                // keeps the transcript; vector memory keeps distilled knowledge.
                let learned = extract_memories(self.deps.llm.as_ref(), user_request, &result).await;
                if !learned.is_empty() {
                    match self.deps.embedder.embed(&learned).await {
                        Ok(vecs) => {
                            let mem = self.deps.memory.lock().unwrap();
                            for (text, vec) in learned.iter().zip(vecs.iter()) {
                                if let Err(e) = mem.add(text, "learned", intent.as_str(), vec) {
                                    let msg: String = format!("{e}").chars().take(200).collect();
                                    log(Level::Warn, "MEMORY", "learned memory store failed",
                                        Some(&serde_json::json!({ "error": msg })));
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            let msg: String = format!("{e}").chars().take(200).collect();
                            log(Level::Warn, "MEMORY", "learned memory embed failed",
                                Some(&serde_json::json!({ "error": msg })));
                        }
                    }
                }
                return task;
            }

            cb.on_ui_state("executing");
            messages.push(ChatMessage {
                role: crate::llm::ChatRole::Assistant,
                content: response.content,
                tool_call_id: None,
                tool_calls: response.tool_calls.clone(),
            });

            for call in &response.tool_calls {
                if cb.is_cancelled() {
                    task.status = TaskStatus::Cancelled;
                    task.result = Some("Task cancelled by user.".into());
                    return task;
                }
                let Some(tool) = self.deps.tools.iter().find(|t| t.name() == call.name) else {
                    messages.push(ChatMessage {
                        role: crate::llm::ChatRole::Tool,
                        content: serde_json::json!({
                            "success": false,
                            "error": { "code": "UNKNOWN_TOOL", "message": format!("No such tool: {}", call.name) },
                        })
                        .to_string(),
                        tool_call_id: Some(call.id.clone()),
                        tool_calls: vec![],
                    });
                    continue;
                };
                let summary = describe_tool_call(&call.name, &call.arguments);
                log(Level::Tool, "call", &summary, None);
                add_step(&mut task, &summary);
                let step_index = task.steps.len() - 1;
                cb.on_step(&summary, "active", None);
                task.tool_calls.push(crate::task_state::ToolCallRecord {
                    tool: call.name.clone(),
                    args: call.arguments.clone(),
                    result_summary: String::new(),
                });

                if tool_risk(&call.name, &call.arguments) == Risk::Dangerous {
                    task.status = TaskStatus::AwaitingApproval;
                    let approved = cb.request_approval(&summary).await;
                    task.status = TaskStatus::Running;
                    if !approved {
                        let denial = serde_json::json!({
                            "success": false,
                            "error": { "code": "DENIED_BY_USER", "message": "User denied this action." },
                        })
                        .to_string();
                        messages.push(ChatMessage {
                            role: crate::llm::ChatRole::Tool,
                            content: denial,
                            tool_call_id: Some(call.id.clone()),
                            tool_calls: vec![],
                        });
                        finish_step(&mut task, step_index, StepStatus::Failed, Some("denied by user".into()));
                        cb.on_step(&summary, "failed", Some("denied by user"));
                        continue;
                    }
                }

                let result = tool.execute(&call.arguments, &tool_ctx).await;
                let text = result.to_json().to_string();
                let summary_text: String = text.chars().take(300).collect();
                messages.push(ChatMessage {
                    role: crate::llm::ChatRole::Tool,
                    content: text,
                    tool_call_id: Some(call.id.clone()),
                    tool_calls: vec![],
                });
                if let Some(last) = task.tool_calls.last_mut() {
                    last.result_summary = summary_text;
                }
                match &result {
                    crate::tools::types::ToolResult::Ok(_) => {
                        finish_step(&mut task, step_index, StepStatus::Done, None);
                        cb.on_step(&summary, "done", None);
                    }
                    crate::tools::types::ToolResult::Err(e) => {
                        finish_step(&mut task, step_index, StepStatus::Failed, Some(e.message.clone()));
                        cb.on_step(&summary, "failed", Some(&e.message));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_memories;

    #[test]
    fn extractor_output_parses() {
        assert!(parse_memories("NONE").is_empty());
        assert!(parse_memories("none\n").is_empty());
        let got = parse_memories("Some chatter\n- User prefers dark mode\n- Co-Founder repo at /home/u/co\n* Likes Arch Linux");
        assert_eq!(got.len(), 3);
        assert!(got[0].contains("dark mode"));
        // caps at 5
        let many = (0..10).map(|i| format!("- fact {i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(parse_memories(&many).len(), 5);
    }
}
