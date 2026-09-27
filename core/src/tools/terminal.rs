/** Terminal tool: execute commands, return stdout/stderr/exitCode/duration. */
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use super::types::{Risk, Tool, ToolContext, ToolResult};
use crate::permissions::terminal_risk;

pub struct TerminalOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

pub async fn run_command(command: &str, cwd: &std::path::Path, timeout_ms: u64) -> TerminalOutput {
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        tokio::process::Command::new("sh").arg("-c").arg(command).current_dir(cwd).output(),
    )
    .await;
    let elapsed = started.elapsed().as_millis() as u64;
    match result {
        Ok(Ok(out)) => TerminalOutput {
            stdout: truncate(&String::from_utf8_lossy(&out.stdout)),
            stderr: truncate(&String::from_utf8_lossy(&out.stderr)),
            exit_code: out.status.code().unwrap_or(1),
            duration_ms: elapsed,
        },
        Ok(Err(e)) => TerminalOutput {
            stdout: String::new(),
            stderr: format!("Failed to spawn shell: {e}"),
            exit_code: 1,
            duration_ms: elapsed,
        },
        Err(_) => TerminalOutput {
            stdout: String::new(),
            stderr: format!("Command timed out after {timeout_ms}ms."),
            exit_code: 124,
            duration_ms: elapsed,
        },
    }
}

fn truncate(s: &str) -> String {
    const MAX: usize = 200_000;
    if s.len() <= MAX {
        return s.to_string();
    }
    let mut out: String = s.chars().take(MAX).collect();
    out.push_str("\n…[truncated]");
    out
}

pub struct TerminalTool;

impl Tool for TerminalTool {
    fn name(&self) -> &'static str {
        "terminal.execute"
    }

    fn description(&self) -> &'static str {
        "Run a shell command (sh -c) in the working directory. Returns stdout/stderr/exitCode/durationMs. Prefer safe read-only commands; destructive commands need user approval."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to run" },
                "timeoutMs": { "type": "number", "description": "Timeout in ms (default 60000, max 300000)" },
            },
            "required": ["command"],
        })
    }

    fn risk(&self, args: &serde_json::Value) -> Risk {
        terminal_risk(args.get("command").and_then(|c| c.as_str()).unwrap_or(""))
    }

    fn execute<'a>(
        &'a self,
        args: &'a serde_json::Value,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let command = args.get("command").and_then(|c| c.as_str()).unwrap_or("").trim();
            if command.is_empty() {
                return ToolResult::fail("EMPTY_COMMAND", "No command provided.");
            }
            let timeout_ms = args
                .get("timeoutMs")
                .and_then(|t| t.as_u64())
                .unwrap_or(60_000)
                .clamp(1_000, 300_000);
            let out = run_command(command, &ctx.cwd, timeout_ms).await;
            ToolResult::ok(serde_json::json!({
                "stdout": out.stdout,
                "stderr": out.stderr,
                "exitCode": out.exit_code,
                "durationMs": out.duration_ms,
            }))
        })
    }
}
