/**
 * Computer-control tools. Real app launching via the OS;
 * mouse/keyboard/screenshot are honest stubs until a native input backend lands.
 */
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use super::types::{Risk, Tool, ToolContext, ToolResult};

fn binaries_for(target: &str) -> Vec<&'static str> {
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
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .output()
        .await
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

async fn launch_app(name: &str) -> Option<String> {
    let target = name.trim().to_lowercase();
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
    for id in desktop_ids_for(target.as_str()) {
        if spawn_detached("gtk-launch", &[id]) {
            return Some(format!("desktop:{id}"));
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
