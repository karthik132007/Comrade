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
    let provider = if !get("LLM_PROVIDER").is_empty() {
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
    })
}
