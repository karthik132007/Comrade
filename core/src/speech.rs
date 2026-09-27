/**
 * Voice layer: OpenRouter-backed STT/TTS + local fallback.
 * Flow: Microphone → STT → Comrade Core → LLM/Tools → TTS → Speaker.
 */
use std::time::Duration;

const STT_URL: &str = "https://openrouter.ai/api/v1/audio/transcriptions";
const TTS_URL: &str = "https://openrouter.ai/api/v1/audio/speech";
const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;
const MAX_TTS_CHARS: usize = 1500;

#[allow(async_fn_in_trait)]
pub trait SpeechToText: Send + Sync {
    /// Transcribe raw audio bytes. `format` is the container (e.g. `webm` from MediaRecorder).
    async fn transcribe(
        &self,
        audio: &[u8],
        language: Option<&str>,
        format: &str,
    ) -> anyhow::Result<String>;
}

#[derive(Debug)]
pub struct SynthesizedSpeech {
    pub audio: Vec<u8>,
    pub mime_type: String,
}

#[allow(async_fn_in_trait)]
pub trait TextToSpeech: Send + Sync {
    async fn synthesize(&self, text: &str, voice: Option<&str>) -> anyhow::Result<SynthesizedSpeech>;
}

pub struct OpenRouterStt {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenRouterStt {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model, client: reqwest::Client::new() }
    }
}

impl SpeechToText for OpenRouterStt {
    async fn transcribe(
        &self,
        audio: &[u8],
        language: Option<&str>,
        format: &str,
    ) -> anyhow::Result<String> {
        if self.api_key.is_empty() {
            anyhow::bail!("STT_UNAVAILABLE: OPENROUTER_API_KEY is not configured.");
        }
        if audio.is_empty() {
            anyhow::bail!("STT_EMPTY_AUDIO: no audio captured.");
        }
        if audio.len() > MAX_AUDIO_BYTES {
            anyhow::bail!("STT_TOO_LARGE: {:.1}MB exceeds 25MB.", audio.len() as f64 / 1_048_576.0);
        }
        let mut body = serde_json::json!({
            "model": self.model,
            "input_audio": {
                "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, audio),
                "format": format,
            },
        });
        if let Some(lang) = language {
            body["language"] = serde_json::Value::String(lang.to_string());
        }
        let res = tokio::time::timeout(
            Duration::from_secs(60),
            self.client
                .post(STT_URL)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("HTTP-Referer", "https://github.com/comrade-desktop")
                .header("X-Title", "Comrade")
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("STT_FAILED: request timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(200).collect();
            anyhow::bail!("STT_FAILED ({status}): {snippet}");
        }
        let json: serde_json::Value = res.json().await?;
        if let Some(msg) = json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
            anyhow::bail!("STT_FAILED: {msg}");
        }
        Ok(json
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string())
    }
}

pub struct OpenRouterTts {
    api_key: String,
    model: String,
    voice: String,
    client: reqwest::Client,
}

impl OpenRouterTts {
    pub fn new(api_key: String, model: String, voice: String) -> Self {
        Self { api_key, model, voice, client: reqwest::Client::new() }
    }
}

impl TextToSpeech for OpenRouterTts {
    async fn synthesize(&self, text: &str, voice: Option<&str>) -> anyhow::Result<SynthesizedSpeech> {
        if self.api_key.is_empty() {
            anyhow::bail!("TTS_UNAVAILABLE: OPENROUTER_API_KEY is not configured.");
        }
        let input: String = text.trim().chars().take(MAX_TTS_CHARS).collect();
        if input.is_empty() {
            anyhow::bail!("TTS_EMPTY_TEXT: nothing to speak.");
        }
        let res = tokio::time::timeout(
            Duration::from_secs(60),
            self.client
                .post(TTS_URL)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("HTTP-Referer", "https://github.com/comrade-desktop")
                .header("X-Title", "Comrade")
                .json(&serde_json::json!({
                    "model": self.model,
                    "input": input,
                    "voice": voice.unwrap_or(&self.voice),
                    "response_format": "mp3",
                }))
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("TTS_FAILED: request timed out."))??;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(200).collect();
            anyhow::bail!("TTS_FAILED ({status}): {snippet}");
        }
        // Success returns raw audio; failures return JSON — validate first.
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !content_type.starts_with("audio/") {
            let text = res.text().await.unwrap_or_default();
            let snippet: String = text.chars().take(200).collect();
            anyhow::bail!("TTS_BAD_RESPONSE ({content_type}): {snippet}");
        }
        let bytes = res.bytes().await?.to_vec();
        Ok(SynthesizedSpeech { audio: bytes, mime_type: content_type })
    }
}

/** Offline fallback: espeak-ng renders wav locally. Robotic, but always available. */
pub struct LocalTts;

impl TextToSpeech for LocalTts {
    async fn synthesize(&self, text: &str, _voice: Option<&str>) -> anyhow::Result<SynthesizedSpeech> {
        let input: String = text.trim().chars().take(MAX_TTS_CHARS).collect();
        if input.is_empty() {
            anyhow::bail!("TTS_EMPTY_TEXT: nothing to speak.");
        }
        let out = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new("espeak-ng").arg("--stdout").arg(&input).output(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("LOCAL_TTS_FAILED: espeak-ng unavailable ({e})"))?
        .map_err(|e| anyhow::anyhow!("LOCAL_TTS_FAILED: espeak-ng unavailable ({e})"))?;
        if !out.status.success() || out.stdout.is_empty() {
            anyhow::bail!("LOCAL_TTS_FAILED: espeak-ng produced no audio.");
        }
        Ok(SynthesizedSpeech { audio: out.stdout, mime_type: "audio/wav".into() })
    }
}

/** Primary TTS with automatic fallback to local speech on failure. */
pub struct FallbackTts {
    primary: OpenRouterTts,
    on_fallback: Option<Box<dyn Fn(String) + Send + Sync>>,
}

impl FallbackTts {
    pub fn new(primary: OpenRouterTts, on_fallback: Option<Box<dyn Fn(String) + Send + Sync>>) -> Self {
        Self { primary, on_fallback }
    }
}

impl TextToSpeech for FallbackTts {
    async fn synthesize(&self, text: &str, voice: Option<&str>) -> anyhow::Result<SynthesizedSpeech> {
        match self.primary.synthesize(text, voice).await {
            Ok(speech) => Ok(speech),
            Err(err) => {
                if let Some(cb) = &self.on_fallback {
                    let msg: String = format!("{err}").chars().take(200).collect();
                    cb(msg);
                }
                LocalTts.synthesize(text, voice).await
            }
        }
    }
}
