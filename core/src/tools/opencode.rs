/** OpenCode coding specialist. Real CLI integration — never faked. */
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use super::types::{Risk, Tool, ToolContext, ToolResult};

pub async fn is_opencode_available(bin: &str) -> (bool, String) {
    match tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(bin).arg("--version").output(),
    )
    .await
    {
        Ok(Ok(out)) if out.status.success() => {
            (true, String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        _ => (false, String::new()),
    }
}

pub async fn run_coding_task(bin: &str, task: &str, cwd: &Path) -> Result<String, String> {
    let out = tokio::time::timeout(
        Duration::from_secs(600),
        tokio::process::Command::new(bin).arg("run").arg("--format").arg("json").arg(task).current_dir(cwd).output(),
    )
    .await
    .map_err(|_| "OpenCode task timed out after 10 minutes.".to_string())?
    .map_err(|e| format!("Failed to spawn OpenCode: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        let stderr: String = String::from_utf8_lossy(&out.stderr).chars().take(500).collect();
        return Err(format!("OpenCode exited with {}: {stderr}", out.status.code().unwrap_or(-1)));
    }
    // Try to extract human-readable text from the JSON envelope.
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout) {
        if let Some(text) = parsed.get("text").and_then(|t| t.as_str()) {
            return Ok(text.to_string());
        }
    }
    Ok(stdout)
}

pub struct OpenCodeTool {
    pub bin: String,
}

impl Tool for OpenCodeTool {
    fn name(&self) -> &'static str {
        "opencode.executeTask"
    }

    fn description(&self) -> &'static str {
        "Delegate a CODING task to OpenCode (inspect repo, modify code, run tests). Pass a precise task description. Requires user approval."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "Precise coding task for OpenCode" },
                "workingDir": { "type": "string", "description": "Repo dir (default: agent cwd)" },
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
            let (available, version) = is_opencode_available(&self.bin).await;
            if !available {
                return ToolResult::fail(
                    "OPENCODE_UNAVAILABLE",
                    format!(
                        "OpenCode CLI (\"{}\") is not installed or not on PATH. Install it to enable coding tasks.",
                        self.bin
                    ),
                );
            }
            let cwd = args
                .get("workingDir")
                .and_then(|w| w.as_str())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ctx.cwd.clone());
            match run_coding_task(&self.bin, task, &cwd).await {
                Ok(output) => ToolResult::ok(serde_json::json!({
                    "task": task,
                    "cwd": cwd.to_string_lossy(),
                    "output": output,
                    "opencodeVersion": version,
                })),
                Err(e) => ToolResult::fail("OPENCODE_FAILED", format!("OpenCode task failed: {e}")),
            }
        })
    }
}
