//! Fixed adapters: neither prompts nor model ids can add runner options.
use serde::Serialize;

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

pub fn redact(mut text: String, secrets: &[String]) -> String {
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret, "[REDACTED]");
    }
    // Strip control characters but preserve layout; UI always renders plain text.
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
