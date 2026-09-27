/** Central config loader. Backend only. Never logs secret values. */
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ComradeConfig {
    pub llm_provider: String,
    pub deepseek_key: String,
    pub openrouter_key: String,
    pub llm_model: String,
    pub stt_model: String,
    pub tts_model: String,
    pub tts_voice: String,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub profile_dir: String,
    pub opencode_bin: String,
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
    let provider = get("LLM_PROVIDER").to_lowercase();
    ComradeConfig {
        llm_provider: if provider == "openrouter" { "openrouter".into() } else { "deepseek".into() },
        deepseek_key: get("DEEPSEEK_API_KEY"),
        openrouter_key: get("OPENROUTER_API_KEY"),
        llm_model: {
            let m = get("LLM_MODEL");
            if m.is_empty() { "deepseek-flash".into() } else { m }
        },
        stt_model: {
            let m = get("STT_MODEL");
            if m.is_empty() { "openai/whisper-large-v3".into() } else { m }
        },
        tts_model: {
            let m = get("TTS_MODEL");
            if m.is_empty() { "mistralai/voxtral-mini-tts-2603".into() } else { m }
        },
        tts_voice: {
            let m = get("TTS_VOICE");
            if m.is_empty() { "en_paul_neutral".into() } else { m }
        },
        embedding_model: {
            let m = get("EMBEDDING_MODEL");
            if m.is_empty() { "openai/text-embedding-3-small".into() } else { m }
        },
        embedding_dim: get("EMBEDDING_DIM").parse().unwrap_or(1536),
        profile_dir: expand_home(&{
            let m = get("COMRADE_PROFILE_DIR");
            if m.is_empty() {
                crate::paths::profile_dir_default().to_string_lossy().to_string()
            } else {
                m
            }
        }),
        opencode_bin: {
            let m = get("OPENCODE_BIN");
            if m.is_empty() { "opencode".into() } else { m }
        },
        max_steps: agent_max_steps(&get("AGENT_MAX_STEPS")),
        timeout_ms: agent_timeout(&get("AGENT_TIMEOUT_MS")),
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
