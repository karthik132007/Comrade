/**
 * OpenAI-compatible chat provider. Backs both OpenRouter and DeepSeek —
 * same wire format, different base URL + key. DeepSeek is the default brain;
 * OpenRouter remains for STT/TTS/embeddings (DeepSeek has no audio APIs).
 */
use futures_util::StreamExt;
use serde_json::json;

use super::types::{ChatMessage, ChatOptions, ChatRole, LlmProvider, LlmResponse, ToolCallRequest};

pub const OPENROUTER_CHAT_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
pub const DEEPSEEK_CHAT_URL: &str = "https://api.deepseek.com/chat/completions";

pub struct OpenRouterProvider {
    api_key: String,
    default_model: String,
    api_url: String,
    client: reqwest::Client,
}

impl OpenRouterProvider {
    pub fn new(api_key: String, default_model: String) -> Self {
        Self {
            api_key,
            default_model,
            api_url: OPENROUTER_CHAT_URL.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// DeepSeek-direct reasoning endpoint (OpenAI-compatible).
    pub fn deepseek(api_key: String, default_model: String) -> Self {
        Self {
            api_key,
            default_model,
            api_url: DEEPSEEK_CHAT_URL.to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn ensure_key(&self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            anyhow::bail!("OPENROUTER_API_KEY is not configured.");
        }
        Ok(())
    }

    fn body(&self, messages: &[ChatMessage], opts: &ChatOptions, stream: bool) -> serde_json::Value {
        let wire: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let mut obj = json!({ "role": m.role.as_str(), "content": m.content });
                if !m.tool_calls.is_empty() {
                    obj["tool_calls"] = json!(m
                        .tool_calls
                        .iter()
                        .map(|t| json!({
                            "id": t.id,
                            "type": "function",
                            "function": { "name": Self::wire_name(&t.name), "arguments": t.arguments.to_string() },
                        }))
                        .collect::<Vec<_>>());
                }
                if let Some(id) = &m.tool_call_id {
                    obj["tool_call_id"] = json!(id);
                }
                obj
            })
            .collect();
        let mut body = json!({
            "model": opts.model.clone().unwrap_or_else(|| self.default_model.clone()),
            "messages": wire,
            "max_tokens": opts.max_tokens,
            "temperature": opts.temperature,
            "stream": stream,
        });
        if !opts.tools.is_empty() {
            body["tools"] = json!(opts
                .tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": {
                        "name": Self::wire_name(&t.name),
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                }))
                .collect::<Vec<_>>());
        }
        body
    }

    fn parse_tool_calls(raw: Option<&serde_json::Value>) -> Vec<ToolCallRequest> {
        let mut out = Vec::new();
        let Some(arr) = raw.and_then(|v| v.as_array()) else {
            return out;
        };
        for tc in arr {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let func = tc.get("function");
            let wire_name = func.and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("");
            let name = Self::local_name(wire_name);
            let args_str = func.and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("{}");
            let arguments: serde_json::Value =
                serde_json::from_str(args_str).unwrap_or(json!({ "_raw": args_str }));
            out.push(ToolCallRequest { id, name, arguments });
        }
        out
    }

    /// DeepSeek (strict OpenAI validation) rejects dots in function names, so
    /// `namespace.tool` goes on the wire as `namespace_tool`. All Comrade tool
    /// names follow that shape with no underscores, so this round-trips exactly.
    fn wire_name(local: &str) -> String {
        local.replace('.', "_")
    }

    fn local_name(wire: &str) -> String {
        match wire.find('_') {
            Some(i) => format!("{}.{}", &wire[..i], &wire[i + 1..]),
            None => wire.to_string(),
        }
    }
}

impl LlmProvider for OpenRouterProvider {
    async fn chat(&self, messages: &[ChatMessage], opts: &ChatOptions) -> anyhow::Result<LlmResponse> {
        self.ensure_key()?;
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            self.client
                .post(&self.api_url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("HTTP-Referer", "https://github.com/comrade-desktop")
                .header("X-Title", "Comrade")
                .json(&self.body(messages, opts, false))
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("OpenRouter chat timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(300).collect();
            anyhow::bail!("OpenRouter chat failed ({status}): {snippet}");
        }
        let json: serde_json::Value = res.json().await?;
        if let Some(msg) = json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
            anyhow::bail!("OpenRouter error: {msg}");
        }
        let msg = &json["choices"][0]["message"];
        Ok(LlmResponse {
            content: msg.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string(),
            tool_calls: Self::parse_tool_calls(msg.get("tool_calls")),
        })
    }

    async fn stream(
        &self,
        messages: &[ChatMessage],
        opts: &ChatOptions,
        on_token: &mut dyn FnMut(String),
    ) -> anyhow::Result<LlmResponse> {
        self.ensure_key()?;
        let res = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://github.com/comrade-desktop")
            .header("X-Title", "Comrade")
            .json(&self.body(messages, opts, true))
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(300).collect();
            anyhow::bail!("OpenRouter stream failed ({status}): {snippet}");
        }
        let mut buffer = String::new();
        let mut content = String::new();
        // id -> (name, arguments-so-far); chunks after the first may omit id.
        let mut calls: Vec<(String, String, String)> = Vec::new();
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buffer.find("\n\n") {
                let event: String = buffer.drain(..pos + 2).collect();
                for line in event.lines() {
                    let line = line.trim();
                    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                        continue;
                    };
                    if data == "[DONE]" {
                        continue;
                    }
                    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
                        continue;
                    };
                    let delta = &parsed["choices"][0]["delta"];
                    if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
                        content.push_str(text);
                        on_token(text.to_string());
                    }
                    if let Some(arr) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in arr {
                            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let args = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let entry = if id.is_empty() {
                                calls.last_mut()
                            } else if let Some(e) = calls.iter_mut().find(|(i, _, _)| i == id) {
                                Some(e)
                            } else {
                                calls.push((id.to_string(), String::new(), String::new()));
                                calls.last_mut()
                            };
                            if let Some((_, n, a)) = entry {
                                if !name.is_empty() {
                                    *n = name.to_string();
                                }
                                a.push_str(args);
                            }
                        }
                    }
                }
            }
        }
        let tool_calls = calls
            .into_iter()
            .map(|(id, name, args)| ToolCallRequest {
                id,
                name: Self::local_name(&name),
                arguments: serde_json::from_str(&args).unwrap_or(json!({ "_raw": args })),
            })
            .collect();
        Ok(LlmResponse { content, tool_calls })
    }
}

/// Provider factory: DeepSeek-direct vs OpenRouter from env config.
pub fn create_provider(api_key: String, model: String) -> OpenRouterProvider {
    OpenRouterProvider::new(api_key, model)
}

#[allow(dead_code)]
fn role_name(role: ChatRole) -> &'static str {
    role.as_str()
}

#[cfg(test)]
mod tests {
    use super::OpenRouterProvider;

    #[test]
    fn tool_names_round_trip_for_strict_providers() {
        for local in [
            "terminal.execute",
            "filesystem.list",
            "computer.openApplication",
            "browser.getPageText",
            "opencode.executeTask",
        ] {
            let wire = OpenRouterProvider::wire_name(local);
            assert!(
                wire.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "wire name invalid: {wire}"
            );
            assert_eq!(OpenRouterProvider::local_name(&wire), local);
        }
    }
}
