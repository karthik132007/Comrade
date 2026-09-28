/**
 * Coding specialist router. CODING tasks go to the user's default enabled
 * coding agent (opencode, claude, codex, copilot, qwen, …) — picked in
 * onboarding/Settings, stored in comrade.conf. Enable-all is the default:
 * every detected agent is usable unless unchecked.
 */
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use super::types::{Risk, Tool, ToolContext, ToolResult};
use crate::prefs::{coding_agent_candidates, detect_coding_agents, CodingAgentInfo};

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
        detected.iter().find(|d| d.id == id && enabled_set.iter().any(|e| e == id))
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
            }
            // unreachable: find above covers all shapes, but keep exhaustive clarity
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

fn build_argv(agent_id: &str, task: &str) -> Vec<String> {
    match agent_id {
        "claude" => vec!["-p".into(), task.into(), "--output-format".into(), "json".into()],
        "codex" => vec!["exec".into(), task.into()],
        "copilot" => vec!["-p".into(), task.into(), "-s".into()],
        "qwen" => vec![task.into(), "-o".into(), "json".into()],
        _ => vec!["run".into(), "--format".into(), "json".into(), task.into()], // opencode
    }
}

/// Pull human-readable text out of the various JSON envelopes agents emit.
fn extract_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        serde_json::Value::Object(m) => {
            for k in ["result", "response", "text", "output", "message"] {
                if let Some(s) = m.get(k).and_then(extract_text) {
                    return Some(s);
                }
            }
            if let Some(arr) = m.get("content").and_then(|c| c.as_array()) {
                let joined = arr.iter().filter_map(extract_text).collect::<Vec<_>>().join("\n");
                if !joined.trim().is_empty() {
                    return Some(joined);
                }
            }
            None
        }
        serde_json::Value::Array(a) => {
            let joined = a.iter().filter_map(extract_text).collect::<Vec<_>>().join("\n");
            if joined.trim().is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    }
}

fn parse_output(stdout: &str) -> String {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return "(agent returned no output)".to_string();
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(text) = extract_text(&json) {
            return text.chars().take(50_000).collect();
        }
    }
    trimmed.chars().take(50_000).collect()
}

pub async fn run_coding_task(
    agent: &ResolvedAgent,
    task: &str,
    cwd: &Path,
) -> Result<String, String> {
    let argv = build_argv(&agent.id, task);
    let out = tokio::time::timeout(
        Duration::from_secs(600),
        tokio::process::Command::new(&agent.bin)
            .args(&argv)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| format!("{} task timed out after 10 minutes.", agent.name))?
    .map_err(|e| format!("Failed to spawn {}: {e}", agent.name))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        let stderr: String = String::from_utf8_lossy(&out.stderr).chars().take(800).collect();
        return Err(format!(
            "{} exited with {}: {stderr}",
            agent.name,
            out.status.code().unwrap_or(-1)
        ));
    }
    Ok(parse_output(&stdout))
}

pub struct CodingTool;

impl Tool for CodingTool {
    fn name(&self) -> &'static str {
        "coding.executeTask"
    }

    fn description(&self) -> &'static str {
        "Delegate a CODING task to the user's enabled coding agent (opencode, claude, codex, copilot, qwen — default picked in Settings). Pass a precise task description plus optional agent id. Requires user approval."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "Precise coding task for the agent" },
                "workingDir": { "type": "string", "description": "Repo dir (default: agent cwd)" },
                "agent": { "type": "string", "description": "Agent id override, e.g. claude (default: Settings default)" },
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
            let task = args.get("task").and_then(|t| t.as_str()).unwrap_or("").trim();
            if task.is_empty() {
                return ToolResult::fail("EMPTY_TASK", "No coding task provided.");
            }
            let prefs = crate::prefs::load();
            let detected = detect_coding_agents();
            let requested = args.get("agent").and_then(|a| a.as_str());
            let agent = match resolve_agent(&prefs.coding.agents, &prefs.coding.default, &detected, requested) {
                Ok(a) => a,
                Err(e) => return ToolResult::fail("NO_CODING_AGENT", e),
            };
            let cwd = args
                .get("workingDir")
                .and_then(|w| w.as_str())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ctx.cwd.clone());
            match run_coding_task(&agent, task, &cwd).await {
                Ok(output) => ToolResult::ok(serde_json::json!({
                    "task": task,
                    "cwd": cwd.to_string_lossy(),
                    "agent": agent.id,
                    "output": output,
                })),
                Err(e) => ToolResult::fail("CODING_FAILED", e.to_string()),
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

    #[test]
    fn output_parsing_covers_envelopes() {
        assert_eq!(parse_output(r#"{"result": "done!"}"#), "done!");
        assert_eq!(parse_output(r#"{"response": "hi"}"#), "hi");
        assert_eq!(
            parse_output(r#"{"content": [{"text": "a"}, {"text": "b"}]}"#),
            "a\nb"
        );
        assert_eq!(parse_output("plain text"), "plain text");
    }
}
