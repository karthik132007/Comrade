/** Shared LLM abstractions. Backend only — keys never leave this process. */
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ToolCallRequest>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: ChatRole::User, content: content.into(), tool_call_id: None, tool_calls: vec![] }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: ChatRole::System, content: content.into(), tool_call_id: None, tool_calls: vec![] }
    }
}

#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct ChatOptions {
    pub model: Option<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub tools: Vec<ToolDefinition>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self { model: None, max_tokens: 2048, temperature: 0.3, tools: vec![] }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallRequest>,
}

#[allow(async_fn_in_trait)]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, messages: &[ChatMessage], opts: &ChatOptions) -> anyhow::Result<LlmResponse>;
    async fn stream(
        &self,
        messages: &[ChatMessage],
        opts: &ChatOptions,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> anyhow::Result<LlmResponse>;
}
