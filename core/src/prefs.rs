/**
 * User preferences in `comrade.conf` (INI-lite) inside the comrade-agent home.
 * Non-secret settings only — API keys stay in `.env`.
 *
 * ```ini
 * [browser]
 * exe = /opt/brave-origin-bin/brave-origin
 * kind = binary        # binary | flatpak
 * headless = false
 * profile = user       # user | comrade: your own profile (logged in)
 *                      # or Comrade's isolated profile
 * debug_port = 9222    # attach here first when your browser was started
 *                      # with --remote-debugging-port=9222 (0 = off)
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
 * [coding]
 * agents = opencode, claude, copilot
 * default = opencode
 * ```
 */
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const CONF_NAME: &str = "comrade.conf";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPrefs {
    pub exe: String,
    pub kind: String,
    pub headless: bool,
    /// "user" (default): drive your own browser profile — logins, Gmail,
    /// tabs all present. "comrade": isolated profile under comrade-agent/.
    #[serde(default = "default_browser_profile")]
    pub profile: String,
    /// DevTools port to attach to before launching (0 = never attach).
    #[serde(default = "default_debug_port")]
    pub debug_port: u16,
}

fn default_browser_profile() -> String {
    "user".into()
}

fn default_debug_port() -> u16 {
    9222
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingPrefs {
    /// Enabled agent ids. Empty + no conf file = all detected (enable-all default).
    pub agents: Vec<String>,
    pub default: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    pub browser: BrowserPrefs,
    pub voice: VoicePrefs,
    pub coding: CodingPrefs,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            browser: BrowserPrefs {
                exe: String::new(),
                kind: "binary".into(),
                headless: false,
                profile: default_browser_profile(),
                debug_port: default_debug_port(),
            },
            voice: VoicePrefs::default(),
            coding: CodingPrefs { agents: vec![], default: String::new() },
        }
    }
}

impl Prefs {
    /// A prefs object is usable once a browser executable is chosen.
    pub fn onboarded(&self) -> bool {
        !self.browser.exe.trim().is_empty()
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
            ("browser", "exe") => prefs.browser.exe = value,
            ("browser", "kind") => {
                prefs.browser.kind = if value == "flatpak" { "flatpak".into() } else { "binary".into() }
            }
            ("browser", "headless") => {
                if let Some(b) = parse_bool(&value) {
                    prefs.browser.headless = b;
                }
            }
            ("browser", "profile") => {
                prefs.browser.profile =
                    if value.trim().to_lowercase() == "comrade" { "comrade".into() } else { "user".into() }
            }
            ("browser", "debug_port") => {
                if let Ok(n) = value.parse::<u16>() {
                    prefs.browser.debug_port = n;
                }
            }
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
         [browser]\n\
         exe = {}\n\
         kind = {}\n\
         headless = {}\n\
         profile = {}\n\
         debug_port = {}\n\
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
         [coding]\n\
         agents = {}\n\
         default = {}\n",
        prefs.browser.exe,
        prefs.browser.kind,
        prefs.browser.headless,
        prefs.browser.profile,
        prefs.browser.debug_port,
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
    // Validate before writing: a browser choice must point at something real.
    if prefs.browser.exe.trim().is_empty() {
        anyhow::bail!("No browser selected.");
    }
    if prefs.browser.kind == "flatpak" {
        if prefs.browser.exe.contains('/') || prefs.browser.exe.contains(' ') {
            anyhow::bail!("Flatpak browser must be an app id (e.g. com.brave.Browser).");
        }
    } else if !Path::new(&prefs.browser.exe).exists() {
        anyhow::bail!("Browser executable not found: {}", prefs.browser.exe);
    }
    std::fs::write(path, render_conf(prefs))?;
    Ok(())
}

pub fn load() -> Prefs {
    load_from(&conf_path())
}

pub fn save(prefs: &Prefs) -> anyhow::Result<()> {
    save_to(&conf_path(), prefs)
}

// ---------- browser detection ----------

#[derive(Clone, Debug, Serialize)]
pub struct BrowserInfo {
    pub name: String,
    pub exe: String,
    pub kind: String,
    pub version: String,
    /// CDP automation is implemented (Chromium family). Gecko browsers are
    /// listed for completeness but can't be driven yet.
    pub automation_supported: bool,
}

fn path_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        for part in std::env::split_paths(&path) {
            dirs.push(part);
        }
    }
    dirs
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    for dir in path_search_dirs() {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn exe_version(exe: &str, kind: &str) -> String {
    let (program, args): (&str, Vec<&str>) = if kind == "flatpak" {
        ("flatpak", vec!["run", exe, "--version"])
    } else {
        (exe, vec!["--version"])
    };
    std::process::Command::new(program)
        .args(&args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Candidate (display name, binary names or absolute paths) per platform.
/// Gecko browsers are included with automation_supported=false.
fn binary_candidates() -> Vec<(&'static str, Vec<&'static str>, bool)> {
    if cfg!(target_os = "windows") {
        let pf = std::env::var("ProgramFiles").unwrap_or_default();
        let pfx86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        return vec![
            ("Brave", vec![
                Box::leak(format!("{pf}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe").into_boxed_str()) as &str,
                "brave",
            ], true),
            ("Chrome", vec![
                Box::leak(format!("{pf}\\Google\\Chrome\\Application\\chrome.exe").into_boxed_str()) as &str,
                "chrome",
            ], true),
            ("Edge", vec![
                Box::leak(format!("{pf}\\Microsoft\\Edge\\Application\\msedge.exe").into_boxed_str()) as &str,
                "msedge",
            ], true),
            ("Opera", vec![
                Box::leak(format!("{local}\\Programs\\Opera\\opera.exe").into_boxed_str()) as &str,
                "opera",
            ], true),
            ("Vivaldi", vec![
                Box::leak(format!("{local}\\Vivaldi\\Application\\vivaldi.exe").into_boxed_str()) as &str,
                "vivaldi",
            ], true),
            ("Chromium", vec!["chromium", Box::leak(format!("{pfx86}\\Chromium\\Application\\chrome.exe").into_boxed_str()) as &str], true),
            ("Firefox", vec![
                Box::leak(format!("{pf}\\Mozilla Firefox\\firefox.exe").into_boxed_str()) as &str,
                "firefox",
            ], false),
            ("Zen", vec![
                Box::leak(format!("{local}\\Zen\\zen.exe").into_boxed_str()) as &str,
                "zen",
            ], false),
        ];
    }
    if cfg!(target_os = "macos") {
        return vec![
            ("Brave", vec!["/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", "brave", "brave-browser"], true),
            ("Chrome", vec!["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "google-chrome", "chrome"], true),
            ("Chromium", vec!["/Applications/Chromium.app/Contents/MacOS/Chromium", "chromium"], true),
            ("Edge", vec!["/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "microsoft-edge"], true),
            ("Opera", vec!["/Applications/Opera.app/Contents/MacOS/Opera", "opera"], true),
            ("Vivaldi", vec!["/Applications/Vivaldi.app/Contents/MacOS/Vivaldi", "vivaldi"], true),
            ("Firefox", vec!["/Applications/Firefox.app/Contents/MacOS/firefox", "firefox"], false),
            ("Zen", vec!["/Applications/Zen.app/Contents/MacOS/zen", "zen"], false),
            ("LibreWolf", vec!["/Applications/LibreWolf.app/Contents/MacOS/librewolf", "librewolf"], false),
        ];
    }
    // Linux / other Unix.
    vec![
        ("Brave", vec!["brave", "brave-browser", "brave-origin", "/opt/brave-origin-bin/brave-origin", "/opt/brave.com/brave/brave"], true),
        ("Chrome", vec!["google-chrome", "google-chrome-stable", "/opt/google/chrome/chrome", "/usr/bin/google-chrome-stable"], true),
        ("Chromium", vec!["chromium", "chromium-browser", "/usr/bin/chromium"], true),
        ("Edge", vec!["microsoft-edge", "microsoft-edge-stable", "/opt/microsoft/msedge/msedge"], true),
        ("Opera", vec!["opera", "opera-stable", "/usr/bin/opera"], true),
        ("Vivaldi", vec!["vivaldi", "vivaldi-stable", "/opt/vivaldi/vivaldi"], true),
        ("Firefox", vec!["firefox", "/usr/bin/firefox", "/opt/firefox/firefox", "/snap/bin/firefox"], false),
        ("Zen", vec!["zen", "zen-browser", "/opt/zen/zen"], false),
        ("LibreWolf", vec!["librewolf", "/usr/bin/librewolf", "/opt/librewolf/librewolf"], false),
        ("Floorp", vec!["floorp", "/usr/bin/floorp", "/opt/floorp/floorp"], false),
    ]
}

fn flatpak_candidates() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("Brave", "com.brave.Browser", true),
        ("Chrome", "com.google.Chrome", true),
        ("Chromium", "org.chromium.Chromium", true),
        ("Edge", "com.microsoft.Edge", true),
        ("Firefox", "org.mozilla.firefox", false),
    ]
}

fn flatpak_apps() -> Vec<String> {
    std::process::Command::new("flatpak")
        .args(["list", "--app", "--columns=application"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Dedup-aware insert shared by binary + flatpak scans.
fn insert_browser(
    found: &mut Vec<BrowserInfo>,
    seen: &mut HashMap<String, bool>,
    name: &str,
    exe: String,
    kind: &str,
    version: String,
    automation_supported: bool,
) {
    if seen.contains_key(&exe) {
        return;
    }
    seen.insert(exe.clone(), true);
    // Wrapper scripts canonicalize differently from the real binary but
    // report the same name+version — drop those duplicates too.
    if !version.is_empty() {
        let key = format!("{name}\0{version}");
        if seen.contains_key(&key) {
            return;
        }
        seen.insert(key, true);
    }
    found.push(BrowserInfo { name: name.to_string(), exe, kind: kind.to_string(), version, automation_supported });
}

/// Every browser found on this machine, deduplicated.
pub fn detect_browsers() -> Vec<BrowserInfo> {
    let mut found: Vec<BrowserInfo> = Vec::new();
    let mut seen: HashMap<String, bool> = HashMap::new();
    for (name, candidates, automatable) in binary_candidates() {
        for candidate in candidates {
            let exe_path = if candidate.contains('/') || candidate.contains('\\') {
                let p = PathBuf::from(candidate);
                if p.is_file() { Some(p) } else { None }
            } else {
                find_on_path(candidate)
            };
            if let Some(path) = exe_path {
                // Canonicalize so symlinks (/usr/bin/x → /opt/...) dedup to one entry.
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                let exe = path.to_string_lossy().to_string();
                insert_browser(&mut found, &mut seen, name, exe.clone(), "binary", exe_version(&exe, "binary"), automatable);
            }
        }
    }
    if cfg!(target_os = "linux") {
        let installed = flatpak_apps();
        for (name, app_id, automatable) in flatpak_candidates() {
            if installed.iter().any(|a| a == app_id) {
                insert_browser(
                    &mut found,
                    &mut seen,
                    &format!("{name} (flatpak)"),
                    app_id.to_string(),
                    "flatpak",
                    exe_version(app_id, "flatpak"),
                    automatable,
                );
            }
        }
    }
    found
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
            browser: BrowserPrefs {
                exe: "/usr/bin/chromium".into(),
                kind: "binary".into(),
                headless: true,
                profile: "comrade".into(),
                debug_port: 9333,
            },
            voice: VoicePrefs { autoplay: false, ..VoicePrefs::default() },
            coding: CodingPrefs { agents: vec!["opencode".into(), "claude".into()], default: "opencode".into() },
        };
        // exe may not exist on all machines — bypass validation via render/parse.
        std::fs::write(&path, render_conf(&prefs)).unwrap();
        assert_eq!(load_from(&path), prefs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conf_defaults_and_comments() {
        let prefs = parse_conf("# comment\n[Browser]\nEXE = /x/y\nHEADLESS = yes\n[voice]\nautoplay = off\n[coding]\nagents = OpenCode, Claude\ndefault = Claude\n");
        assert_eq!(prefs.browser.exe, "/x/y");
        assert!(prefs.browser.headless);
        assert!(!prefs.voice.autoplay);
        assert_eq!(prefs.browser.kind, "binary");
        assert_eq!(prefs.coding.agents, vec!["opencode".to_string(), "claude".to_string()]);
        assert_eq!(prefs.coding.default, "claude");
        assert!(!Prefs::default().onboarded());
        assert!(prefs.onboarded());
        // New installs default to the user's own (logged-in) browser profile.
        assert_eq!(Prefs::default().browser.profile, "user");
        assert_eq!(Prefs::default().browser.debug_port, 9222);
        assert_eq!(prefs.browser.profile, "user");
        assert_eq!(prefs.browser.debug_port, 9222);
    }

    #[test]
    fn browser_profile_prefs_parse() {
        let prefs = parse_conf("[browser]\nexe = /x/y\nprofile = comrade\ndebug_port = 9333\n");
        assert_eq!(prefs.browser.profile, "comrade");
        assert_eq!(prefs.browser.debug_port, 9333);
        // Anything but comrade (or garbage / out-of-range ports) falls back safe.
        let prefs = parse_conf("[browser]\nexe = /x/y\nprofile = EVERYTHING\ndebug_port = banana\n");
        assert_eq!(prefs.browser.profile, "user");
        assert_eq!(prefs.browser.debug_port, 9222);
    }

    #[test]
    fn validation_rejects_missing_exe() {
        let dir = std::env::temp_dir().join("comrade-prefs-test2");
        let mut prefs = Prefs::default();
        assert!(save_to(&dir.join(CONF_NAME), &prefs).is_err());
        prefs.browser.exe = "/definitely/not/here-12345".into();
        assert!(save_to(&dir.join(CONF_NAME), &prefs).is_err());
    }

    #[test]
    fn wrapper_scripts_dedup_by_name_and_version() {
        let mut found = Vec::new();
        let mut seen = HashMap::new();
        insert_browser(&mut found, &mut seen, "Brave", "/usr/bin/brave-origin".into(), "binary", "Brave Origin 154".into(), true);
        insert_browser(&mut found, &mut seen, "Brave", "/opt/brave-origin-bin/brave-origin".into(), "binary", "Brave Origin 154".into(), true);
        insert_browser(&mut found, &mut seen, "Chrome", "/usr/bin/google-chrome".into(), "binary", "Google Chrome 154".into(), true);
        // Same exe twice collapses even without a version.
        insert_browser(&mut found, &mut seen, "X", "/usr/bin/x".into(), "binary", "".into(), true);
        insert_browser(&mut found, &mut seen, "X", "/usr/bin/x".into(), "binary", "".into(), true);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].exe, "/usr/bin/brave-origin");
        assert!(found.iter().all(|b| b.automation_supported));
    }

    #[test]
    fn detection_does_not_crash() {        let browsers = detect_browsers();
        for b in &browsers {
            assert!(!b.exe.is_empty());
            assert!(!b.name.is_empty());
        }
    }
}

// ---------- coding agent detection ----------

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
