/** Permission system: safe actions auto-run, dangerous ones need approval. */
use crate::tools::types::Risk;

const DANGEROUS_COMMAND_FRAGMENTS: &[&str] = &[
    "rm -rf",
    "sudo ",
    "sudo\t",
    "mkfs",
    ":(){", // fork bomb
    "of=/dev",
    "shutdown",
    "reboot",
    "git reset --hard",
    "git push",
    "git clean -fd",
    ">/dev/sd",
];

const DANGEROUS_SEND_FRAGMENTS: &[&str] = &["send email", "purchase", "submit form"];

pub fn terminal_risk(command: &str) -> Risk {
    let c = command.to_lowercase();
    if c == "sudo" || c.starts_with("sudo ") {
        return Risk::Dangerous;
    }
    if DANGEROUS_COMMAND_FRAGMENTS.iter().any(|p| c.contains(p)) {
        return Risk::Dangerous;
    }
    if DANGEROUS_SEND_FRAGMENTS.iter().any(|p| c.contains(p)) {
        return Risk::Dangerous;
    }
    Risk::Safe
}

const ALWAYS_DANGEROUS_TOOLS: &[&str] = &[
    "computer.mouseClick",
    "computer.doubleClick",
    "computer.type",
    "computer.keyPress",
    "computer.hotkey",
    "browser.click",
    "browser.type",
    "browser.press",
    "filesystem.write",
    "filesystem.create",
    "coding.executeTask",
];

pub fn tool_risk(tool_name: &str, args: &serde_json::Value) -> Risk {
    if tool_name == "terminal.execute" {
        let cmd = args.get("command").and_then(|c| c.as_str()).unwrap_or("");
        return terminal_risk(cmd);
    }
    if ALWAYS_DANGEROUS_TOOLS.contains(&tool_name) {
        return Risk::Dangerous;
    }
    Risk::Safe
}

pub fn describe_tool_call(tool_name: &str, args: &serde_json::Value) -> String {
    let arg_str = match args.as_object() {
        Some(map) => map
            .iter()
            .map(|(k, v)| {
                let mut s = v.to_string();
                if s.len() > 120 {
                    s.truncate(120);
                }
                format!("{k}={s}")
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => String::new(),
    };
    format!("{tool_name} {arg_str}").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flags_destructive_commands() {
        assert!(matches!(terminal_risk("rm -rf / tmp"), Risk::Dangerous));
        assert!(matches!(terminal_risk("sudo apt install x"), Risk::Dangerous));
        assert!(matches!(terminal_risk("git push origin main"), Risk::Dangerous));
        assert!(matches!(terminal_risk("ls -la"), Risk::Safe));
    }

    #[test]
    fn tool_policy_matches_spec() {
        assert!(matches!(tool_risk("filesystem.write", &json!({})), Risk::Dangerous));
        assert!(matches!(tool_risk("filesystem.read", &json!({})), Risk::Safe));
        assert!(matches!(
            tool_risk("terminal.execute", &json!({"command": "rm -rf x"})),
            Risk::Dangerous
        ));
    }
}
