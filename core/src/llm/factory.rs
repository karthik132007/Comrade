/** Provider factory: DeepSeek-direct (default brain) vs OpenRouter from env. */
use crate::config::ComradeConfig;
use crate::llm::OpenRouterProvider;
use crate::service::{AnyEmbedder, AnyLlm, ServiceClient};

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

/// Service-aware backend: when a server is configured + enabled, chat and
/// embeddings go to the server; otherwise the direct cloud providers above.
pub fn create_backends(config: &ComradeConfig) -> (AnyLlm, AnyEmbedder) {
    if config.use_service() {
        let svc = ServiceClient::new(config.service_config());
        (AnyLlm::Service(svc.clone()), AnyEmbedder::Service(svc))
    } else {
        (
            AnyLlm::Direct(create_configured(config)),
            AnyEmbedder::Direct(crate::llm::OpenRouterEmbedder::new(
                embedding_key(config),
                config.embedding_model.clone(),
            )),
        )
    }
}

/// Effective embedding dim: server dim in service mode, else the direct one.
pub fn embedding_dim(config: &ComradeConfig) -> usize {
    if config.use_service() {
        config.server_embedding_dim
    } else {
        config.embedding_dim
    }
}
