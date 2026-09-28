/**
 * Browser driver layer.
 *
 * Architecture (family-agnostic, Chromium implemented first):
 *
 * ```text
 * tools/browser.rs  ->  BrowserDriver trait  ->  ChromiumDriver (CDP)
 *                                          ->  GeckoDriver (WebDriver BiDi, planned)
 * ```
 *
 * Chromium family (Chrome, Brave, Edge, Opera, Vivaldi, Chromium) is driven
 * over the Chrome DevTools Protocol:
 * - the configured browser exe is launched once with `--remote-debugging-port`
 *   against a persistent profile (`comrade-agent/browser-profile`),
 * - tabs are managed via the HTTP DevTools endpoints (`/json/list`, `/json/new`),
 * - page automation goes over a per-call WebSocket (`tokio-tungstenite`).
 *
 * Firefox family (Firefox, Zen, LibreWolf, Floorp) shares the same trait but
 * returns a clear `FIREFOX_UNSUPPORTED` error until the BiDi backend lands.
 * Detection is by exe/app-id substring so any current or future fork works.
 */

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Family detection (works for any present or future fork)
// ---------------------------------------------------------------------------

/// Browser automation family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserFamily {
    Chromium,
    Gecko,
}

const GECKO_MARKERS: &[&str] = &[
    "firefox",
    "zen",
    "librewolf",
    "floorp",
    "waterfox",
    "mozilla",
];

/// Classify an exe path or flatpak app-id into its automation family.
/// Unknown / unbranded Chromium forks default to Chromium (CDP is the
/// de-facto standard there); only known Gecko markers route to Gecko.
pub fn detect_family(exe: &str) -> BrowserFamily {
    let lower = exe.to_lowercase();
    if GECKO_MARKERS.iter().any(|m| lower.contains(m)) {
        BrowserFamily::Gecko
    } else {
        BrowserFamily::Chromium
    }
}

// ---------------------------------------------------------------------------
// Managed Chromium process (singleton)
// ---------------------------------------------------------------------------

struct ManagedBrowser {
    /// Launched by us (killed on close) vs attached to the user's live
    /// browser (never killed — detach only).
    child: Option<tokio::process::Child>,
    port: u16,
    exe_key: String,
    profile_key: String,
}

/// Which profile to drive: the user's own (logged in) or Comrade's isolated one.
pub fn use_user_profile(mode: &str) -> bool {
    mode.trim().to_lowercase() != "comrade"
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

/// One profile location in all three OS conventions:
/// unix/mac relative paths, win relative to %LOCALAPPDATA% (+ `\User Data`).
struct Leaf {
    unix: &'static str,
    mac: &'static str,
    win: &'static str,
}

fn expand_leaf(leaf: &Leaf) -> String {
    if cfg!(target_os = "windows") {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| home_dir());
        format!("{base}\\{}\\User Data", leaf.win)
    } else if cfg!(target_os = "macos") {
        format!("{}/{}", home_dir(), leaf.mac)
    } else {
        let rel = leaf.unix;
        if let Some(stripped) = rel.strip_prefix(".config/") {
            match std::env::var("XDG_CONFIG_HOME").ok().filter(|v| !v.trim().is_empty()) {
                Some(base) => format!("{base}/{stripped}"),
                None => format!("{}/{rel}", home_dir()),
            }
        } else {
            format!("{}/{rel}", home_dir())
        }
    }
}

/// Ordered profile-dir candidates for the concrete binary/channel.
/// First entry = that build's own default location; the rest are sibling
/// fallbacks (other channels of the same browser).
fn user_profile_candidates(exe: &str) -> Vec<std::path::PathBuf> {
    struct Fam {
        mark: &'static str,
        channels: &'static [(&'static str, Leaf)],
        default: Leaf,
        fallback: Option<Leaf>,
    }
    // NOTE: the "Origin" build (this machine's brave-origin) keeps its data
    // under BraveSoftware/Brave-Origin — NOT Brave-Browser. Channel-aware
    // ordering matters: a wrong pick silently yields a fresh empty profile.
    let fams: &[Fam] = &[
        Fam { mark: "brave", channels: &[
            ("origin", Leaf { unix: ".config/BraveSoftware/Brave-Origin", mac: "Library/Application Support/BraveSoftware/Brave-Origin", win: "BraveSoftware\\Brave-Origin" }),
            ("beta", Leaf { unix: ".config/BraveSoftware/Brave-Browser-Beta", mac: "Library/Application Support/BraveSoftware/Brave-Browser-Beta", win: "BraveSoftware\\Brave-Browser-Beta" }),
            ("dev", Leaf { unix: ".config/BraveSoftware/Brave-Browser-Dev", mac: "Library/Application Support/BraveSoftware/Brave-Browser-Dev", win: "BraveSoftware\\Brave-Browser-Dev" }),
            ("nightly", Leaf { unix: ".config/BraveSoftware/Brave-Browser-Nightly", mac: "Library/Application Support/BraveSoftware/Brave-Browser-Nightly", win: "BraveSoftware\\Brave-Browser-Nightly" }),
        ],
            default: Leaf { unix: ".config/BraveSoftware/Brave-Browser", mac: "Library/Application Support/BraveSoftware/Brave-Browser", win: "BraveSoftware\\Brave-Browser" },
            fallback: Some(Leaf { unix: ".config/BraveSoftware/Brave-Origin", mac: "Library/Application Support/BraveSoftware/Brave-Origin", win: "BraveSoftware\\Brave-Origin" }) },
        Fam { mark: "msedge", channels: &[
            ("beta", Leaf { unix: ".config/microsoft-edge-beta", mac: "Library/Application Support/Microsoft Edge Beta", win: "Microsoft\\Edge Beta" }),
            ("dev", Leaf { unix: ".config/microsoft-edge-dev", mac: "Library/Application Support/Microsoft Edge Dev", win: "Microsoft\\Edge Dev" }),
        ],
            default: Leaf { unix: ".config/microsoft-edge", mac: "Library/Application Support/Microsoft Edge", win: "Microsoft\\Edge" },
            fallback: None },
        Fam { mark: "edge", channels: &[
            ("beta", Leaf { unix: ".config/microsoft-edge-beta", mac: "Library/Application Support/Microsoft Edge Beta", win: "Microsoft\\Edge Beta" }),
            ("dev", Leaf { unix: ".config/microsoft-edge-dev", mac: "Library/Application Support/Microsoft Edge Dev", win: "Microsoft\\Edge Dev" }),
        ],
            default: Leaf { unix: ".config/microsoft-edge", mac: "Library/Application Support/Microsoft Edge", win: "Microsoft\\Edge" },
            fallback: None },
        Fam { mark: "opera", channels: &[
            ("beta", Leaf { unix: ".config/opera-beta", mac: "Library/Application Support/com.operasoftware.OperaBeta", win: "Opera Software\\Opera Beta" }),
            ("developer", Leaf { unix: ".config/opera-developer", mac: "Library/Application Support/com.operasoftware.OperaDeveloper", win: "Opera Software\\Opera Developer" }),
        ],
            default: Leaf { unix: ".config/opera", mac: "Library/Application Support/com.operasoftware.Opera", win: "Opera Software\\Opera Stable" },
            fallback: None },
        Fam { mark: "vivaldi", channels: &[
            ("snapshot", Leaf { unix: ".config/vivaldi-snapshot", mac: "Library/Application Support/Vivaldi Snapshot", win: "Vivaldi\\Vivaldi Snapshot" }),
        ],
            default: Leaf { unix: ".config/vivaldi", mac: "Library/Application Support/Vivaldi", win: "Vivaldi" },
            fallback: None },
        Fam { mark: "chromium", channels: &[],
            default: Leaf { unix: ".config/chromium", mac: "Library/Application Support/Chromium", win: "Chromium" },
            fallback: None },
        Fam { mark: "chrome", channels: &[
            ("beta", Leaf { unix: ".config/google-chrome-beta", mac: "Library/Application Support/Google/Chrome Beta", win: "Google\\Chrome Beta" }),
            ("unstable", Leaf { unix: ".config/google-chrome-unstable", mac: "Library/Application Support/Google/Chrome Dev", win: "Google\\Chrome Dev" }),
            ("dev", Leaf { unix: ".config/google-chrome-unstable", mac: "Library/Application Support/Google/Chrome Dev", win: "Google\\Chrome Dev" }),
        ],
            default: Leaf { unix: ".config/google-chrome", mac: "Library/Application Support/Google/Chrome", win: "Google\\Chrome" },
            fallback: None },
    ];
    let lower = exe.to_lowercase();
    let Some(fam) = fams.iter().find(|f| lower.contains(f.mark)) else {
        return Vec::new();
    };
    let mut leaves: Vec<&Leaf> = Vec::new();
    if let Some((_, leaf)) = fam.channels.iter().find(|(m, _)| lower.contains(m)) {
        leaves.push(leaf);
    }
    leaves.push(&fam.default);
    if let Some(fb) = &fam.fallback {
        leaves.push(fb);
    }
    leaves.into_iter().map(|l| std::path::PathBuf::from(expand_leaf(l))).collect()
}

/// The browser's own profile directory (cookies, logins, tabs).
/// Unknown forks yield None → the isolated Comrade profile (safe default —
/// never guess at a path that could belong to another app).
pub fn user_profile_dir(exe: &str) -> Option<std::path::PathBuf> {
    let cands = user_profile_candidates(exe);
    if cands.is_empty() {
        return None;
    }
    // A live session wins: the profile actually in use right now.
    if let Some(live) = cands.iter().find(|p| p.is_dir() && lock_holder_pid(p).is_some()) {
        return Some(live.clone());
    }
    // Otherwise the first existing dir; if none exists yet, the build's own
    // default location (first candidate) so launch uses the standard place.
    cands.iter().find(|p| p.is_dir()).cloned().or_else(|| cands.into_iter().next())
}

/// Resolve the `--user-data-dir` for this run: your own profile (logged in)
/// or Comrade's isolated one, per Settings.
pub fn resolve_profile_dir(exe: &str, mode: &str) -> std::path::PathBuf {
    if use_user_profile(mode) {
        if let Some(dir) = user_profile_dir(exe) {
            return dir;
        }
    }
    profile_dir()
}

static MANAGED: OnceLock<tokio::sync::Mutex<Option<ManagedBrowser>>> = OnceLock::new();

fn managed_lock() -> &'static tokio::sync::Mutex<Option<ManagedBrowser>> {
    MANAGED.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn free_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn profile_dir() -> std::path::PathBuf {
    crate::paths::profile_dir_default()
}

fn debugger_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

async fn http_ok(url: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build();
    let Ok(client) = client else { return false };
    client
        .get(url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn wait_for_debugger(port: u16) -> anyhow::Result<()> {
    let url = format!("{}/json/version", debugger_url(port));
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if http_ok(&url).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    anyhow::bail!("browser did not expose DevTools on port {port} within 20s")
}

fn launch_args(port: u16, headless: bool, profile: &str) -> Vec<String> {
    let mut args = vec![
        format!("--remote-debugging-port={port}"),
        "--remote-allow-origins=*".to_string(),
        format!("--user-data-dir={profile}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--no-sandbox".to_string(),
        "about:blank".to_string(),
    ];
    if headless {
        args.push("--headless=new".to_string());
        args.push("--disable-gpu".to_string());
    }
    args
}

/// Ensure the configured Chromium browser is ready to drive.
/// Order: reuse our instance → attach to your live browser on debug_port →
/// launch (your own profile by default, so Gmail/logins are present).
/// Returns the DevTools HTTP base port.
pub async fn ensure_chromium() -> Result<u16, String> {
    let prefs = crate::prefs::load();
    let exe = prefs.browser.exe.trim().to_string();
    if exe.is_empty() {
        return Err("NO_BROWSER: pick a browser in Settings first.".to_string());
    }
    if detect_family(&exe) == BrowserFamily::Gecko {
        return Err(gecko_unsupported(&exe));
    }

    let mut guard = managed_lock().lock().await;

    // Reuse the live instance when it still answers (launched or attached).
    // A launched instance is only reused for the same exe + profile: if you
    // switched browsers or profiles in Settings, it is retired below.
    if let Some(m) = guard.as_mut() {
        let wanted_profile = resolve_profile_dir(&exe, &prefs.browser.profile).to_string_lossy().to_string();
        let same_target = m.exe_key == exe && m.profile_key == wanted_profile;
        let usable = match m.child.as_mut() {
            Some(child) => same_target && child.try_wait().map(|s| s.is_none()).unwrap_or(false),
            // Attached: owned by the user, assumed alive until probed.
            // (same_target is false by construction: profile_key == "attached".)
            None => true,
        };
        if usable && http_ok(&format!("{}/json/version", debugger_url(m.port))).await {
            return Ok(m.port);
        }
        // Stale: kill only what we launched; attached sessions just detach.
        if let Some(mut child) = m.child.take() {
            let _ = child.kill().await;
        }
        *guard = None;
    }

    if prefs.browser.kind == "flatpak" {
        return Err("FLATPAK_UNSUPPORTED: Chromium flatpak automation is not wired yet; pick a native binary build.".to_string());
    }

    // Attach to your already-running browser first (your tabs + logins, and
    // no profile-lock fight). Start it once with e.g.
    // `brave --remote-debugging-port=9222` and Comrade drives that window.
    if prefs.browser.debug_port != 0
        && http_ok(&format!("{}/json/version", debugger_url(prefs.browser.debug_port))).await
    {
        let port = prefs.browser.debug_port;
        *guard = Some(ManagedBrowser {
            child: None,
            port,
            exe_key: exe,
            profile_key: "attached".to_string(),
        });
        return Ok(port);
    }

    if !std::path::Path::new(&exe).exists() {
        return Err(format!("BROWSER_NOT_FOUND: executable missing: {exe}"));
    }

    let user_mode = use_user_profile(&prefs.browser.profile);
    let profile = resolve_profile_dir(&exe, &prefs.browser.profile);
    if let Err(e) = std::fs::create_dir_all(&profile) {
        return Err(format!("PROFILE_FAILED: cannot create browser profile: {e}"));
    }

    let profile_key = profile.to_string_lossy().to_string();
    let launch_once = |port: u16| launch_args(port, prefs.browser.headless, &profile_key);

    let port = free_port().map_err(|e| format!("PORT_FAILED: {e}"))?;
    match spawn_and_settle(&exe, &launch_once(port), port).await {
        Ok(child) => {
            *guard = Some(ManagedBrowser { child: Some(child), port, exe_key: exe, profile_key });
            return Ok(port);
        }
        Err(code) => {
            let locked_exit = code.starts_with("EXITED:21") || lock_held(&profile);
            let hung = code.starts_with("NO_DEVTOOLS");
            if !locked_exit && !hung {
                return Err(friendly_launch_error(&exe, &code));
            }
            if !user_mode {
                // Our isolated profile is ours to reclaim (orphan from an
                // earlier agent run). Never do this for a user profile.
                kill_stale_profile_holders(&profile_key).await;
                return retry_launch(&exe, &launch_once, &mut *guard).await;
            }
            match lock_holder_pid(&profile) {
                Some(pid) => {
                    let want = prefs.browser.debug_port;
                    Err(format!(
                        "PROFILE_IN_USE: {exe} is already running with your profile (pid {pid}). \
                         Close that window and retry, or start it with --remote-debugging-port={want} \
                         and Comrade will attach to your live session instead."
                    ))
                }
                None => {
                    // Stale lock files from an unclean shutdown (no live
                    // holder) — clear them and retry once.
                    clear_stale_locks(&profile);
                    retry_launch(&exe, &launch_once, &mut *guard).await
                }
            }
        }
    }
}

/// Spawn, early-exit check, then DevTools wait. Ok = live child.
/// Err is `EXITED:<code>` or `NO_DEVTOOLS:<detail>` for the caller to triage.
async fn spawn_and_settle(exe: &str, args: &[String], port: u16) -> Result<tokio::process::Child, String> {
    let mut child = spawn_browser(exe, args)?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("EXITED:{}", status.code().unwrap_or(-1)));
    }
    if let Err(e) = wait_for_debugger(port).await {
        let _ = child.kill().await;
        return Err(format!("NO_DEVTOOLS:{e}"));
    }
    Ok(child)
}

/// Single retry on a fresh port (used after reclaiming a stale lock).
async fn retry_launch(
    exe: &str,
    launch_once: &(dyn Fn(u16) -> Vec<String> + Sync),
    slot: &mut Option<ManagedBrowser>,
) -> Result<u16, String> {
    let exe_owned = exe.to_string();
    let port = free_port().map_err(|e| format!("PORT_FAILED: {e}"))?;
    let args = launch_once(port);
    match spawn_and_settle(&exe_owned, &args, port).await {
        Ok(child) => {
            let profile_key = args
                .iter()
                .find_map(|a| a.strip_prefix("--user-data-dir="))
                .unwrap_or("")
                .to_string();
            *slot = Some(ManagedBrowser {
                child: Some(child),
                port,
                exe_key: exe_owned,
                profile_key,
            });
            Ok(port)
        }
        Err(code) => Err(friendly_launch_error(&exe_owned, &code)),
    }
}

fn friendly_launch_error(exe: &str, code: &str) -> String {
    if let Some(n) = code.strip_prefix("EXITED:") {
        format!("LAUNCH_FAILED: {exe} exited immediately with {n} (check --no-sandbox / missing libs).")
    } else if let Some(detail) = code.strip_prefix("NO_DEVTOOLS:") {
        format!("LAUNCH_FAILED: {detail}")
    } else {
        format!("LAUNCH_FAILED: {exe}: {code}")
    }
}

/// Spawn the browser; the child is reaped when the agent exits.
fn spawn_browser(exe: &str, args: &[String]) -> Result<tokio::process::Child, String> {
    tokio::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("LAUNCH_FAILED: cannot start {exe}: {e}"))
}

/// Quick existence check for the profile lock (secondary signal next to
/// exit code 21 — some forks differ; lock_holder_pid is the authority).
fn lock_held(profile: &std::path::Path) -> bool {
    profile.join("SingletonLock").exists()
}

/// PID recorded in the profile's SingletonLock (`hostname-pid`), verified
/// alive AND browser-shaped (guards against PID reuse and against locks from
/// non-browser crashes). None = no live holder (lock missing or stale).
fn lock_holder_pid(profile: &std::path::Path) -> Option<u32> {
    let target = std::fs::read_link(profile.join("SingletonLock")).ok()?;
    let pid: u32 = target.to_string_lossy().rsplit('-').next()?.parse().ok()?;
    if pid < 2 {
        return None;
    }
    let cmd = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .to_lowercase()
        .replace('\0', " ");
    const MARKERS: &[&str] = &["brave", "chrome", "chromium", "msedge", "edge", "opera", "vivaldi"];
    if MARKERS.iter().any(|m| cmd.contains(m)) {
        Some(pid)
    } else {
        None
    }
}

/// Remove leftover lock symlinks after an unclean shutdown (only when no
/// live holder exists — see lock_holder_pid). Chrome recreates them.
fn clear_stale_locks(profile: &std::path::Path) {
    for f in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profile.join(f));
    }
}

/// Kill leftover browser processes launched by Comrade against the ISOLATED
/// profile. Never call this for a user profile — only our own
/// `--user-data-dir=<profile>` command lines match, then waits until the
/// SingletonSocket lock clears (Chrome needs a moment to tear down its
/// zygote children after SIGTERM).
async fn kill_stale_profile_holders(profile_key: &str) {
    // pkill -f matches the full command line; scoped to our profile dir so
    // the user's own browser windows are never touched.
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", &format!("--user-data-dir={profile_key}")])
        .output()
        .await;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        let locked = std::path::Path::new(profile_key).join("SingletonSocket").exists()
            && holder_running(profile_key).await;
        if !locked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
}

/// True when a live process still carries our profile dir in its cmdline.
async fn holder_running(profile_key: &str) -> bool {
    tokio::process::Command::new("pgrep")
        .args(["-f", &format!("--user-data-dir={profile_key}")])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn gecko_unsupported(exe: &str) -> String {
    format!(
        "FIREFOX_UNSUPPORTED: \"{exe}\" is a Firefox-family (Gecko) browser. \
         Gecko automation via WebDriver BiDi is planned but not implemented yet — \
         pick a Chromium-family browser (Chrome, Brave, Edge, Chromium, Opera, Vivaldi) in Settings."
    )
}

/// Outcome of detaching from / stopping the managed browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// Killed a browser Comrade launched.
    Closed,
    /// Detached from your live browser (left running — it's yours).
    Detached,
    /// Nothing was connected.
    Idle,
}

/// Stop the managed browser (used by browser.close / tests). Launched
/// instances are killed; attached live sessions are only detached from.
pub async fn close_chromium() -> CloseOutcome {
    let mut guard = managed_lock().lock().await;
    match guard.take() {
        Some(m) if m.child.is_some() => {
            let mut m = m;
            if let Some(mut child) = m.child.take() {
                let _ = child.kill().await;
            }
            CloseOutcome::Closed
        }
        Some(_) => CloseOutcome::Detached,
        None => CloseOutcome::Idle,
    }
}

// ---------------------------------------------------------------------------
// CDP primitives
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct PageTarget {
    #[allow(dead_code)]
    id: String,
    ws_url: String,
    url: String,
}

async fn list_targets(port: u16) -> Result<Vec<PageTarget>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("CDP_HTTP: {e}"))?;
    let resp = client
        .get(format!("{}/json/list", debugger_url(port)))
        .send()
        .await
        .map_err(|e| format!("CDP_HTTP: cannot list targets: {e}"))?;
    let items: Vec<Value> = resp
        .json()
        .await
        .map_err(|e| format!("CDP_HTTP: bad target list: {e}"))?;
    let mut out = Vec::new();
    for item in items {
        let kind = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if kind != "page" {
            continue;
        }
        let (Some(id), Some(ws), Some(url)) = (
            item.get("id").and_then(|v| v.as_str()),
            item.get("webSocketDebuggerUrl").and_then(|v| v.as_str()),
            item.get("url").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        out.push(PageTarget { id: id.to_string(), ws_url: ws.to_string(), url: url.to_string() });
    }
    Ok(out)
}

async fn new_tab(port: u16, url: &str) -> Result<PageTarget, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("CDP_HTTP: {e}"))?;
    let resp = client
        .put(format!("{}/json/new?{}", debugger_url(port), url_query(url)))
        .send()
        .await
        .map_err(|e| format!("CDP_HTTP: cannot open tab: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("OPEN_FAILED: DevTools refused new tab ({}).", resp.status()));
    }
    let item: Value = resp.json().await.map_err(|e| format!("CDP_HTTP: bad new-tab reply: {e}"))?;
    let (Some(id), Some(ws), Some(target_url)) = (
        item.get("id").and_then(|v| v.as_str()),
        item.get("webSocketDebuggerUrl").and_then(|v| v.as_str()),
        item.get("url").and_then(|v| v.as_str()),
    ) else {
        return Err("OPEN_FAILED: DevTools returned a tab without a debugger URL.".to_string());
    };
    Ok(PageTarget { id: id.to_string(), ws_url: ws.to_string(), url: target_url.to_string() })
}

fn url_query(url: &str) -> String {
    // /json/new?URL — value is URL-encoded by hand (no extra deps).
    let mut enc = String::new();
    for b in url.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b'/' | b'?'
            | b'#' | b'[' | b']' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+'
            | b',' | b';' | b'=' | b'%' => enc.push(b as char),
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    enc
}

async fn active_page(port: u16) -> Result<PageTarget, String> {
    let targets = list_targets(port).await?;
    // Prefer a normal page over chrome:// / about:blank shells.
    if let Some(t) = targets.iter().find(|t| !t.url.starts_with("chrome://") && !t.url.starts_with("devtools://")) {
        return Ok(t.clone());
    }
    if let Some(t) = targets.into_iter().next() {
        return Ok(t);
    }
    new_tab(port, "about:blank").await
}

/// Raw CDP round-trip: connect, send one method, wait for the matching id.
pub async fn cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value, String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("CDP_WS: connect failed: {e}"))?;
    let msg = serde_json::json!({ "id": 1, "method": method, "params": params });
    ws.send(tokio_tungstenite::tungstenite::Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| format!("CDP_WS: send failed: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if Instant::now() > deadline {
            return Err("CDP_TIMEOUT: no reply from browser within 20s.".to_string());
        }
        let next = tokio::time::timeout(deadline - Instant::now(), ws.next())
            .await
            .map_err(|_| "CDP_TIMEOUT: no reply from browser within 20s.".to_string())?;
        let Some(Ok(msg)) = next else { continue };
        let text = match msg {
            tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
            _ => continue,
        };
        let Ok(val) = serde_json::from_str::<Value>(&text) else { continue };
        if val.get("id").and_then(|v| v.as_u64()) != Some(1) {
            continue; // event broadcast, not our reply.
        }
        if let Some(err) = val.get("error") {
            return Err(format!("CDP_ERROR: {err}"));
        }
        return Ok(val.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Evaluate JS in the page and return the (JSON-ish) value.
/// Throws inside the page surface as TOOL_RETRYABLE errors.
pub async fn cdp_eval(ws_url: &str, expression: &str) -> Result<Value, String> {
    let result = cdp_call(
        ws_url,
        "Runtime.evaluate",
        serde_json::json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
    )
    .await?;
    if let Some(exc) = result.get("exceptionDetails") {
        let text = exc
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("page threw an exception");
        return Err(format!("PAGE_ERROR: {text}"));
    }
    let res = result.get("result").unwrap_or(&Value::Null);
    if res.get("type").and_then(|t| t.as_str()) == Some("undefined") {
        return Ok(Value::Null);
    }
    Ok(res.get("value").cloned().unwrap_or(Value::Null))
}

async fn wait_ready(ws_url: &str, timeout: Duration) -> Value {
    let start = Instant::now();
    while start.elapsed() < timeout {
        match cdp_eval(ws_url, "document.readyState").await {
            Ok(v) if v.as_str() == Some("complete") => return v,
            _ => tokio::time::sleep(Duration::from_millis(300)).await,
        }
    }
    Value::Null
}

/// Normalize user input into a navigable URL.
pub fn normalize_url(input: &str) -> String {
    let t = input.trim();
    if t.is_empty() {
        return t.to_string();
    }
    if t.starts_with("about:") || t.starts_with("chrome://") {
        return t.to_string();
    }
    if t.contains("://") {
        return t.to_string();
    }
    if !t.contains(' ') && (t.contains('.') || t.contains(':') || t.starts_with("localhost")) {
        return format!("https://{t}");
    }
    // Bare words become a search (keeps "open youtube" style prompts working).
    format!("https://www.google.com/search?q={}", url_query(t))
}

// ---------------------------------------------------------------------------
// High-level page operations used by the browser.* tools
// ---------------------------------------------------------------------------

pub async fn page_navigate(port: u16, url: &str) -> Result<(String, String), String> {
    let page = active_page(port).await?;
    let target = normalize_url(url);
    if target.is_empty() {
        return Err("EMPTY_URL: no URL provided.".to_string());
    }
    let res = cdp_call(&page.ws_url, "Page.navigate", serde_json::json!({ "url": target })).await?;
    if let Some(err) = res.get("errorText").and_then(|e| e.as_str()) {
        return Err(format!("NAVIGATE_FAILED: {err}"));
    }
    wait_ready(&page.ws_url, Duration::from_secs(15)).await;
    let final_url = current_url(port).await.unwrap_or(target.clone());
    Ok((target, final_url))
}

pub async fn current_url(port: u16) -> Result<String, String> {
    let page = active_page(port).await?;
    let v = cdp_eval(&page.ws_url, "location.href").await?;
    Ok(v.as_str().unwrap_or("").to_string())
}

/// Evaluate JS on the active page and return it as a string.
pub async fn cdp_eval_via_active(port: u16, js: &str) -> Result<String, String> {
    let page = active_page(port).await?;
    let v = cdp_eval(&page.ws_url, js).await?;
    Ok(v.as_str().unwrap_or("").to_string())
}

/// Reload the active page and wait until it settles.
pub async fn page_reload(port: u16) -> Result<String, String> {
    let page = active_page(port).await?;
    cdp_call(&page.ws_url, "Page.reload", serde_json::json!({})).await?;
    wait_ready(&page.ws_url, Duration::from_secs(15)).await;
    current_url(port).await
}

pub async fn page_title(port: u16) -> Result<(String, String), String> {
    let page = active_page(port).await?;
    let title = cdp_eval(&page.ws_url, "document.title")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let url = cdp_eval(&page.ws_url, "location.href")
        .await
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    Ok((title, url))
}

pub async fn page_text(port: u16, max_chars: usize) -> Result<(String, String), String> {
    let page = active_page(port).await?;
    let v = cdp_eval(&page.ws_url, "document.documentElement ? document.documentElement.innerText : document.body.innerText").await?;
    let url = current_url(port).await.unwrap_or_default();
    let text = v.as_str().unwrap_or("").to_string();
    let truncated = text.len() > max_chars;
    let out: String = text.chars().take(max_chars).collect();
    let _ = truncated;
    Ok((out, url))
}

pub async fn page_click(port: u16, selector: &str) -> Result<String, String> {
    if selector.trim().is_empty() {
        return Err("EMPTY_SELECTOR: no CSS selector provided.".to_string());
    }
    let page = active_page(port).await?;
    let expr = format!(
        "( () => {{ const el = document.querySelector({sel}); if (!el) return {{ ok: false }}; \
           el.scrollIntoView({{ block: 'center' }}); \
           const r = el.getBoundingClientRect(); \
           el.click(); \
           return {{ ok: true, tag: el.tagName, text: (el.innerText || el.value || '').slice(0,120) }}; }})()",
        sel = js_string(selector)
    );
    let v = cdp_eval(&page.ws_url, &expr).await?;
    if v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
        Ok(v.get("tag").and_then(|t| t.as_str()).unwrap_or("?").to_string())
    } else {
        // Fallback hint: how many candidates exist for a looser match.
        Err(format!("NOT_FOUND: no element matches selector {selector:?}."))
    }
}

pub async fn page_type(port: u16, selector: &str, text: &str) -> Result<(), String> {
    if selector.trim().is_empty() {
        return Err("EMPTY_SELECTOR: no CSS selector provided.".to_string());
    }
    let page = active_page(port).await?;
    let expr = format!(
        "( () => {{ const el = document.querySelector({sel}); if (!el) return {{ ok: false }}; \
           el.focus(); \
           if (el.isContentEditable) {{ el.textContent = {txt}; }} \
           else if ('value' in el) {{ el.value = {txt}; }} \
           else {{ el.textContent = {txt}; }} \
           el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
           el.dispatchEvent(new Event('change', {{ bubbles: true }})); \
           return {{ ok: true }}; }})()",
        sel = js_string(selector),
        txt = js_string(text)
    );
    let v = cdp_eval(&page.ws_url, &expr).await?;
    if v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(format!("NOT_FOUND: no element matches selector {selector:?}."))
    }
}

fn key_codes(key: &str) -> Option<(u8, i32)> {
    Some(match key {
        "Enter" => ("Enter".len() as u8, 13),
        _ => return None,
    })
}

/// Press a key: special keys via CDP Input, text via insertText.
pub async fn page_press(port: u16, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("EMPTY_KEY: no key provided.".to_string());
    }
    let page = active_page(port).await?;
    // Single printable char (or space) → insertText is the faithful path.
    if key.chars().count() == 1 {
        cdp_call(&page.ws_url, "Input.insertText", serde_json::json!({ "text": key })).await?;
        return Ok(());
    }
    // Named keys via raw key events.
    let (code, windows_code) = match key {
        "Enter" => ("Enter", 13),
        "Tab" => ("Tab", 9),
        "Escape" | "Esc" => ("Escape", 27),
        "Backspace" => ("Backspace", 8),
        "Delete" => ("Delete", 46),
        "ArrowLeft" => ("ArrowLeft", 37),
        "ArrowUp" => ("ArrowUp", 38),
        "ArrowRight" => ("ArrowRight", 39),
        "ArrowDown" => ("ArrowDown", 40),
        "Home" => ("Home", 36),
        "End" => ("End", 35),
        "PageUp" => ("PageUp", 33),
        "PageDown" => ("PageDown", 34),
        _ => {
            // "a", "A", "1" handled above; anything else: try it as literal text
            // when it looks like text, else fail with a helpful message.
            if key.len() <= 8 {
                cdp_call(&page.ws_url, "Input.insertText", serde_json::json!({ "text": key })).await?;
                return Ok(());
            }
            return Err(format!(
                "UNKNOWN_KEY: {key:?}. Use a single character or one of Enter, Tab, Escape, Backspace, Delete, ArrowLeft/Up/Right/Down, Home, End, PageUp, PageDown."
            ));
        }
    };
    let _ = key_codes("Enter"); // keep helper referenced for future key maps.
    for kind in ["rawKeyDown", "char", "keyUp"] {
        let mut params = serde_json::json!({ "type": kind, "key": code, "windowsVirtualKeyCode": windows_code });
        if kind != "char" {
            params["code"] = Value::String(code.to_string());
        } else if code == "Enter" {
            params["text"] = Value::String("\r".to_string());
        }
        // keyUp must not carry text.
        if kind == "keyUp" {
            params.as_object_mut().map(|o| o.remove("text"));
        }
        cdp_call(&page.ws_url, "Input.dispatchKeyEvent", params).await?;
    }
    Ok(())
}

pub async fn page_scroll(port: u16, x: i64, y: i64) -> Result<(i64, i64), String> {
    let page = active_page(port).await?;
    let expr = format!(
        "(() => {{ window.scrollBy({x}, {y}); return {{ x: window.scrollX, y: window.scrollY }}; }})()",
    );
    let v = cdp_eval(&page.ws_url, &expr).await?;
    let sx = v.get("x").and_then(|n| n.as_i64()).unwrap_or(0);
    let sy = v.get("y").and_then(|n| n.as_i64()).unwrap_or(0);
    Ok((sx, sy))
}

pub async fn page_screenshot(port: u16) -> Result<std::path::PathBuf, String> {
    let page = active_page(port).await?;
    let res = cdp_call(&page.ws_url, "Page.captureScreenshot", serde_json::json!({ "format": "png" })).await?;
    let b64 = res
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| "SCREENSHOT_FAILED: browser returned no image data.".to_string())?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("SCREENSHOT_FAILED: bad image data: {e}"))?;
    let dir = crate::paths::comrade_home().join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("SCREENSHOT_FAILED: {e}"))?;
    let path = dir.join(format!(
        "browser-{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::write(&path, &bytes).map_err(|e| format!("SCREENSHOT_FAILED: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_detection_covers_forks() {
        assert_eq!(detect_family("/usr/bin/google-chrome-stable"), BrowserFamily::Chromium);
        assert_eq!(detect_family("/usr/bin/brave-origin"), BrowserFamily::Chromium);
        assert_eq!(detect_family("/opt/microsoft/msedge/msedge"), BrowserFamily::Chromium);
        assert_eq!(detect_family("com.brave.Browser"), BrowserFamily::Chromium);
        assert_eq!(detect_family("/usr/bin/firefox"), BrowserFamily::Gecko);
        assert_eq!(detect_family("/opt/zen/zen"), BrowserFamily::Gecko);
        assert_eq!(detect_family("org.mozilla.firefox"), BrowserFamily::Gecko);
        assert_eq!(detect_family("/usr/bin/librewolf"), BrowserFamily::Gecko);
    }

    #[test]
    fn profile_resolution_prefers_real_dirs() {
        // Whatever exists on this machine, resolution must be deterministic:
        // comrade mode always yields the isolated profile.
        let iso = resolve_profile_dir("/usr/bin/brave-origin", "comrade");
        assert!(iso.ends_with("browser-profile"));
        // Unknown forks safely fall back to the isolated profile.
        let unknown = resolve_profile_dir("/usr/bin/some-future-browser", "user");
        assert!(unknown.ends_with("browser-profile"));
        // Known forks resolve into their conventional homes. Channel-aware:
        // brave-origin keeps Brave-Origin (NOT Brave-Browser) — picking the
        // wrong sibling silently yields a fresh logged-out profile.
        let brave = resolve_profile_dir("/usr/bin/brave-origin", "user");
        let s = brave.to_string_lossy().to_string();
        if cfg!(target_os = "linux") {
            assert!(s.ends_with("Brave-Origin"), "got {s}");
            let plain = resolve_profile_dir("/usr/bin/brave", "user").to_string_lossy().to_string();
            assert!(plain.ends_with("Brave-Browser"), "got {plain}");
            let chrome = resolve_profile_dir("/usr/bin/google-chrome-stable", "user")
                .to_string_lossy()
                .to_string();
            assert!(chrome.ends_with("google-chrome"), "got {chrome}");
        } else {
            assert!(s.contains("BraveSoftware") || s.ends_with("browser-profile"), "got {s}");
        }
        // Mode parsing: only "comrade" opts out of the user profile.
        assert!(use_user_profile("user"));
        assert!(use_user_profile(""));
        assert!(use_user_profile("EVERYTHING"));
        assert!(!use_user_profile("comrade"));
        assert!(!use_user_profile("Comrade"));
    }

    #[test]
    fn stale_locks_are_detected_and_cleared() {
        let dir = std::env::temp_dir().join("comrade-lock-test");
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(lock_holder_pid(&dir), None);
        // Garbage target: no live holder.
        #[cfg(unix)]
        std::os::unix::fs::symlink("not-a-lock", dir.join("SingletonLock")).unwrap();
        assert_eq!(lock_holder_pid(&dir), None);
        let _ = std::fs::remove_file(dir.join("SingletonLock"));
        // PID of this test runner is alive but not a browser: PID-reuse guard.
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            format!("testhost-{}", std::process::id()),
            dir.join("SingletonLock"),
        )
        .unwrap();
        assert_eq!(lock_holder_pid(&dir), None);
        std::fs::write(dir.join("SingletonSocket"), "x").unwrap();
        std::fs::write(dir.join("SingletonCookie"), "y").unwrap();
        clear_stale_locks(&dir);
        assert!(!dir.join("SingletonLock").exists());
        assert!(!dir.join("SingletonSocket").exists());
        assert!(!dir.join("SingletonCookie").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_normalization_keeps_nav_working() {
        assert_eq!(normalize_url("https://example.com/x"), "https://example.com/x");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert!(normalize_url("imagine dragons").contains("google.com/search"));
    }
}
