/** Intent classification: LLM-first with heuristic fallback. */
use crate::llm::{ChatMessage, ChatOptions, LlmProvider};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Chat,
    Browser,
    Computer,
    Terminal,
    Filesystem,
    Coding,
    Research,
    MultiStep,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Chat => "CHAT",
            Intent::Browser => "BROWSER",
            Intent::Computer => "COMPUTER",
            Intent::Terminal => "TERMINAL",
            Intent::Filesystem => "FILESYSTEM",
            Intent::Coding => "CODING",
            Intent::Research => "RESEARCH",
            Intent::MultiStep => "MULTI_STEP",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "CHAT" => Some(Intent::Chat),
            "BROWSER" => Some(Intent::Browser),
            "COMPUTER" => Some(Intent::Computer),
            "TERMINAL" => Some(Intent::Terminal),
            "FILESYSTEM" => Some(Intent::Filesystem),
            "CODING" => Some(Intent::Coding),
            "RESEARCH" => Some(Intent::Research),
            "MULTI_STEP" => Some(Intent::MultiStep),
            _ => None,
        }
    }
}

const CODING_HINTS: &[&str] = &[
    "fix bug",
    "add feature",
    "refactor",
    "endpoint",
    "component",
    "auth bug",
    "debug",
    "write tests",
    "modify",
    "authentication",
];

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

pub fn heuristic_intent(text: &str) -> Intent {
    let t = text.to_lowercase();
    if t.contains("open brave") || t.contains("launch brave") || t.contains("open chrome") || t.contains("open firefox") || t.contains("open browser") {
        return Intent::Browser;
    }
    if t.contains("http") || t.contains("website") || t.contains("github.com") || t.contains("notifications") || t.contains("pull request") {
        return Intent::Browser;
    }
    if t.contains("open vs code") || t.contains("open vscode") || t.contains("open code") || t.contains("launch vs code") || t.contains("open terminal") || t.contains("open app") || t.contains("launch terminal") {
        return Intent::Computer;
    }
    if t.contains("docker") || t.contains("git ") || t.contains("npm ") || t.contains("run ") || t.contains("build ") || t.contains("test ") || t.contains("server") || t.contains("containers") {
        return Intent::Terminal;
    }
    if (t.contains("create") || t.contains("write") || t.contains("read") || t.contains("list") || t.contains("find")) && t.contains("file") {
        // "create a file" is a filesystem op — unless it also asks for code changes.
        if !(t.contains("add") || t.contains("fix") || t.contains("implement")) {
            return Intent::Filesystem;
        }
    }
    if contains_any(&t, CODING_HINTS)
        && (t.contains("add") || t.contains("fix") || t.contains("create") || t.contains("modify") || t.contains("refactor") || t.contains("debug") || t.contains("implement") || t.contains("footer") || t.contains("terms and conditions"))
    {
        return Intent::Coding;
    }
    if t.contains("search") || t.contains("research") || t.contains("summar") {
        return Intent::Research;
    }
    if t.contains(" and ") || t.contains("then ") || t.contains("also ") || t.contains("verify") || t.contains("fix it") {
        return Intent::MultiStep;
    }
    Intent::Chat
}

pub async fn classify_intent<P: LlmProvider>(
    llm: &P,
    user_request: &str,
) -> (Intent, &'static str) {
    let opts = ChatOptions {
        model: None,
        max_tokens: 16,
        temperature: 0.0,
        tools: vec![],
    };
    let messages = vec![
        ChatMessage::system(
            "Classify the user request into exactly one label: CHAT, BROWSER, COMPUTER, TERMINAL, FILESYSTEM, CODING, RESEARCH, MULTI_STEP. \
             CODING = modify/create/debug source code (delegate to OpenCode). \
             BROWSER = read/navigate websites (never code changes). \
             Reply with ONLY the label.",
        ),
        ChatMessage::user(user_request),
    ];
    if let Ok(res) = llm.chat(&messages, &opts).await {
        let label = res.content.trim().to_uppercase();
        if let Some(intent) = Intent::from_label(&label) {
            return (intent, "llm");
        }
    }
    (heuristic_intent(user_request), "heuristic")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_routes_common_requests() {
        assert_eq!(heuristic_intent("Fix the authentication bug in Co-Founder"), Intent::Coding);
        assert_eq!(heuristic_intent("Open GitHub and check notifications"), Intent::Browser);
        assert_eq!(heuristic_intent("Open Brave"), Intent::Browser);
        assert_eq!(heuristic_intent("Run the backend and tell me if it starts"), Intent::Terminal);
        assert_eq!(heuristic_intent("Create a file containing hello"), Intent::Filesystem);
    }
}
