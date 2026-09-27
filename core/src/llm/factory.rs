/** Provider factory: DeepSeek-direct (default brain) vs OpenRouter from env. */
use crate::config::ComradeConfig;
use crate::llm::OpenRouterProvider;

pub fn create_configured(config: &ComradeConfig) -> OpenRouterProvider {
    if config.llm_provider == "openrouter" {
        OpenRouterProvider::new(config.openrouter_key.clone(), config.llm_model.clone())
    } else {
        OpenRouterProvider::deepseek(config.deepseek_key.clone(), config.llm_model.clone())
    }
}

/// Embedding key: OpenRouter (DeepSeek has no embedding API).
pub fn embedding_key(config: &ComradeConfig) -> String {
    config.openrouter_key.clone()
}
