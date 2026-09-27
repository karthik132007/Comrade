/**
 * Text embeddings for vector memory. DeepSeek offers no embedding API, so
 * embeddings ride on OpenRouter (OpenAI-compatible /embeddings endpoint).
 * Used ONLY for memory vectors — the reasoning brain stays DeepSeek-direct.
 */
use std::time::Duration;

const EMBEDDINGS_URL: &str = "https://openrouter.ai/api/v1/embeddings";
const BATCH_SIZE: usize = 32;

#[allow(async_fn_in_trait)]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
}

pub struct OpenRouterEmbedder {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenRouterEmbedder {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model, client: reqwest::Client::new() }
    }
}

impl Embedder for OpenRouterEmbedder {
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if self.api_key.is_empty() {
            anyhow::bail!("EMBED_UNAVAILABLE: OPENROUTER_API_KEY is not configured.");
        }
        let non_empty: Vec<&String> = texts.iter().filter(|t| !t.trim().is_empty()).collect();
        if non_empty.is_empty() {
            anyhow::bail!("EMBED_EMPTY: nothing to embed.");
        }
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in non_empty.chunks(BATCH_SIZE) {
            let res = tokio::time::timeout(
                Duration::from_secs(60),
                self.client
                    .post(EMBEDDINGS_URL)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("HTTP-Referer", "https://github.com/comrade-desktop")
                    .header("X-Title", "Comrade")
                    .json(&serde_json::json!({ "model": self.model, "input": chunk }))
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
            let json: serde_json::Value = res.json().await?;
            let mut data = json
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            // Providers don't guarantee order — sort by index when present.
            data.sort_by_key(|d| d.get("index").and_then(|i| i.as_u64()).unwrap_or(0));
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
}
