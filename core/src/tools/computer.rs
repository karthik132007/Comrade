/**
 * Computer-control tools. Real app launching via the OS;
 * mouse/keyboard/screenshot are honest stubs until a native input backend lands.
 */
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use super::types::{Risk, Tool, ToolContext, ToolResult};

fn binaries_for(target: &str) -> Vec<&'static str> {
    #[cfg(windows)]
    return match target {
        "brave" => vec!["brave.exe"],
        "chrome" => vec!["chrome.exe", "chromium.exe"],
        "firefox" => vec!["firefox.exe"],
        "vs code" | "vscode" | "code" => vec!["code.exe", "code.cmd"],
        "terminal" => vec!["wt.exe", "cmd.exe"],
        _ => vec![],
    };
    #[cfg(not(windows))]
    match target {
        "brave" => vec!["brave", "brave-browser"],
        "chrome" => vec!["google-chrome", "chromium"],
        "firefox" => vec!["firefox"],
        "vs code" | "vscode" | "code" => vec!["code"],
        "terminal" => vec!["gnome-terminal", "x-terminal-emulator"],
        _ => vec![],
    }
}

fn desktop_ids_for(target: &str) -> Vec<&'static str> {
    match target {
        "brave" => vec!["brave-origin", "brave-browser", "brave"],
        "chrome" => vec!["com.google.Chrome", "google-chrome"],
        "firefox" => vec!["firefox"],
        "vs code" | "vscode" | "code" => vec!["com.visualstudio.code", "code"],
        "terminal" => vec!["org.gnome.Terminal", "gnome-terminal"],
        _ => vec![],
    }
}

fn spawn_detached(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

async fn on_path(bin: &str) -> bool {
    #[cfg(windows)]
    let mut command = {
        // An absolute system executable avoids both Git Bash and PATH spoofing
        // of the lookup utility. No shell interprets the requested name.
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let mut command = tokio::process::Command::new(std::path::PathBuf::from(root).join("System32/where.exe"));
        command.arg(bin);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = tokio::process::Command::new("sh");
        command
        .arg("-c")
        .arg("command -v -- \"$1\"")
        .arg("sh")
        .arg(bin);
        command
    };
    command
        .kill_on_drop(true)
        .output()
        .await
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_path_lookup_finds_executable_and_rejects_shell_source() {
        #[cfg(windows)]
        let executable = "cmd.exe";
        #[cfg(not(windows))]
        let executable = "sh";
        assert!(on_path(executable).await);
        assert!(!on_path("comrade-nonexistent-app-960d24").await);
        assert!(!on_path("cmd.exe & echo injected").await);
    }

    #[cfg(windows)]
    #[test]
    fn windows_application_aliases_use_native_binaries() {
        assert!(binaries_for("chrome").contains(&"chrome.exe"));
        assert!(binaries_for("terminal").contains(&"cmd.exe"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_path_lookup_supports_spaces_in_executable_path() {
        let dir = std::env::temp_dir().join(format!("comrade app lookup {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("application with spaces.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        assert!(on_path(&executable.to_string_lossy()).await);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

async fn launch_app(name: &str) -> Option<String> {
    let target = name.trim().to_lowercase();
    if cfg!(target_os = "macos") {
        let application = match target.as_str() {
            "brave" => "Brave Browser",
            "chrome" => "Google Chrome",
            "firefox" => "Firefox",
            "vs code" | "vscode" | "code" => "Visual Studio Code",
            "terminal" => "Terminal",
            _ => name.trim(),
        };
        let launched = tokio::process::Command::new("open")
            .args(["-a", application])
            .output()
            .await
            .map(|output| output.status.success())
            .unwrap_or(false);
        if launched {
            return Some(format!("application:{application}"));
        }
    }
    let mut candidates = binaries_for(target.as_str());
    if candidates.is_empty() {
        // Unknown app: try the literal name as a binary (may contain spaces — skip).
        if !target.contains(' ') {
            candidates.push(Box::leak(target.clone().into_boxed_str()));
        }
    }
    for bin in candidates {
        if on_path(bin).await && spawn_detached(bin, &[]) {
            return Some(format!("binary:{bin}"));
        }
    }
    if cfg!(target_os = "linux") {
        for id in desktop_ids_for(target.as_str()) {
            if spawn_detached("gtk-launch", &[id]) {
                return Some(format!("desktop:{id}"));
            }
        }
    }
    None
}

pub struct OpenApplicationTool;

impl Tool for OpenApplicationTool {
    fn name(&self) -> &'static str {
        "computer.openApplication"
    }

    fn description(&self) -> &'static str {
        "Open a desktop application by name (e.g. Brave, VS Code, terminal)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "Application name" } },
            "required": ["name"],
        })
    }

    fn risk(&self, _args: &serde_json::Value) -> Risk {
        Risk::Safe
    }

    fn execute<'a>(
        &'a self,
        args: &'a serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let name = args.get("name").and_then(|n| n.as_str()).unwrap_or("").trim();
            if name.is_empty() {
                return ToolResult::fail("EMPTY_NAME", "No application name provided.");
            }
            match launch_app(name).await {
                Some(via) => ToolResult::ok(serde_json::json!({
                    "application": name,
                    "via": via,
                    "launched": true,
                })),
                None => ToolResult::fail(
                    "LAUNCH_FAILED",
                    format!("Could not launch application \"{name}\". Is it installed?"),
                ),
            }
        })
    }
}

/// Honest stub: present in the tool list, fails with NOT_IMPLEMENTED.
pub struct StubTool {
    pub tool_name: &'static str,
    pub blurb: &'static str,
}

impl Tool for StubTool {
    fn name(&self) -> &'static str {
        self.tool_name
    }

    fn description(&self) -> &'static str {
        self.blurb
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    fn risk(&self, _args: &serde_json::Value) -> Risk {
        Risk::Dangerous
    }

    fn execute<'a>(
        &'a self,
        _args: &'a serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        let name = self.tool_name;
        Box::pin(async move {
            ToolResult::fail(
                "NOT_IMPLEMENTED",
                format!("{name} needs a native input backend and is not wired up yet."),
            )
        })
    }
}

pub fn computer_stubs() -> Vec<StubTool> {
    [
        ("computer.screenshot", "Capture the screen (needs a native backend)."),
        ("computer.closeApplication", "Close an application (needs process management)."),
        ("computer.mouseMove", "Move the mouse (needs a native input backend)."),
        ("computer.mouseClick", "Click the mouse (needs a native input backend)."),
        ("computer.doubleClick", "Double-click (needs a native input backend)."),
        ("computer.type", "Type text (needs a native input backend)."),
        ("computer.keyPress", "Press a key (needs a native input backend)."),
        ("computer.hotkey", "Press a hotkey combo (needs a native input backend)."),
        ("computer.getScreenSize", "Get screen size (needs a native backend)."),
    ]
    .into_iter()
    .map(|(tool_name, blurb)| StubTool { tool_name, blurb })
    .collect()
}
