/** Intent classification: LLM-first with heuristic fallback. */
use crate::llm::{ChatMessage, ChatOptions, ChatRole, LlmProvider};

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
    if t.contains("coding.startplan")
        || t.contains("coding.executetask")
        || t.contains("coding agents")
        || t.contains("comrade orch")
    {
        return Intent::Coding;
    }
    if t.contains("open brave")
        || t.contains("launch brave")
        || t.contains("open chrome")
        || t.contains("open firefox")
        || t.contains("open browser")
    {
        return Intent::Browser;
    }
    if t.contains("http")
        || t.contains("website")
        || t.contains("github.com")
        || t.contains("notifications")
        || t.contains("pull request")
    {
        return Intent::Browser;
    }
    if t.contains("open vs code")
        || t.contains("open vscode")
        || t.contains("open code")
        || t.contains("launch vs code")
        || t.contains("open terminal")
        || t.contains("open app")
        || t.contains("launch terminal")
    {
        return Intent::Computer;
    }
    if t.contains("docker")
        || t.contains("git ")
        || t.contains("npm ")
        || t.contains("run ")
        || t.contains("build ")
        || t.contains("test ")
        || t.contains("server")
        || t.contains("containers")
    {
        return Intent::Terminal;
    }
    if (t.contains("create")
        || t.contains("write")
        || t.contains("read")
        || t.contains("list")
        || t.contains("find"))
        && t.contains("file")
    {
        // "create a file" is a filesystem op — unless it also asks for code changes.
        if !(t.contains("add") || t.contains("fix") || t.contains("implement")) {
            return Intent::Filesystem;
        }
    }
    if contains_any(&t, CODING_HINTS)
        && (t.contains("add")
            || t.contains("fix")
            || t.contains("create")
            || t.contains("modify")
            || t.contains("refactor")
            || t.contains("debug")
            || t.contains("implement")
            || t.contains("footer")
            || t.contains("terms and conditions"))
    {
        return Intent::Coding;
    }
    if t.contains("search") || t.contains("research") || t.contains("summar") {
        return Intent::Research;
    }
    if t.contains(" and ")
        || t.contains("then ")
        || t.contains("also ")
        || t.contains("verify")
        || t.contains("fix it")
    {
        return Intent::MultiStep;
    }
    Intent::Chat
}

pub async fn classify_intent<P: LlmProvider>(
    llm: &P,
    user_request: &str,
) -> (Intent, &'static str) {
    classify_intent_with_context(llm, user_request, &[]).await
}

pub async fn classify_intent_with_context<P: LlmProvider>(
    llm: &P,
    user_request: &str,
    context: &[ChatMessage],
) -> (Intent, &'static str) {
    let opts = ChatOptions {
        model: None,
        max_tokens: 16,
        temperature: 0.0,
        tools: vec![],
    };
    let mut messages = vec![
        ChatMessage::system(
            "Classify the user request into exactly one label: CHAT, BROWSER, COMPUTER, TERMINAL, FILESYSTEM, CODING, RESEARCH, MULTI_STEP. \
             CODING = modify/create/debug source code (delegate through the local coding orchestrator). \
             BROWSER = read/navigate websites (never code changes). \
             Classify the latest message in its conversation context. Short replies such as yes, no or proceed refer to the preceding request. Reply with ONLY the label.",
        ),
    ];
    messages.extend(
        context
            .iter()
            .filter(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant))
            .cloned(),
    );
    messages.push(ChatMessage::user(user_request));
    if let Ok(res) = llm.chat(&messages, &opts).await {
        let label = res.content.trim().to_uppercase();
        if let Some(intent) = Intent::from_label(&label) {
            return (intent, "llm");
        }
    }
    let normalized = user_request
        .trim()
        .trim_end_matches(['.', '!'])
        .to_lowercase();
    let request = if [
        "yes", "yep", "yeah", "ok", "okay", "sure", "proceed", "go ahead", "do it", "no", "cancel",
    ]
    .contains(&normalized.as_str())
    {
        context
            .iter()
            .rev()
            .find(|m| m.role == ChatRole::User)
            .map(|m| m.content.as_str())
            .unwrap_or(user_request)
    } else {
        user_request
    };
    (heuristic_intent(request), "heuristic")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_routes_common_requests() {
        assert_eq!(
            heuristic_intent("Fix the authentication bug in Co-Founder"),
            Intent::Coding
        );
        assert_eq!(
            heuristic_intent("Open GitHub and check notifications"),
            Intent::Browser
        );
        assert_eq!(heuristic_intent("Open Brave"), Intent::Browser);
        assert_eq!(
            heuristic_intent("Run the backend and tell me if it starts"),
            Intent::Terminal
        );
        assert_eq!(
            heuristic_intent("Create a file containing hello"),
            Intent::Filesystem
        );
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use crate::llm::LlmResponse;
    struct Offline;
    impl LlmProvider for Offline {
        async fn chat(&self, _: &[ChatMessage], _: &ChatOptions) -> anyhow::Result<LlmResponse> {
            anyhow::bail!("offline")
        }
        async fn stream(
            &self,
            _: &[ChatMessage],
            _: &ChatOptions,
            _: &mut (dyn FnMut(String) + Send),
        ) -> anyhow::Result<LlmResponse> {
            anyhow::bail!("offline")
        }
    }
    #[tokio::test]
    async fn confirmations_use_previous_task_in_fallback_but_new_requests_stand_on_their_own() {
        let context = vec![
            ChatMessage::user("Plan the theme with coding.startPlan"),
            ChatMessage {
                role: ChatRole::Assistant,
                content: "Shall I proceed?".into(),
                tool_calls: vec![],
                tool_call_id: None,
            },
        ];
        assert_eq!(
            classify_intent_with_context(&Offline, "yes", &context)
                .await
                .0,
            Intent::Coding
        );
        assert_eq!(
            classify_intent_with_context(&Offline, "proceed!", &context)
                .await
                .0,
            Intent::Coding
        );
        assert_eq!(
            classify_intent_with_context(&Offline, "no", &context)
                .await
                .0,
            Intent::Coding
        );
        assert_eq!(
            classify_intent_with_context(&Offline, "Open a website", &context)
                .await
                .0,
            Intent::Browser
        );
        assert_eq!(
            classify_intent_with_context(&Offline, "hello", &context)
                .await
                .0,
            Intent::Chat
        );
        assert_eq!(
            classify_intent_with_context(&Offline, "yes", &[]).await.0,
            Intent::Chat
        );
    }
}
