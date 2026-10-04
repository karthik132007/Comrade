//! Regression for a proposed coding plan followed by a separate "yes" turn.
use super::*;
use crate::history::HistoryStore;
use crate::llm::{ChatRole, LlmResponse, ToolCallRequest};
use crate::tools::types::ToolResult;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const GOAL: &str = "Plan and delegate this coding task through coding.startPlan. Goal: Add a paper and ink theme with yellowish paper, white and dark ink.";
const PROPOSAL: &str = "Proposed plan: implement the paper and ink theme in this project, then verify it. Shall I proceed?";

struct ContextBrain {
    project: PathBuf,
    goal: String,
    seen: Mutex<Vec<Vec<ChatMessage>>>,
}
impl LlmProvider for ContextBrain {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        opts: &ChatOptions,
    ) -> anyhow::Result<LlmResponse> {
        if opts.max_tokens == 16 {
            if messages.last().unwrap().content == "yes" {
                assert!(messages
                    .iter()
                    .any(|m| m.role == ChatRole::User && m.content == self.goal));
                assert!(messages
                    .iter()
                    .any(|m| m.role == ChatRole::Assistant && m.content == PROPOSAL));
            }
            return Ok(LlmResponse {
                content: "CODING".into(),
                ..Default::default()
            });
        }
        Ok(LlmResponse {
            content: "NONE".into(),
            ..Default::default()
        })
    }
    async fn stream(
        &self,
        messages: &[ChatMessage],
        _: &ChatOptions,
        _: &mut (dyn FnMut(String) + Send),
    ) -> anyhow::Result<LlmResponse> {
        self.seen.lock().unwrap().push(messages.to_vec());
        if messages.last().unwrap().role == ChatRole::Tool {
            return Ok(LlmResponse {
                content: "The launch result is recorded. See Comrade Orch for status.".into(),
                ..Default::default()
            });
        }
        let latest = messages.last().unwrap();
        if latest.content != "yes" {
            return Ok(LlmResponse {
                content: PROPOSAL.into(),
                ..Default::default()
            });
        }
        assert_eq!(latest.role, ChatRole::User);
        let turns: Vec<_> = messages
            .iter()
            .filter(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant))
            .collect();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].role, ChatRole::User);
        assert_eq!(turns[0].content, self.goal);
        assert_eq!(turns[1].role, ChatRole::Assistant);
        assert_eq!(turns[1].content, PROPOSAL);
        assert!(messages.iter().any(|m| m.role == ChatRole::System
            && m.content.contains("Local coding agents")
            && m.content.contains("available")));
        Ok(LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallRequest {
                id: "start".into(),
                name: "coding.startPlan".into(),
                arguments: serde_json::json!({
                    "title":"Paper and ink", "working_dir":self.project, "tasks":[{"id":"theme","agent":"codex","task":"Add yellowish paper, white and dark ink theme; run the UI checks."}], "max_parallel":1, "timeout_secs":600
                }),
            }],
        })
    }
}
struct NoEmbeddings;
impl Embedder for NoEmbeddings {
    async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(vec![vec![1.0]])
    }
}
struct PlanTool {
    calls: Arc<Mutex<Vec<serde_json::Value>>>,
}
impl Tool for PlanTool {
    fn name(&self) -> &'static str {
        "coding.startPlan"
    }
    fn description(&self) -> &'static str {
        "Test coding launch"
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::orchestration::OrchestrationTool("coding.startPlan").parameters()
    }
    fn risk(&self, _: &serde_json::Value) -> Risk {
        Risk::Dangerous
    }
    fn execute<'a>(
        &'a self,
        args: &'a serde_json::Value,
        _: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(args.clone());
            ToolResult::ok(serde_json::json!({"run_id":"run-theme","status":"queued"}))
        })
    }
}
struct Approval {
    allow: bool,
    requested: AtomicUsize,
}
impl AgentCallbacks for Approval {
    fn on_ui_state(&self, _: &str) {}
    fn on_step(&self, _: &str, _: &str, _: Option<&str>) {}
    fn on_token(&self, _: &str) {}
    fn is_cancelled(&self) -> bool {
        false
    }
    async fn request_approval(&self, summary: &str) -> bool {
        assert!(summary.contains("Paper and ink"));
        self.requested.fetch_add(1, Ordering::SeqCst);
        self.allow
    }
}
fn agent(
    brain: Arc<ContextBrain>,
    calls: Arc<Mutex<Vec<serde_json::Value>>>,
) -> Agent<ContextBrain, NoEmbeddings> {
    Agent::new(AgentDeps {
        cwd: brain.project.clone(),
        llm: brain,
        embedder: Arc::new(NoEmbeddings),
        tools: vec![Arc::new(PlanTool { calls })],
        memory: Arc::new(Mutex::new(MemoryStore::open_in_memory(1).unwrap())),
        model: None,
        max_steps: 4,
        timeout_ms: 10000,
    })
}
fn fixture() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "comrade-chat-context-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn plan_then_yes_restores_the_session_after_restart_and_calls_launch_once() {
    let root = fixture();
    let file = root.join("history.db");
    let history = HistoryStore::open(&file).unwrap();
    // Legacy conversations also work without project metadata.
    let sid = history
        .session_for_request(None, None, "Paper theme")
        .unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let goal = format!("Project: {}. {GOAL}", root.display());
    let brain = Arc::new(ContextBrain {
        goal: goal.clone(),
        project: root.clone(),
        seen: Mutex::new(vec![]),
    });
    let cb = Approval {
        allow: true,
        requested: AtomicUsize::new(0),
    };
    let proposed = agent(brain.clone(), calls.clone())
        .run_task(&goal, &cb)
        .await;
    assert_eq!(proposed.status, TaskStatus::Done);
    history
        .add_message(
            &sid,
            "user",
            &format!("Project: {}. {GOAL}", root.display()),
        )
        .unwrap();
    history
        .add_message(&sid, "comrade", proposed.result.as_deref().unwrap())
        .unwrap();
    drop(history);
    let reopened = HistoryStore::open(&file).unwrap();
    let resumed = reopened
        .session_for_request(Some(&sid), None, "yes")
        .unwrap();
    assert_eq!(resumed, sid);
    let context = TaskContext {
        conversation: reopened.conversation_context(&sid).unwrap(),
        coding_project: None,
    };
    let task = agent(brain, calls.clone())
        .run_task_with_context("yes", &context, &cb)
        .await;
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.intent, "CODING");
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(cb.requested.load(Ordering::SeqCst), 1);
    assert_eq!(
        calls.lock().unwrap()[0]["working_dir"],
        root.to_string_lossy().as_ref()
    );
    assert_eq!(task.tool_calls[0].tool, "coding.startPlan");
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn yes_does_not_bypass_the_concrete_launch_approval() {
    let root = fixture();
    let history = HistoryStore::open(&root.join("history.db")).unwrap();
    let sid = history
        .session_for_request(None, Some(root.to_str().unwrap()), "theme")
        .unwrap();
    history
        .add_message(
            &sid,
            "user",
            &format!("Project: {}. {GOAL}", root.display()),
        )
        .unwrap();
    history.add_message(&sid, "comrade", PROPOSAL).unwrap();
    let context = TaskContext {
        conversation: history.conversation_context(&sid).unwrap(),
        coding_project: history.project_directory(&sid).map(PathBuf::from),
    };
    let calls = Arc::new(Mutex::new(vec![]));
    let goal = format!("Project: {}. {GOAL}", root.display());
    let brain = Arc::new(ContextBrain {
        goal: goal.clone(),
        project: root.clone(),
        seen: Mutex::new(vec![]),
    });
    let cb = Approval {
        allow: false,
        requested: AtomicUsize::new(0),
    };
    let task = agent(brain.clone(), calls.clone())
        .run_task_with_context("yes", &context, &cb)
        .await;
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(cb.requested.load(Ordering::SeqCst), 1);
    assert!(calls.lock().unwrap().is_empty());
    assert!(brain.seen.lock().unwrap()[0]
        .iter()
        .any(|m| m.role == ChatRole::System
            && m.content.contains("This is a Comrade Orch conversation")
            && m.content.contains(root.to_str().unwrap())));
    drop(history);
    std::fs::remove_dir_all(root).unwrap();
}
