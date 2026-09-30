/** Central config loader. Backend only. Never logs secret values. */
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ComradeConfig {
    pub llm_provider: String,
    pub deepseek_key: String,
    pub openrouter_key: String,
    pub llm_model: String,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub profile_dir: String,
    pub max_steps: usize,
    pub timeout_ms: u64,
    // Service backend (Settings [server] or COMRADE_SERVER_* env).
    pub server_enabled: bool,
    pub server_url: String,
    pub server_key: String,
    pub server_llm_model: String,
    pub server_embedding_model: String,
    pub server_embedding_dim: usize,
    pub server_stt_model: String,
    pub server_tts_voice: String,
    pub voice_backend: String,
}

impl ComradeConfig {
    /// True when chat + embeddings should go to the service server.
    pub fn use_service(&self) -> bool {
        self.server_enabled && !self.server_url.trim().is_empty()
    }

    /// True when speech (STT/TTS) should go to the service server.
    /// Explicit `server` wins; `local` pins on-device even when a server
    /// is configured for the brain.
    pub fn use_server_voice(&self) -> bool {
        self.voice_backend == "server" && !self.server_url.trim().is_empty()
    }

    pub fn service_config(&self) -> crate::service::ServiceConfig {
        crate::service::ServiceConfig {
            base_url: self.server_url.clone(),
            api_key: self.server_key.clone(),
            llm_model: self.server_llm_model.clone(),
            embedding_model: self.server_embedding_model.clone(),
            embedding_dim: self.server_embedding_dim,
            stt_model: self.server_stt_model.clone(),
            tts_voice: self.server_tts_voice.clone(),
            timeout_secs: 120,
        }
    }
}

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
        return format!("{home}/{rest}");
    }
    p.to_string()
}

fn parse_dotenv(path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let idx = line.find('=').unwrap_or(0);
        let key = line[..idx].trim().to_string();
        let mut value = line[idx + 1..].trim().to_string();
        if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            value = value[1..value.len() - 1].to_string();
        }
        map.insert(key, value);
    }
    map
}

/// Find the project root: walk up from CWD looking for `.env`, else exe dir.
pub fn find_project_root() -> PathBuf {
    let mut dir: Option<PathBuf> = std::env::current_dir().ok();
    for _ in 0..5 {
        let Some(d) = dir.clone() else { break };
        if d.join(".env").exists() {
            return d;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn load_config(root: &Path) -> ComradeConfig {
    let file_vars = parse_dotenv(&root.join(".env"));
    let get = |k: &str| -> String {
        std::env::var(k).ok().or_else(|| file_vars.get(k).cloned()).unwrap_or_default()
    };
    // Non-secret knobs also live in comrade.conf for easy testing.
    // Precedence: ENV > .env > comrade.conf > builtin default.
    let conf = crate::prefs::load();
    let conf_or = |env_val: String, conf_val: &str, fallback: &str| -> String {
        if !env_val.is_empty() {
            env_val
        } else if !conf_val.trim().is_empty() {
            conf_val.to_string()
        } else {
            fallback.to_string()
        }
    };
    let provider = get("LLM_PROVIDER").to_lowercase();
    // Service backend: ENV/COMRADE_SERVER_* > .env > comrade.conf [server].
    // Enabled when explicitly turned on OR when a URL is supplied via env.
    let server_url = {
        let e = get("COMRADE_SERVER_URL");
        if !e.trim().is_empty() {
            e.trim().to_string()
        } else if !conf.server.base_url.trim().is_empty() {
            conf.server.base_url.trim().to_string()
        } else {
            crate::service::resolve_server_url("")
        }
    };
    let server_key = {
        let e = get("COMRADE_SERVER_KEY");
        if !e.is_empty() { e } else { conf.server.api_key.clone() }
    };
    let server_enabled = {
        let raw = get("COMRADE_SERVER_ENABLED");
        if !raw.is_empty() {
            matches!(raw.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on")
        } else if conf.server.enabled {
            true
        } else {
            // A URL supplied outside comrade.conf (process env or .env file)
            // counts as an explicit opt-in; a bare conf URL without the
            // checkbox does not (avoids surprise activation from stale conf).
            std::env::var("COMRADE_SERVER_URL").ok().filter(|u| !u.trim().is_empty()).is_some()
                || file_vars
                    .get("COMRADE_SERVER_URL")
                    .map(|u| !u.trim().is_empty())
                    .unwrap_or(false)
        }
    };    let provider = if !get("LLM_PROVIDER").is_empty() {
        if provider == "openrouter" { "openrouter".into() } else { "deepseek".into() }
    } else {
        conf.llm.provider.clone()
    };
    ComradeConfig {
        llm_provider: if provider == "openrouter" { "openrouter".into() } else { "deepseek".into() },
        deepseek_key: get("DEEPSEEK_API_KEY"),
        openrouter_key: get("OPENROUTER_API_KEY"),
        llm_model: conf_or(get("LLM_MODEL"), &conf.llm.model, "deepseek-flash"),
        embedding_model: conf_or(
            get("EMBEDDING_MODEL"),
            &conf.memory.embedding_model,
            "openai/text-embedding-3-small",
        ),
        embedding_dim: {
            let raw = get("EMBEDDING_DIM");
            if !raw.is_empty() {
                raw.parse().unwrap_or(conf.memory.embedding_dim)
            } else {
                conf.memory.embedding_dim
            }
        },
        profile_dir: expand_home(&{
            let m = get("COMRADE_PROFILE_DIR");
            if m.is_empty() {
                crate::paths::profile_dir_default().to_string_lossy().to_string()
            } else {
                m
            }
        }),
        max_steps: {
            let raw = get("AGENT_MAX_STEPS");
            if !raw.is_empty() {
                agent_max_steps(&raw)
            } else {
                conf.agent.max_steps
            }
        },
        timeout_ms: {
            let raw = get("AGENT_TIMEOUT_MS");
            if !raw.is_empty() {
                agent_timeout(&raw)
            } else {
                conf.agent.timeout_ms
            }
        },
        server_enabled,
        server_url,
        server_key,
        server_llm_model: crate::service::normalize_llm_model(&conf_or(
            get("COMRADE_SERVER_LLM_MODEL"),
            &conf.server.llm_model,
            "deepseek-flash",
        )),
        server_embedding_model: conf_or(
            get("COMRADE_SERVER_EMBEDDING_MODEL"),
            &conf.server.embedding_model,
            "comrade-embed",
        ),
        server_embedding_dim: {
            let raw = get("COMRADE_SERVER_EMBEDDING_DIM");
            if !raw.is_empty() {
                raw.parse().unwrap_or(conf.server.embedding_dim)
            } else {
                conf.server.embedding_dim
            }
        },
        server_stt_model: conf_or(
            get("COMRADE_SERVER_STT_MODEL"),
            &conf.server.stt_model,
            "comrade-stt",
        ),
        server_tts_voice: conf_or(
            get("COMRADE_SERVER_TTS_VOICE"),
            &conf.server.tts_voice,
            "default",
        ),
        voice_backend: {
            let raw = get("COMRADE_VOICE_BACKEND").trim().to_lowercase();
            if raw == "server" || raw == "local" {
                raw
            } else {
                let v = conf.voice.backend.trim().to_lowercase();
                if v == "server" { "server".into() } else { "local".into() }
            }
        },
    }
}

fn agent_max_steps(s: &str) -> usize {
    s.parse().unwrap_or(15)
}

fn agent_timeout(s: &str) -> u64 {
    s.parse().unwrap_or(120_000)
}

/// Key presence (names only — never values) for startup diagnostics.
pub fn presence(cfg: &ComradeConfig) -> serde_json::Value {
    serde_json::json!({
        "DEEPSEEK_API_KEY": !cfg.deepseek_key.is_empty(),
        "OPENROUTER_API_KEY": !cfg.openrouter_key.is_empty(),
        "COMRADE_SERVER_URL": !cfg.server_url.is_empty(),
        "COMRADE_SERVER_KEY": !cfg.server_key.is_empty(),
        "service_mode": cfg.use_service(),
        "server_voice": cfg.use_server_voice(),
    })
}
