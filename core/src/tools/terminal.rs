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
    #[cfg(windows)]
    let mut shell = {
        let cmd = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
            .join("System32")
            .join("cmd.exe");
        let mut shell = tokio::process::Command::new(cmd);
        shell.args(["/D", "/S", "/C"]);
        // This input is intentionally shell source. /S removes these outer
        // quotes; raw_arg preserves cmd syntax and inner quoted paths exactly.
        shell.raw_arg(format!("\"{command}\""));
        shell
    };
    #[cfg(not(windows))]
    let mut shell = {
        let mut shell = tokio::process::Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    };
    shell.current_dir(cwd).kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), shell.output()).await;
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
        "Run a shell command in the working directory using cmd.exe /D /S /C and Windows cmd syntax on Windows, or sh -c on Unix. Returns stdout/stderr/exitCode/durationMs. Prefer safe read-only commands; destructive commands need user approval."
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
            let command = args
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .trim();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "comrade terminal cwd with spaces {} {stamp}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn command_uses_cwd_with_spaces_and_quoted_relative_path() {
        let directory = TestDirectory::new();
        let result = run_command("echo cwd-ok > \"relative output.txt\"", &directory.0, 5000).await;
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(
            std::fs::read_to_string(directory.0.join("relative output.txt"))
                .unwrap()
                .trim(),
            "cwd-ok"
        );
    }

    #[tokio::test]
    async fn command_preserves_stdout_stderr_and_exit_status() {
        let directory = TestDirectory::new();
        #[cfg(windows)]
        let command = "echo stdout-message& echo stderr-message 1>&2& exit /b 37";
        #[cfg(not(windows))]
        let command = "printf 'stdout-message\\n'; printf 'stderr-message\\n' >&2; exit 37";
        let result = run_command(command, &directory.0, 5000).await;
        assert_eq!(result.exit_code, 37);
        assert_eq!(result.stdout.trim(), "stdout-message");
        assert_eq!(result.stderr.trim(), "stderr-message");
    }

    #[tokio::test]
    async fn timeout_stops_the_shell_and_returns_124() {
        let directory = TestDirectory::new();
        // Builtins only: the timeout owns and kills the busy shell itself.
        #[cfg(windows)]
        let command = "for /L %i in (1,0,2) do @rem waiting";
        #[cfg(not(windows))]
        let command = "while :; do :; done";
        let result = run_command(command, &directory.0, 100).await;
        assert_eq!(result.exit_code, 124);
        assert!(result.stderr.contains("timed out after 100ms"));
        assert!(result.duration_ms < 3000);
    }
}
