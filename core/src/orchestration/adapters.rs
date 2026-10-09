//! Fixed adapters: neither prompts nor model ids can add runner options.
use serde::Serialize;
use std::borrow::Cow;
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct Adapter {
    pub id: &'static str,
    pub name: &'static str,
    pub binary: &'static str,
    pub credentials: &'static [&'static str],
}

pub fn adapters() -> Vec<Adapter> {
    vec![
        Adapter {
            id: "opencode",
            name: "OpenCode",
            binary: "opencode",
            credentials: &["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENROUTER_API_KEY"],
        },
        Adapter {
            id: "codex",
            name: "Codex",
            binary: "codex",
            credentials: &["CODEX_API_KEY", "OPENAI_API_KEY"],
        },
        Adapter {
            id: "claude",
            name: "Claude Code",
            binary: "claude",
            credentials: &["ANTHROPIC_API_KEY"],
        },
        Adapter {
            id: "copilot",
            name: "Copilot CLI",
            binary: "copilot",
            credentials: &["COPILOT_GITHUB_TOKEN"],
        },
        Adapter {
            id: "qwen",
            name: "Qwen Code",
            binary: "qwen",
            credentials: &["OPENAI_API_KEY"],
        },
        Adapter {
            id: "hermes",
            name: "Hermes",
            binary: "hermes",
            credentials: &["OPENROUTER_API_KEY"],
        },
    ]
}

pub fn adapter(id: &str) -> Result<Adapter, String> {
    adapters()
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("Unsupported coding agent: {id}"))
}

pub fn argv(id: &str, task: &str, model: &str) -> Result<Vec<String>, String> {
    let a = adapter(id)?;
    let flags: &[&str] = match id {
        "codex" => &[
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "workspace-write",
        ],
        "claude" => &[
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "acceptEdits",
            "--max-turns",
            "30",
        ],
        "copilot" => &["-p", "-s"],
        "qwen" => &[
            "-p",
            "--output-format",
            "stream-json",
            "--max-session-turns",
            "30",
        ],
        "hermes" => &["chat", "-q"],
        _ => &["run", "--format", "json"],
    };
    let mut args = vec![a.binary.to_string()];
    args.extend(flags.iter().map(|s| s.to_string()));
    if !model.is_empty() {
        args.extend(["--model".into(), model.into()]);
    }
    // For -p/-q agents the prompt follows the option immediately.
    if let Some(pos) = args.iter().position(|s| s == "-p" || s == "-q") {
        args.insert(pos + 1, task.into());
    } else {
        args.push("--".into());
        args.push(task.into());
    }
    Ok(args)
}

/// Batch shims cannot carry CR/LF through Rust's Windows argument encoder.
/// Keep the task as one regular argument; never interpolate it into shell code.
pub(super) fn task_argument(task: &str, windows_batch: bool) -> Result<Cow<'_, str>, String> {
    if !windows_batch {
        return Ok(Cow::Borrowed(task));
    }
    let encoded = serde_json::to_string(task).map_err(|e| format!("Cannot encode task: {e}"))?;
    Ok(Cow::Owned(format!(
        "Decode this JSON string as the task text, preserving escaped newlines: {encoded}"
    )))
}

pub fn argv_for_binary(
    id: &str,
    task: &str,
    model: &str,
    binary: &Path,
) -> Result<Vec<String>, String> {
    let windows_batch = cfg!(windows)
        && binary
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            });
    let task = task_argument(task, windows_batch)?;
    argv(id, &task, model)
}

pub fn redact(mut text: String, secrets: &[String]) -> String {
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret, "[REDACTED]");
    }
    // Strip control characters but preserve layout; UI always renders plain text.
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
