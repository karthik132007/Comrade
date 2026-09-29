/**
 * User preferences in `comrade.conf` (INI-lite) inside the comrade-agent home.
 * Non-secret settings only — API keys stay in `.env`.
 *
 * ```ini
 * [browser]
 * auto_show = true   # open the in-app browser pane when a browser tool runs
 * width_pct = 45     # pane width (20-70% of the main area)
 *
 * [voice]
 * autoplay = true
 * enabled = true
 * mic =
 *
 * [voice.stt]
 * engine = sherpa-onnx
 * model = zipformer-en-20M-int8
 * language = en
 * sample_rate = 16000
 *
 * [voice.vad]
 * threshold = 0.5
 * silence_ms = 700
 * min_speech_ms = 250
 *
 * [voice.tts]
 * engine = kokoro
 * voice = 0
 * speed = 1.0
 *
 * [voice.runtime]
 * max_utterance_ms = 30000
 * decode_every_frames = 16
 * chunk_max_chars = 220
 * chunk_min_merge = 12
 * num_threads = 2
 *
 * [llm]
 * provider = deepseek     # deepseek | openrouter (.env/ENV wins)
 * model = deepseek-flash  # (.env LLM_MODEL wins)
 *
 * [agent]
 * max_steps = 15          # (ENV AGENT_MAX_STEPS wins)
 * timeout_ms = 120000     # (ENV AGENT_TIMEOUT_MS wins)
 *
 * [memory]
 * embedding_model = openai/text-embedding-3-small  # (ENV wins)
 * embedding_dim = 1536                             # (ENV wins)
 *
 * [coding]
 * agents = opencode, claude, copilot
 * default = opencode
 * ```
 *
 * Browser note: Comrade ships its own dedicated Chromium under
 * `comrade-agent/browser/` (isolated `browser-profile/`). There is no
 * browser picker anymore — old `exe/kind/headless/profile/debug_port`
 * keys are ignored on read and never written. The bundled browser always
 * runs headless; the only visible surface is the resizable in-app pane.
 */
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONF_NAME: &str = "comrade.conf";

/// In-app browser pane preferences. Comrade owns one dedicated Chromium
/// (`comrade-agent/browser/`, isolated `browser-profile/`); these prefs only
/// control the embedded pane, never which browser runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPrefs {
    /// Open the in-app pane automatically when a browser tool runs.
    #[serde(default = "default_browser_auto_show")]
    pub auto_show: bool,
    /// Pane width as % of the main area (clamped 20-70).
    #[serde(default = "default_browser_width")]
    pub width_pct: u32,
}

fn default_browser_auto_show() -> bool {
    true
}

fn default_browser_width() -> u32 {
    45
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceSttPrefs {
    pub engine: String,
    pub model: String,
    pub language: String,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceVadPrefs {
    pub threshold: f32,
    pub silence_ms: u32,
    pub min_speech_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceTtsPrefs {
    pub engine: String,
    pub voice: String,
    pub speed: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoicePrefs {
    pub autoplay: bool,
    pub enabled: bool,
    /// Microphone device name; empty = system default.
    pub mic: String,
    pub stt: VoiceSttPrefs,
    pub vad: VoiceVadPrefs,
    pub tts: VoiceTtsPrefs,
    #[serde(default)]
    pub runtime: VoiceRuntimePrefs,
}

impl Default for VoicePrefs {
    fn default() -> Self {
        Self {
            autoplay: true,
            enabled: true,
            mic: String::new(),
            stt: VoiceSttPrefs {
                engine: "sherpa-onnx".into(),
                model: "zipformer-en-20M-int8".into(),
                language: "en".into(),
                sample_rate: 16000,
            },
            vad: VoiceVadPrefs { threshold: 0.5, silence_ms: 700, min_speech_ms: 250 },
            tts: VoiceTtsPrefs { engine: "kokoro".into(), voice: "0".into(), speed: 1.0 },
            runtime: VoiceRuntimePrefs::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingPrefs {
    /// Enabled agent ids. Empty + no conf file = all detected (enable-all default).
    pub agents: Vec<String>,
    pub default: String,
}

/// Non-secret LLM routing. Secrets stay in `.env`/env. `.env`/`env` win
/// over these when both are set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmPrefs {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
}

fn default_llm_provider() -> String {
    "deepseek".into()
}

fn default_llm_model() -> String {
    "deepseek-flash".into()
}

impl Default for LlmPrefs {
    fn default() -> Self {
        Self { provider: default_llm_provider(), model: default_llm_model() }
    }
}

/// Agent loop guardrails (mirrors AGENT_MAX_STEPS / AGENT_TIMEOUT_MS).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPrefs {
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_max_steps() -> usize {
    15
}

fn default_timeout_ms() -> u64 {
    120_000
}

impl Default for AgentPrefs {
    fn default() -> Self {
        Self { max_steps: default_max_steps(), timeout_ms: default_timeout_ms() }
    }
}

/// Vector-memory embedding choice (mirrors EMBEDDING_MODEL / EMBEDDING_DIM).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPrefs {
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    #[serde(default = "default_embedding_dim")]
    pub embedding_dim: usize,
}

fn default_embedding_model() -> String {
    "openai/text-embedding-3-small".into()
}

fn default_embedding_dim() -> usize {
    1536
}

impl Default for MemoryPrefs {
    fn default() -> Self {
        Self {
            embedding_model: default_embedding_model(),
            embedding_dim: default_embedding_dim(),
        }
    }
}

/// Tunables for the live voice loop. Every field is exercised by
/// `core/src/voice/manager.rs`; editing the conf and restarting is enough
/// to test VAD/STT-chunk/TTS-thread tradeoffs without rebuilding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceRuntimePrefs {
    #[serde(default = "default_max_utterance_ms")]
    pub max_utterance_ms: u32,
    #[serde(default = "default_decode_every_frames")]
    pub decode_every_frames: usize,
    #[serde(default = "default_chunk_max_chars")]
    pub chunk_max_chars: usize,
    #[serde(default = "default_chunk_min_merge")]
    pub chunk_min_merge: usize,
    #[serde(default = "default_voice_threads")]
    pub num_threads: i32,
}

fn default_max_utterance_ms() -> u32 {
    30_000
}

fn default_decode_every_frames() -> usize {
    16
}

fn default_chunk_max_chars() -> usize {
    220
}

fn default_chunk_min_merge() -> usize {
    12
}

fn default_voice_threads() -> i32 {
    2
}

impl Default for VoiceRuntimePrefs {
    fn default() -> Self {
        Self {
            max_utterance_ms: default_max_utterance_ms(),
            decode_every_frames: default_decode_every_frames(),
            chunk_max_chars: default_chunk_max_chars(),
            chunk_min_merge: default_chunk_min_merge(),
            num_threads: default_voice_threads(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    pub browser: BrowserPrefs,
    pub voice: VoicePrefs,
    pub coding: CodingPrefs,
    #[serde(default)]
    pub llm: LlmPrefs,
    #[serde(default)]
    pub agent: AgentPrefs,
    #[serde(default)]
    pub memory: MemoryPrefs,
}

    impl Default for Prefs {
    fn default() -> Self {
        Self {
            browser: BrowserPrefs { auto_show: default_browser_auto_show(), width_pct: default_browser_width() },
            voice: VoicePrefs::default(),
            coding: CodingPrefs { agents: vec![], default: String::new() },
            llm: LlmPrefs::default(),
            agent: AgentPrefs::default(),
            memory: MemoryPrefs::default(),
        }
    }
}

impl Prefs {
    /// Usable once coding agents are chosen (browser needs no setup — it is bundled).
    pub fn onboarded(&self) -> bool {
        !self.coding.agents.is_empty() || !self.coding.default.trim().is_empty()
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub fn parse_conf(text: &str) -> Prefs {
    let mut prefs = Prefs::default();
    let mut section = String::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_lowercase();
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = line[..eq].trim().to_lowercase();
        let mut value = line[eq + 1..].trim().to_string();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = value[1..value.len() - 1].to_string();
        }
        match (section.as_str(), key.as_str()) {
            // Current in-app pane prefs.
            ("browser", "auto_show") => {
                if let Some(b) = parse_bool(&value) {
                    prefs.browser.auto_show = b;
                }
            }
            ("browser", "width_pct") => {
                if let Ok(n) = value.parse::<u32>() {
                    prefs.browser.width_pct = n.clamp(20, 70);
                }
            }
            // Legacy system-browser keys (pre bundled-Chromium): ignored so
            // old comrade.conf files keep loading. Never written back.
            ("browser", "exe")
            | ("browser", "kind")
            | ("browser", "headless")
            | ("browser", "profile")
            | ("browser", "debug_port") => {}
            ("voice", "autoplay") => {
                if let Some(b) = parse_bool(&value) {
                    prefs.voice.autoplay = b;
                }
            }
            ("voice", "enabled") => {
                if let Some(b) = parse_bool(&value) {
                    prefs.voice.enabled = b;
                }
            }
            ("voice", "mic") => prefs.voice.mic = value,
            ("voice.stt", "engine") => prefs.voice.stt.engine = value,
            ("voice.stt", "model") => prefs.voice.stt.model = value,
            ("voice.stt", "language") => prefs.voice.stt.language = value,
            ("voice.stt", "sample_rate") => {
                if let Ok(n) = value.parse() {
                    prefs.voice.stt.sample_rate = n;
                }
            }
            ("voice.vad", "threshold") => {
                if let Ok(n) = value.parse() {
                    prefs.voice.vad.threshold = n;
                }
            }
            ("voice.vad", "silence_ms") => {
                if let Ok(n) = value.parse() {
                    prefs.voice.vad.silence_ms = n;
                }
            }
            ("voice.vad", "min_speech_ms") => {
                if let Ok(n) = value.parse() {
                    prefs.voice.vad.min_speech_ms = n;
                }
            }
            ("voice.tts", "engine") => prefs.voice.tts.engine = value,
            ("voice.tts", "voice") => prefs.voice.tts.voice = value,
            ("voice.tts", "speed") => {
                if let Ok(n) = value.parse::<f32>() {
                    if (0.25..=4.0).contains(&n) {
                        prefs.voice.tts.speed = n;
                    }
                }
            }
            ("voice.runtime", "max_utterance_ms") => {
                if let Ok(n) = value.parse() {
                    prefs.voice.runtime.max_utterance_ms = n;
                }
            }
            ("voice.runtime", "decode_every_frames") => {
                if let Ok(n) = value.parse::<usize>() {
                    prefs.voice.runtime.decode_every_frames = n.max(1);
                }
            }
            ("voice.runtime", "chunk_max_chars") => {
                if let Ok(n) = value.parse::<usize>() {
                    prefs.voice.runtime.chunk_max_chars = n.max(20);
                }
            }
            ("voice.runtime", "chunk_min_merge") => {
                if let Ok(n) = value.parse::<usize>() {
                    prefs.voice.runtime.chunk_min_merge = n;
                }
            }
            ("voice.runtime", "num_threads") => {
                if let Ok(n) = value.parse::<i32>() {
                    prefs.voice.runtime.num_threads = n.clamp(1, 16);
                }
            }
            ("llm", "provider") => {
                let v = value.trim().to_lowercase();
                prefs.llm.provider = if v == "openrouter" { "openrouter".into() } else { "deepseek".into() };
            }
            ("llm", "model") => {
                if !value.trim().is_empty() {
                    prefs.llm.model = value;
                }
            }
            ("agent", "max_steps") => {
                if let Ok(n) = value.parse::<usize>() {
                    prefs.agent.max_steps = n.max(1);
                }
            }
            ("agent", "timeout_ms") => {
                if let Ok(n) = value.parse::<u64>() {
                    prefs.agent.timeout_ms = n.max(1000);
                }
            }
            ("memory", "embedding_model") => {
                if !value.trim().is_empty() {
                    prefs.memory.embedding_model = value;
                }
            }
            ("memory", "embedding_dim") => {
                if let Ok(n) = value.parse::<usize>() {
                    prefs.memory.embedding_dim = n.max(1);
                }
            }
            ("coding", "agents") => {
                prefs.coding.agents = value
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            ("coding", "default") => prefs.coding.default = value.trim().to_lowercase(),
            _ => {}
        }
    }
    prefs
}

pub fn render_conf(prefs: &Prefs) -> String {
    format!(
        "# Comrade preferences — safe to edit. Secrets stay in .env.\n\
         # Precedence for [llm]/[agent]/[memory]: ENV > .env > this file.\n\
         # [browser] controls the in-app pane only. The browser itself is\n\
         # Comrade's bundled Chromium (comrade-agent/browser/, isolated\n\
         # browser-profile/) — always headless, visible only inside the app.\n\
         [browser]\n\
         auto_show = {}\n\
         width_pct = {}\n\
         \n\
         [voice]\n\
         autoplay = {}\n\
         enabled = {}\n\
         mic = {}\n\
         \n\
         [voice.stt]\n\
         engine = {}\n\
         model = {}\n\
         language = {}\n\
         sample_rate = {}\n\
         \n\
         [voice.vad]\n\
         threshold = {}\n\
         silence_ms = {}\n\
         min_speech_ms = {}\n\
         \n\
         [voice.tts]\n\
         engine = {}\n\
         voice = {}\n\
         speed = {}\n\
         \n\
         [voice.runtime]\n\
         max_utterance_ms = {}\n\
         decode_every_frames = {}\n\
         chunk_max_chars = {}\n\
         chunk_min_merge = {}\n\
         num_threads = {}\n\
         \n\
         [llm]\n\
         provider = {}\n\
         model = {}\n\
         \n\
         [agent]\n\
         max_steps = {}\n\
         timeout_ms = {}\n\
         \n\
         [memory]\n\
         embedding_model = {}\n\
         embedding_dim = {}\n\
         \n\
         [coding]\n\
         agents = {}\n\
         default = {}\n",
        prefs.browser.auto_show,
        prefs.browser.width_pct,
        prefs.voice.autoplay,
        prefs.voice.enabled,
        prefs.voice.mic,
        prefs.voice.stt.engine,
        prefs.voice.stt.model,
        prefs.voice.stt.language,
        prefs.voice.stt.sample_rate,
        prefs.voice.vad.threshold,
        prefs.voice.vad.silence_ms,
        prefs.voice.vad.min_speech_ms,
        prefs.voice.tts.engine,
        prefs.voice.tts.voice,
        prefs.voice.tts.speed,
        prefs.voice.runtime.max_utterance_ms,
        prefs.voice.runtime.decode_every_frames,
        prefs.voice.runtime.chunk_max_chars,
        prefs.voice.runtime.chunk_min_merge,
        prefs.voice.runtime.num_threads,
        prefs.llm.provider,
        prefs.llm.model,
        prefs.agent.max_steps,
        prefs.agent.timeout_ms,
        prefs.memory.embedding_model,
        prefs.memory.embedding_dim,
        prefs.coding.agents.join(", "),
        prefs.coding.default,
    )
}

pub fn conf_path() -> PathBuf {
    crate::paths::comrade_home().join(CONF_NAME)
}

/// Load from an explicit path (tests + custom locations).
pub fn load_from(path: &Path) -> Prefs {
    std::fs::read_to_string(path).map(|t| parse_conf(&t)).unwrap_or_default()
}

pub fn save_to(path: &Path, prefs: &Prefs) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // No validation needed for the bundled browser: it is provisioned under
    // comrade-agent/browser/ (see scripts/fetch-chromium.sh) and the pane
    // prefs are plain numbers/bools clamped on parse. Coding agents need at
    // least one entry to be useful, but an empty list still saves (onboarding).
    std::fs::write(path, render_conf(prefs))?;
    Ok(())
}

pub fn load() -> Prefs {
    load_from(&conf_path())
}

pub fn save(prefs: &Prefs) -> anyhow::Result<()> {
    save_to(&conf_path(), prefs)
}

// ---------- bundled browser ----------

/// Backwards-compatible alias: the "detected browser" list is gone (Comrade
/// drives only its bundled Chromium). Kept so old serialized payloads and
/// external callers still deserialize; always empty from here on.
#[derive(Clone, Debug, Serialize)]
pub struct BrowserInfo {
    pub name: String,
    pub exe: String,
    pub kind: String,
    pub version: String,
    pub automation_supported: bool,
}

/// No system browsers are used anymore — the agent drives only Comrade's
/// bundled Chromium inside the app. Returns an empty list.
pub fn detect_browsers() -> Vec<BrowserInfo> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conf_round_trip() {
        let dir = std::env::temp_dir().join("comrade-prefs-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(CONF_NAME);
        let prefs = Prefs {
            browser: BrowserPrefs { auto_show: false, width_pct: 60 },
            voice: VoicePrefs { autoplay: false, ..VoicePrefs::default() },
            coding: CodingPrefs { agents: vec!["opencode".into(), "claude".into()], default: "opencode".into() },
            ..Prefs::default()
        };
        std::fs::write(&path, render_conf(&prefs)).unwrap();
        assert_eq!(load_from(&path), prefs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conf_new_sections_round_trip() {
        let prefs = parse_conf("[browser]\nauto_show = false\nwidth_pct = 60\n[llm]\nprovider = openrouter\nmodel = x/y\n[agent]\nmax_steps = 7\ntimeout_ms = 5000\n[memory]\nembedding_model = a/b\nembedding_dim = 42\n[voice.runtime]\nmax_utterance_ms = 5000\ndecode_every_frames = 4\nchunk_max_chars = 99\nchunk_min_merge = 5\nnum_threads = 4\n");
        assert_eq!(prefs.browser.auto_show, false);
        assert_eq!(prefs.browser.width_pct, 60);
        assert_eq!(prefs.llm.provider, "openrouter");
        assert_eq!(prefs.llm.model, "x/y");
        assert_eq!(prefs.agent.max_steps, 7);
        assert_eq!(prefs.agent.timeout_ms, 5000);
        assert_eq!(prefs.memory.embedding_model, "a/b");
        assert_eq!(prefs.memory.embedding_dim, 42);
        assert_eq!(prefs.voice.runtime.max_utterance_ms, 5000);
        assert_eq!(prefs.voice.runtime.decode_every_frames, 4);
        assert_eq!(prefs.voice.runtime.chunk_max_chars, 99);
        assert_eq!(prefs.voice.runtime.chunk_min_merge, 5);
        assert_eq!(prefs.voice.runtime.num_threads, 4);
        // clamping keeps garbage test-friendly
        let prefs = parse_conf("[browser]\nwidth_pct = 5\n[voice.runtime]\ndecode_every_frames = 0\nchunk_max_chars = 1\nnum_threads = 99\n[agent]\nmax_steps = 0\n");
        assert_eq!(prefs.browser.width_pct, 20);
        assert_eq!(prefs.voice.runtime.decode_every_frames, 1);
        assert_eq!(prefs.voice.runtime.chunk_max_chars, 20);
        assert_eq!(prefs.voice.runtime.num_threads, 16);
        assert_eq!(prefs.agent.max_steps, 1);
        // serde defaults keep old confs/frontend payloads loading
        let prefs: Prefs = serde_json::from_str(r#"{"browser":{},"voice":{"autoplay":true,"enabled":true,"mic":"","stt":{"engine":"s","model":"m","language":"en","sample_rate":16000},"vad":{"threshold":0.5,"silence_ms":700,"min_speech_ms":250},"tts":{"engine":"k","voice":"0","speed":1.0}},"coding":{"agents":[],"default":""}}"#).unwrap();
        assert_eq!(prefs.browser.auto_show, true);
        assert_eq!(prefs.browser.width_pct, 45);
        assert_eq!(prefs.voice.runtime.num_threads, 2);
        assert_eq!(prefs.llm.provider, "deepseek");
        // render includes the new sections
        let text = render_conf(&Prefs::default());
        assert!(text.contains("[browser]"));
        assert!(text.contains("auto_show"));
        assert!(text.contains("[voice.runtime]"));
        assert!(text.contains("[llm]"));
        assert!(text.contains("[agent]"));
        assert!(text.contains("[memory]"));
    }

    #[test]
    fn conf_defaults_and_comments() {
        let prefs = parse_conf("# comment\n[Browser]\nAUTO_SHOW = off\nWIDTH_PCT = 60\n[voice]\nautoplay = off\n[coding]\nagents = OpenCode, Claude\ndefault = Claude\n");
        assert!(!prefs.browser.auto_show);
        assert_eq!(prefs.browser.width_pct, 60);
        assert!(!prefs.voice.autoplay);
        assert_eq!(prefs.coding.agents, vec!["opencode".to_string(), "claude".to_string()]);
        assert_eq!(prefs.coding.default, "claude");
        assert!(!Prefs::default().onboarded());
        assert!(prefs.onboarded());
        assert_eq!(Prefs::default().browser.auto_show, true);
        assert_eq!(Prefs::default().browser.width_pct, 45);
    }

    #[test]
    fn legacy_browser_keys_are_ignored() {
        let prefs = parse_conf("[browser]\nexe = /x/y\nkind = flatpak\nheadless = yes\nprofile = comrade\ndebug_port = 9333\n");
        assert_eq!(prefs.browser, Prefs::default().browser);
        // Old exe-based confs still render without the legacy keys.
        let text = render_conf(&prefs);
        assert!(!text.contains("exe ="));
        assert!(text.contains("auto_show"));
    }

    #[test]
    fn save_needs_no_browser_exe() {
        let dir = std::env::temp_dir().join("comrade-prefs-test2");
        let prefs = Prefs::default();
        assert!(save_to(&dir.join(CONF_NAME), &prefs).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_system_browser_scan() {
        assert!(detect_browsers().is_empty());
    }
}

// ---------- coding agent detection ----------

fn find_on_path(name: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Clone, Debug, Serialize)]
pub struct CodingAgentInfo {
    pub id: String,
    pub name: String,
    pub bin: String,
    pub version: String,
    /// Non-interactive execution is implemented for this agent.
    pub exec_supported: bool,
}

pub struct AgentCandidate {
    pub id: &'static str,
    pub name: &'static str,
    pub bins: &'static [&'static str],
    /// False when even `--version` has side effects (spawns a GUI).
    pub probe_version: bool,
    pub exec_supported: bool,
}

/// Preferred routing order doubles as display order.
pub fn coding_agent_candidates() -> Vec<AgentCandidate> {
    vec![
        AgentCandidate { id: "opencode", name: "OpenCode", bins: &["opencode"], probe_version: true, exec_supported: true },
        AgentCandidate { id: "claude", name: "Claude Code", bins: &["claude"], probe_version: true, exec_supported: true },
        AgentCandidate { id: "codex", name: "Codex", bins: &["codex"], probe_version: true, exec_supported: true },
        AgentCandidate { id: "copilot", name: "Copilot CLI", bins: &["copilot"], probe_version: true, exec_supported: true },
        AgentCandidate { id: "qwen", name: "Qwen Code", bins: &["qwen"], probe_version: true, exec_supported: true },
        AgentCandidate { id: "kimi", name: "Kimi", bins: &["kimi", "kimi-cli"], probe_version: true, exec_supported: false },
        AgentCandidate { id: "deepseek", name: "DeepSeek harness", bins: &["deepseek-cli", "dsk"], probe_version: true, exec_supported: false },
        AgentCandidate { id: "antigravity", name: "Antigravity", bins: &["antigravity"], probe_version: false, exec_supported: false },
    ]
}

fn bin_version(bin: &str) -> String {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if out.is_empty() {
                String::from_utf8_lossy(&o.stderr).trim().to_string()
            } else {
                out
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Every coding agent found on this machine. Version probing is timeout-guarded
/// by the caller convention: keep each probe under ~5s (see run_probe).
pub fn detect_coding_agents() -> Vec<CodingAgentInfo> {
    let mut found = Vec::new();
    for candidate in coding_agent_candidates() {
        let mut bin_path: Option<String> = None;
        for bin in candidate.bins {
            if bin.contains('/') || bin.contains('\\') {
                if PathBuf::from(bin).is_file() {
                    bin_path = Some(bin.to_string());
                    break;
                }
            } else if let Some(p) = find_on_path(bin) {
                bin_path = Some(p.to_string_lossy().to_string());
                break;
            }
        }
        if let Some(bin) = bin_path {
            // Timeout-guarded probe: some CLIs hang instead of printing.
            let version = if candidate.probe_version {
                let (tx, rx) = std::sync::mpsc::channel();
                let probe_bin = bin.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(bin_version(&probe_bin));
                });
                rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or_default()
            } else {
                String::new()
            };
            found.push(CodingAgentInfo {
                id: candidate.id.to_string(),
                name: candidate.name.to_string(),
                bin,
                version: version.lines().next().unwrap_or("").chars().take(80).collect(),
                exec_supported: candidate.exec_supported,
            });
        }
    }
    found
}
