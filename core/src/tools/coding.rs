/**
 * Local coding specialist router. CODING tasks go to the user's default enabled
 * coding agent (opencode, claude, codex, copilot, qwen, …) — picked in
 * onboarding/Settings, stored in comrade.conf. Enable-all is the default:
 * every detected agent is usable unless unchecked.
 */
use std::future::Future;
use std::pin::Pin;

use super::types::{Risk, Tool, ToolContext, ToolResult};
use crate::prefs::{coding_agent_candidates, CodingAgentInfo};

pub struct ResolvedAgent {
    pub id: String,
    pub name: String,
    pub bin: String,
}

/// Pick the agent for a task: explicit request → prefs default → priority order.
/// Only enabled, installed, exec-capable agents are eligible.
pub fn resolve_agent(
    enabled: &[String],
    default: &str,
    detected: &[CodingAgentInfo],
    requested: Option<&str>,
) -> Result<ResolvedAgent, String> {
    let order: Vec<&str> = coding_agent_candidates().iter().map(|c| c.id).collect();
    let enabled_set: Vec<String> = if enabled.is_empty() {
        // No conf choice yet = enable all detected.
        detected.iter().map(|d| d.id.clone()).collect()
    } else {
        enabled.to_vec()
    };
    let find = |id: &str| -> Option<&CodingAgentInfo> {
        detected
            .iter()
            .find(|d| d.id == id && enabled_set.iter().any(|e| e == id))
    };

    if let Some(req) = requested.map(str::trim).filter(|s| !s.is_empty()) {
        let req = req.to_lowercase();
        match detected.iter().find(|d| d.id == req) {
            None => {
                return Err(format!(
                    "Coding agent \"{req}\" is not installed. Enable another in Settings."
                ));
            }
            Some(info) if !enabled_set.iter().any(|e| e == &info.id) => {
                return Err(format!(
                    "Coding agent \"{req}\" is disabled. Enable it in Settings."
                ));
            }
            Some(info) if !info.exec_supported => {
                return Err(format!(
                    "{} is enabled but non-interactive execution is not supported for it yet. \
                     Pick another default in Settings.",
                    info.name
                ));
            }
            Some(info) => {
                return Ok(ResolvedAgent {
                    id: info.id.clone(),
                    name: info.name.clone(),
                    bin: info.bin.clone(),
                });
            } // unreachable: find above covers all shapes, but keep exhaustive clarity
        }
    }

    let default = default.trim().to_lowercase();
    if !default.is_empty() {
        if let Some(info) = find(&default) {
            if info.exec_supported {
                return Ok(ResolvedAgent {
                    id: info.id.clone(),
                    name: info.name.clone(),
                    bin: info.bin.clone(),
                });
            }
        }
    }
    for id in order {
        if let Some(info) = find(id) {
            if info.exec_supported {
                return Ok(ResolvedAgent {
                    id: info.id.clone(),
                    name: info.name.clone(),
                    bin: info.bin.clone(),
                });
            }
        }
    }
    Err("No coding agents available: none installed, or all are disabled in Settings.".to_string())
}

pub struct CodingTool;

impl Tool for CodingTool {
    fn name(&self) -> &'static str {
        "coding.executeTask"
    }

    fn description(&self) -> &'static str {
        "Start a local coding agent in the selected project using its existing login and native permissions. Requires approval: edits affect the project directly. Returns a run id immediately; monitor with coding.runStatus. Use coding.startPlan for multiple agents/dependencies."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "Precise coding task for the agent" },
                "workingDir": { "type": "string", "description": "Repo dir (default: agent cwd)" },
                "agent": { "type": "string", "description": "Agent id override (default: Settings default)" },
                "model": { "type": "string" },
            },
            "required": ["task"],
        })
    }

    fn risk(&self, _args: &serde_json::Value) -> Risk {
        Risk::Dangerous
    }

    fn execute<'a>(
        &'a self,
        args: &'a serde_json::Value,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let task = args
                .get("task")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim();
            if task.is_empty() {
                return ToolResult::fail("EMPTY_TASK", "No coding task provided.");
            }
            let plan = crate::orchestration::Plan {
                title: task.chars().take(100).collect(),
                working_dir: args
                    .get("workingDir")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| ctx.cwd.to_string_lossy().into_owned()),
                tasks: vec![crate::orchestration::TaskSpec {
                    id: "task-1".into(),
                    task: task.into(),
                    agent: args
                        .get("agent")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    model: args
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    depends_on: vec![],
                }],
                max_parallel: 1,
                timeout_secs: 600,
            };
            match crate::orchestration::global().start(plan).await {
                Ok(run) => ToolResult::ok(
                    serde_json::json!({"run_id":run.id,"status":run.status,"message":"Local job queued. Monitor coding.runStatus or Comrade Orch."}),
                ),
                Err(e) => ToolResult::fail("CODING_FAILED", e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(ids: &[&str]) -> Vec<CodingAgentInfo> {
        ids.iter()
            .map(|id| CodingAgentInfo {
                id: id.to_string(),
                name: id.to_string(),
                bin: format!("/usr/bin/{id}"),
                version: String::new(),
                exec_supported: !["antigravity", "kimi", "deepseek"].contains(id),
            })
            .collect()
    }

    #[test]
    fn router_prefers_default_then_priority() {
        let d = detected(&["opencode", "claude", "copilot"]);
        // Empty prefs = enable all, no default → priority order wins.
        let a = resolve_agent(&[], "", &d, None).unwrap();
        assert_eq!(a.id, "opencode");
        // Explicit default respected.
        let a = resolve_agent(&[], "claude", &d, None).unwrap();
        assert_eq!(a.id, "claude");
        // Explicit request wins.
        let a = resolve_agent(&[], "", &d, Some("copilot")).unwrap();
        assert_eq!(a.id, "copilot");
        // Restricted enable list constrains fallback.
        let a = resolve_agent(&["copilot".into()], "", &d, None).unwrap();
        assert_eq!(a.id, "copilot");
    }

    #[test]
    fn router_rejects_gracefully() {
        let d = detected(&["antigravity"]);
        assert!(resolve_agent(&[], "", &d, None).is_err()); // exec unsupported
        assert!(resolve_agent(&[], "", &d, Some("antigravity")).is_err());
        assert!(resolve_agent(&[], "", &[], None).is_err()); // nothing installed
        assert!(resolve_agent(&["claude".into()], "", &d, None).is_err()); // enabled but absent
    }
}
