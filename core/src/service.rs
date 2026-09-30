/**
 * Comrade service backend: one remote server for LLM, embeddings, STT, TTS.
 *
 * When `[server] enabled = true` (Settings UI, or COMRADE_SERVER_URL), the
 * desktop app stops calling DeepSeek/OpenRouter directly and talks to your
 * server instead. The server speaks OpenAI-compatible endpoints:
 *
 * ```text
 * GET  {base}/healthz  liveness
 * GET  {base}/readyz   200 when required keys present
 * GET  {base}/v1/models  routing table, no secrets
 * POST {base}/v1/chat/completions  {model, messages, tools?, stream} (SSE when stream)
 * POST {base}/v1/embeddings        {model, input} -> {data: [{embedding}]}
 * POST {base}/v1/audio/transcriptions  multipart file (wav) + optional model -> {text}
 * POST {base}/v1/audio/speech      {input, model?, voice?} -> audio bytes (wav requested)
 * ```
 *
 * Local models (sherpa-onnx STT/VAD/TTS) stay available as an optional
 * fallback: set `[voice] backend = local` to keep everything on-device, or
 * `server` to use the server for speech. VAD endpointing can run on a tiny
 * energy detector (`EnergyVad` in voice/vad.rs) so server voice mode needs
 * zero local model downloads.
 */
use futures_util::StreamExt;
use serde_json::json;

use crate::llm::{ChatMessage, ChatOptions, Embedder, LlmProvider, LlmResponse, ToolCallRequest};
use crate::voice::tts::TtsPcm;

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub base_url: String,
    pub api_key: String,
    pub llm_model: String,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub stt_model: String,
    pub tts_voice: String,
    pub timeout_secs: u64,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            llm_model: SERVICE_DEFAULT_LLM_MODEL.into(),
            embedding_model: "comrade-embed".into(),
            embedding_dim: 1536,
            stt_model: "comrade-stt".into(),
            tts_voice: "default".into(),
            timeout_secs: 120,
        }
    }
}

/// The only chat models the app may send, exactly as listed. The server
/// routes these ids; anything else falls back to the default so a stale
/// conf can never send an id the server doesn't know.
pub const SERVICE_LLM_MODELS: &[&str] = &[
    "nvidia/nemotron-3.5-lightning",
    "deepseek-flash",
    "openai/gpt-oss-120b",
    "google/gemma-4-31b-it",
];

pub const SERVICE_DEFAULT_LLM_MODEL: &str = "deepseek-flash";

/// Built-in service backend. Used whenever service mode is on but no URL
/// was configured via env / .env / conf. Code-only: never rendered into
/// comrade.conf, never sent to the frontend.
pub const COMRADE_DEFAULT_SERVER_URL: &str = "https://comradeserver.vercel.app/";

/// Explicit URL wins; otherwise the built-in backend.
pub fn resolve_server_url(configured: &str) -> String {
    let t = configured.trim();
    if t.is_empty() {
        COMRADE_DEFAULT_SERVER_URL.to_string()
    } else {
        t.to_string()
    }
}

/// Exact-match normalization: returns `s` verbatim when it is one of the
/// allowed ids, else the default. Never alters a valid id (no case
/// folding, no trimming inside the id).
pub fn normalize_llm_model(s: &str) -> String {
    let s = s.trim();
    if SERVICE_LLM_MODELS.contains(&s) {
        s.to_string()
    } else {
        SERVICE_DEFAULT_LLM_MODEL.to_string()
    }
}

impl ServiceConfig {
    pub fn enabled(&self) -> bool {
        !self.base_url.trim().is_empty()
    }
    fn base(&self) -> String {
        self.base_url.trim_end_matches('/').to_string()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base(), path)
    }
}

#[derive(Clone)]
pub struct ServiceClient {
    cfg: ServiceConfig,
    client: reqwest::Client,
}

impl ServiceClient {
    pub fn new(cfg: ServiceConfig) -> Self {
        Self { cfg, client: reqwest::Client::new() }
    }

    pub fn config(&self) -> &ServiceConfig {
        &self.cfg
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.cfg.api_key.trim().is_empty() {
            req
        } else {
            req.header("Authorization", format!("Bearer {}", self.cfg.api_key.trim()))
        }
    }

    fn wire_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
        messages
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
                            "function": { "name": t.name.replace('.', "_"), "arguments": t.arguments.to_string() },
                        }))
                        .collect::<Vec<_>>());
                }
                if let Some(id) = &m.tool_call_id {
                    obj["tool_call_id"] = json!(id);
                }
                obj
            })
            .collect()
    }

    fn local_name(wire: &str) -> String {
        match wire.find('_') {
            Some(i) => format!("{}.{}", &wire[..i], &wire[i + 1..]),
            None => wire.to_string(),
        }
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
            let args_str = func.and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("{}");
            let arguments: serde_json::Value =
                serde_json::from_str(args_str).unwrap_or(json!({ "_raw": args_str }));
            out.push(ToolCallRequest { id, name: Self::local_name(wire_name), arguments });
        }
        out
    }

    fn chat_body(&self, messages: &[ChatMessage], opts: &ChatOptions, stream: bool) -> serde_json::Value {
        let mut body = json!({
            "model": opts.model.clone().unwrap_or_else(|| self.cfg.llm_model.clone()),
            "messages": Self::wire_messages(messages),
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
                        "name": t.name.replace('.', "_"),
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                }))
                .collect::<Vec<_>>());
        }
        body
    }

    async fn chat_once(&self, messages: &[ChatMessage], opts: &ChatOptions) -> anyhow::Result<LlmResponse> {
        if !self.cfg.enabled() {
            anyhow::bail!("SERVICE_UNAVAILABLE: server base_url is not configured (Settings → Service).");
        }
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(self.cfg.timeout_secs),
            self.auth(self.client.post(self.cfg.url("/v1/chat/completions")))
                .json(&self.chat_body(messages, opts, false))
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SERVICE_TIMEOUT: chat request timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(300).collect();
            anyhow::bail!("SERVICE_CHAT_FAILED ({status}): {snippet}");
        }
        let body: serde_json::Value = res.json().await?;
        if let Some(msg) = body.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
            anyhow::bail!("SERVICE_ERROR: {msg}");
        }
        let msg = &body["choices"][0]["message"];
        Ok(LlmResponse {
            content: msg.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string(),
            tool_calls: Self::parse_tool_calls(msg.get("tool_calls")),
        })
    }

    async fn stream_once(
        &self,
        messages: &[ChatMessage],
        opts: &ChatOptions,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> anyhow::Result<LlmResponse> {
        if !self.cfg.enabled() {
            anyhow::bail!("SERVICE_UNAVAILABLE: server base_url is not configured (Settings → Service).");
        }
        let res = self
            .auth(self.client.post(self.cfg.url("/v1/chat/completions")))
            .json(&self.chat_body(messages, opts, true))
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(300).collect();
            anyhow::bail!("SERVICE_STREAM_FAILED ({status}): {snippet}");
        }
        let mut buffer = String::new();
        let mut content = String::new();
        let mut calls: Vec<(String, String, String)> = Vec::new();
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            // Normalize CRLF framings so "\n\n" event splitting always works.
            buffer.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
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

    /// Liveness probe for the Settings "Test" button. Tries the server's
    /// `GET /healthz` (liveness), then `/readyz` (keys present), then
    /// `/v1/models` (routing table).
    pub async fn health(&self) -> anyhow::Result<String> {
        if !self.cfg.enabled() {
            anyhow::bail!("SERVICE_UNAVAILABLE: set the server URL first.");
        }
        for path in ["/healthz", "/readyz", "/v1/models"] {
            let res = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.auth(self.client.get(self.cfg.url(path))).send(),
            )
            .await;
            if let Ok(Ok(res)) = res {
                if res.status().is_success() {
                    let text = res.text().await.unwrap_or_default();
                    let snippet: String = text.chars().take(200).collect();
                    return Ok(format!("OK {path}: {snippet}"));
                }
            }
        }
        anyhow::bail!("SERVICE_UNREACHABLE: no response at /healthz, /readyz, or /v1/models.")
    }

    /// Routing table (`GET /v1/models`, no secrets). Used by Settings to
    /// show which models the server exposes.
    pub async fn models(&self) -> anyhow::Result<serde_json::Value> {
        if !self.cfg.enabled() {
            anyhow::bail!("SERVICE_UNAVAILABLE: set the server URL first.");
        }
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.auth(self.client.get(self.cfg.url("/v1/models"))).send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SERVICE_TIMEOUT: /v1/models timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            anyhow::bail!("SERVICE_MODELS_FAILED ({status}).");
        }
        Ok(res.json().await?)
    }

    async fn embed_once(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if !self.cfg.enabled() {
            anyhow::bail!("EMBED_UNAVAILABLE: server base_url is not configured (Settings → Service).");
        }
        let non_empty: Vec<&String> = texts.iter().filter(|t| !t.trim().is_empty()).collect();
        if non_empty.is_empty() {
            anyhow::bail!("EMBED_EMPTY: nothing to embed.");
        }
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.auth(self.client.post(self.cfg.url("/v1/embeddings")))
                .json(&json!({ "model": self.cfg.embedding_model, "input": non_empty }))
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("EMBED_FAILED: request timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(200).collect();
            anyhow::bail!("EMBED_FAILED ({status}): {snippet}");
        }
        let body: serde_json::Value = res.json().await?;
        let mut data = body.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
        data.sort_by_key(|d| d.get("index").and_then(|i| i.as_u64()).unwrap_or(0));
        let mut out = Vec::with_capacity(non_empty.len());
        for item in data {
            let vec: Vec<f32> = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect())
                .unwrap_or_default();
            if vec.is_empty() {
                anyhow::bail!("EMBED_FAILED: empty vector in response.");
            }
            out.push(vec);
        }
        if out.len() != non_empty.len() {
            anyhow::bail!(
                "EMBED_FAILED: got {} vectors for {} texts.",
                out.len(),
                non_empty.len()
            );
        }
        Ok(out)
    }

    /// Transcribe 16 kHz mono PCM via the server (OpenAI-style multipart
    /// upload: `file` wav bytes + optional `model`). Returns plain text.
    pub async fn transcribe(&self, pcm_16k_mono: &[f32], language: &str) -> anyhow::Result<String> {
        if !self.cfg.enabled() {
            anyhow::bail!("STT_UNAVAILABLE: server base_url is not configured.");
        }
        if pcm_16k_mono.is_empty() {
            anyhow::bail!("STT_EMPTY: no audio to transcribe.");
        }
        let wav = encode_wav(pcm_16k_mono, 16000);
        let file_part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| anyhow::anyhow!("STT_FAILED: bad mime ({e})."))?;
        let mut form =
            reqwest::multipart::Form::new().part("file", file_part).text("model", self.cfg.stt_model.clone());
        if !language.trim().is_empty() {
            form = form.text("language", language.to_string());
        }
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.auth(self.client.post(self.cfg.url("/v1/audio/transcriptions"))).multipart(form).send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("STT_FAILED: request timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(200).collect();
            anyhow::bail!("STT_FAILED ({status}): {snippet}");
        }
        let body: serde_json::Value = res.json().await?;
        body.get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("STT_FAILED: empty transcript in response."))
    }

    /// Synthesize one sentence chunk via the server (OpenAI-style JSON
    /// `{input, model?, voice?}` → raw audio bytes). Requests wav so the
    /// payload decodes without extra codecs; still accepts a JSON
    /// `{audio_base64, sample_rate}` envelope if the server sends one.
    pub async fn synthesize(&self, text: &str, voice: &str, speed: f32) -> anyhow::Result<TtsPcm> {
        use base64::Engine as _;
        if !self.cfg.enabled() {
            anyhow::bail!("TTS_UNAVAILABLE: server base_url is not configured.");
        }
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("nothing to synthesize");
        }
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.auth(self.client.post(self.cfg.url("/v1/audio/speech"))).json(&json!({
                "model": "comrade-tts",
                "input": text,
                "voice": voice,
                "speed": speed,
                "response_format": "wav",
            })).send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("TTS_FAILED: request timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            let snippet: String = res.text().await.unwrap_or_default().chars().take(200).collect();
            anyhow::bail!("TTS_FAILED ({status}): {snippet}");
        }
        let content_type = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = res.bytes().await?.to_vec();
        if content_type.contains("json") || looks_like_json(&bytes) {
            let body: serde_json::Value = serde_json::from_slice(&bytes)?;
            let b64 = body
                .get("audio_base64")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("TTS_FAILED: no audio_base64 in response."))?;
            let wav = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| anyhow::anyhow!("TTS_FAILED: bad audio_base64 ({e})."))?;
            let rate = body.get("sample_rate").and_then(|v| v.as_u64()).unwrap_or(24000) as u32;
            return decode_audio_bytes(&wav, rate);
        }
        decode_audio_bytes(&bytes, 24000)
    }
}

fn looks_like_json(bytes: &[u8]) -> bool {
    bytes.first().map(|b| *b == b'{').unwrap_or(false)
}

/// WAV (16-bit PCM) bytes -> f32 samples. Falls back to raw s16le/f32le.
fn decode_audio_bytes(bytes: &[u8], default_rate: u32) -> anyhow::Result<TtsPcm> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" {
        let cursor = std::io::Cursor::new(bytes);
        let mut reader = hound::WavReader::new(cursor)?;
        let spec = reader.spec();
        let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, _) => reader.samples::<f32>().flatten().collect(),
            _ => reader.samples::<i16>().map(|s| s.unwrap_or(0) as f32 / 32768.0).collect(),
        };
        if samples.is_empty() {
            anyhow::bail!("TTS_FAILED: server returned no samples.");
        }
        return Ok(TtsPcm { samples, sample_rate: spec.sample_rate });
    }
    if bytes.len() >= 2 {
        let (chunks, _) = bytes.as_chunks::<2>();
        let s16: Vec<f32> = chunks
            .iter()
            .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
            .collect();
        if !s16.is_empty() {
            return Ok(TtsPcm { samples: s16, sample_rate: default_rate });
        }
    }
    anyhow::bail!("TTS_FAILED: unrecognized audio payload.")
}

/// f32 mono PCM -> 16-bit WAV bytes (for STT upload).
pub fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    if let Ok(mut w) = hound::WavWriter::new(&mut out, spec) {
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            let _ = w.write_sample(v);
        }
        let _ = w.finalize();
    }
    out.into_inner()
}

impl LlmProvider for ServiceClient {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        opts: &ChatOptions,
    ) -> anyhow::Result<LlmResponse> {
        self.chat_once(messages, opts).await
    }

    async fn stream(
        &self,
        messages: &[ChatMessage],
        opts: &ChatOptions,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> anyhow::Result<LlmResponse> {
        match self.stream_once(messages, opts, on_token).await {
            Ok(r) => Ok(r),
            Err(_) => {
                let r = self.chat_once(messages, opts).await?;
                // The UI renders streamed tokens; with no SSE flow it would
                // stay blank, so replay the full text as one token.
                if !r.content.is_empty() {
                    on_token(r.content.clone());
                }
                Ok(r)
            }
        }
    }
}

impl Embedder for ServiceClient {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        self.embed_once(texts).await
    }
}

/// Backend chosen at startup: direct cloud APIs (DeepSeek/OpenRouter) or
/// the Comrade service server. One enum so `Agent<P, E>` stays generic.
pub enum AnyLlm {
    Direct(crate::llm::OpenRouterProvider),
    Service(ServiceClient),
}

impl LlmProvider for AnyLlm {
    async fn chat(&self, m: &[ChatMessage], o: &ChatOptions) -> anyhow::Result<LlmResponse> {
        match self {
            AnyLlm::Direct(p) => p.chat(m, o).await,
            AnyLlm::Service(p) => p.chat(m, o).await,
        }
    }

    async fn stream(
        &self,
        m: &[ChatMessage],
        o: &ChatOptions,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> anyhow::Result<LlmResponse> {
        match self {
            AnyLlm::Direct(p) => p.stream(m, o, on_token).await,
            AnyLlm::Service(p) => p.stream(m, o, on_token).await,
        }
    }
}

pub enum AnyEmbedder {
    Direct(crate::llm::OpenRouterEmbedder),
    Service(ServiceClient),
}

impl Embedder for AnyEmbedder {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        match self {
            AnyEmbedder::Direct(e) => e.embed(texts).await,
            AnyEmbedder::Service(e) => e.embed(texts).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_without_url() {
        let c = ServiceConfig::default();
        assert!(!c.enabled());
        let c = ServiceConfig { base_url: "http://localhost:8000/".into(), ..ServiceConfig::default() };
        assert!(c.enabled());
        assert_eq!(c.url("/v1/models"), "http://localhost:8000/v1/models");
    }

    #[test]
    fn tool_names_round_trip() {
        assert_eq!(ServiceClient::local_name("browser_open"), "browser.open");
        assert_eq!(ServiceClient::local_name("plain"), "plain");
    }

    #[test]
    fn llm_model_allowlist_sends_exact_ids() {
        for id in SERVICE_LLM_MODELS {
            assert_eq!(normalize_llm_model(id), *id);
        }
        assert_eq!(normalize_llm_model("openai/gpt-oss-120b"), "openai/gpt-oss-120b");
        assert_eq!(normalize_llm_model("  deepseek-flash  "), "deepseek-flash");
        assert_eq!(normalize_llm_model("my-custom-llm"), SERVICE_DEFAULT_LLM_MODEL);
        assert_eq!(normalize_llm_model(""), SERVICE_DEFAULT_LLM_MODEL);
    }

    #[test]
    fn wav_round_trip() {        let pcm = vec![0.0f32, 0.5, -0.5, 1.0];
        let wav = encode_wav(&pcm, 16000);
        let back = decode_audio_bytes(&wav, 16000).unwrap();
        assert_eq!(back.sample_rate, 16000);
        assert_eq!(back.samples.len(), pcm.len());
        assert!((back.samples[1] - 0.5).abs() < 0.001);
    }
}
