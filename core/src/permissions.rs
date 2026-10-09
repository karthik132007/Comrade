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

const DANGEROUS_WINDOWS_COMMANDS: &[&str] = &["del", "erase", "format", "diskpart"];

pub fn terminal_risk(command: &str) -> Risk {
    let c = command.to_lowercase();
    if c == "sudo" || c.starts_with("sudo ") {
        return Risk::Dangerous;
    }
    if DANGEROUS_COMMAND_FRAGMENTS.iter().any(|p| c.contains(p)) {
        return Risk::Dangerous;
    }
    // Keep the existing Unix policy, and cover cmd's built-in destructive
    // aliases regardless of option order or extra spaces.
    let separated = c.replace('&', " & ").replace('|', " | ");
    let words: Vec<_> = separated.split_ascii_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        let command = word.trim_matches(|c| matches!(c, '"' | '(' | ')'));
        let command = command.rsplit(['\\', '/']).next().unwrap_or(command);
        let command = command
            .strip_suffix(".exe")
            .or_else(|| command.strip_suffix(".com"))
            .unwrap_or(command);
        if DANGEROUS_WINDOWS_COMMANDS.contains(&command) {
            return Risk::Dangerous;
        }
        if matches!(command, "rd" | "rmdir")
            && words[index + 1..]
                .iter()
                .take_while(|word| !matches!(**word, "&" | "&&" | "|" | "||"))
                .any(|option| option.eq_ignore_ascii_case("/s"))
        {
            return Risk::Dangerous;
        }
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
    "coding.startPlan",
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
    if tool_name == "coding.startPlan" || tool_name == "coding.executeTask" {
        return format!("Start local coding agents in this project. Agents may edit files directly using their existing logins and permissions.\n{}",
            serde_json::to_string_pretty(args).unwrap_or_default());
    }
    let arg_str = match args.as_object() {
        Some(map) => map
            .iter()
            .map(|(k, v)| {
                let mut s = v.to_string();
                if s.chars().count() > 120 {
                    s = s.chars().take(120).collect();
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
        assert!(matches!(
            terminal_risk("sudo apt install x"),
            Risk::Dangerous
        ));
        assert!(matches!(
            terminal_risk("git push origin main"),
            Risk::Dangerous
        ));
        assert!(matches!(terminal_risk("ls -la"), Risk::Safe));
    }

    #[test]
    fn flags_destructive_windows_cmd_aliases() {
        for command in [
            r#"rmdir /s "C:\work folder""#,
            r#"RD /Q /S "C:\work folder""#,
            r#"rd   "C:\work folder"   /s"#,
            "del /f /q file.txt",
            "erase file.txt",
            "FORMAT D:",
            "diskpart /s commands.txt",
            "del",
            "echo ready & del file.txt",
            "echo ready&del file.txt",
            r#""C:\Windows\System32\format.com" D:"#,
        ] {
            assert!(
                matches!(terminal_risk(command), Risk::Dangerous),
                "{command}"
            );
        }
        for command in [
            "dir",
            "echo hello",
            "type notes.txt",
            "rmdir empty-folder",
            "git status",
        ] {
            assert!(matches!(terminal_risk(command), Risk::Safe), "{command}");
        }
    }

    #[test]
    fn tool_policy_matches_spec() {
        assert!(matches!(
            tool_risk("filesystem.write", &json!({})),
            Risk::Dangerous
        ));
        assert!(matches!(
            tool_risk("filesystem.read", &json!({})),
            Risk::Safe
        ));
        assert!(matches!(
            tool_risk("terminal.execute", &json!({"command": "rm -rf x"})),
            Risk::Dangerous
        ));
    }
}
