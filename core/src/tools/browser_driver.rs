/*!
 * Browser driver: Comrade's own bundled Chromium, driven over CDP.
 *
 * Architecture:
 *
 * ```text
 * tools/browser.rs  ->  this driver  ->  bundled Chromium (CDP)
 *                             |
 *                   comrade-agent/browser/  (provisioned binary, see
 *                   scripts/fetch-chromium.sh; override: COMRADE_CHROMIUM_BIN)
 *                   comrade-agent/browser-profile/  (single isolated profile)
 * ```
 *
 * There is deliberately exactly one browser: the agent never touches any
 * system browser and never reads any user profile. The bundled Chromium
 * always runs headless (`--headless=new`), so no window ever opens outside
 * the app — the only visible surface is the resizable in-app browser pane,
 * which renders live screenshots of this same instance (same tabs, same
 * session). Tabs are managed via the HTTP DevTools endpoints
 * (`/json/list`, `/json/new`); page automation goes over a per-call
 * WebSocket (`tokio-tungstenite`). One tab is reused across navigation,
 * reading, and clicking.
 */

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Managed bundled-Chromium process (singleton)
// ---------------------------------------------------------------------------

struct ManagedBrowser {
    /// Always launched by us (killed on close). Nothing is ever attached to:
    /// there is no user browser to attach to by design.
    child: Option<tokio::process::Child>,
    port: u16,
    page_id: Option<String>,
}

#[derive(Default)]
struct BrowserState {
    browser: Option<ManagedBrowser>,
    failed_launch: Option<(String, Instant)>,
}

static MANAGED: OnceLock<tokio::sync::Mutex<BrowserState>> = OnceLock::new();

fn managed_lock() -> &'static tokio::sync::Mutex<BrowserState> {
    MANAGED.get_or_init(|| tokio::sync::Mutex::new(BrowserState::default()))
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

/// Resolve the bundled Chromium executable, installing it first if needed.
/// The browser is a core feature: it provisions itself automatically on
/// first use (one-time download into `comrade-agent/browser/`).
pub async fn bundled_exe() -> Result<std::path::PathBuf, String> {
    super::provision::ensure_provisioned().await
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

/// Headless always: the Chromium window must never appear outside the app.
/// What the user sees is the in-app pane (live screenshots of this process).
fn launch_args(port: u16, profile: &str) -> Vec<String> {
    vec![
        format!("--remote-debugging-port={port}"),
        "--remote-allow-origins=*".to_string(),
        format!("--user-data-dir={profile}"),
        "--headless=new".to_string(),
        "--disable-gpu".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--no-sandbox".to_string(),
        "--window-size=1280,860".to_string(),
        // A URL here would open a tab on every launch; pages are created
        // only after connecting, via active_page().
        "--no-startup-window".to_string(),
    ]
}

/// Ensure Comrade's bundled Chromium is running and return its DevTools port.
/// Reuses the live instance; otherwise launches it headless against the
/// single isolated profile. Never touches system browsers or user profiles.
pub async fn ensure_chromium() -> Result<u16, String> {
    let exe = bundled_exe().await?;
    let exe_key = exe.to_string_lossy().to_string();
    let profile = profile_dir();
    let profile_key = profile.to_string_lossy().to_string();
    let mut guard = managed_lock().lock().await;

    // Reuse the live instance while its process is still ours and answering.
    if let Some(m) = guard.browser.as_mut() {
        let usable = match m.child.as_mut() {
            Some(child) => child.try_wait().map(|s| s.is_none()).unwrap_or(false),
            None => false,
        };
        if usable && http_ok(&format!("{}/json/version", debugger_url(m.port))).await {
            return Ok(m.port);
        }
        if let Some(mut child) = m.child.take() {
            let _ = child.kill().await;
        }
        guard.browser = None;
    }

    if let Some((error, when)) = &guard.failed_launch {
        if when.elapsed() < Duration::from_secs(30) {
            return Err(error.clone());
        }
    }
    guard.failed_launch = None;
    if let Err(e) = std::fs::create_dir_all(&profile) {
        return Err(format!("PROFILE_FAILED: cannot create browser profile: {e}"));
    }

    let port = free_port().map_err(|e| format!("PORT_FAILED: {e}"))?;
    let result = match spawn_and_settle(&exe_key, &launch_args(port, &profile_key), port).await {
        Ok(child) => {
            guard.browser = Some(ManagedBrowser { child: Some(child), port, page_id: None });
            Ok(port)
        }
        Err(code) => {
            let locked_exit = code.starts_with("EXITED:21") || lock_held(&profile);
            let hung = code.starts_with("NO_DEVTOOLS");
            if locked_exit || hung {
                // Our isolated profile is ours to reclaim (orphan from an
                // earlier agent run).
                kill_stale_profile_holders(&profile_key).await;
                if lock_holder_pid(&profile).is_none() {
                    clear_stale_locks(&profile);
                }
                retry_launch(&exe_key, &profile_key, &mut guard.browser).await
            } else {
                Err(friendly_launch_error(&exe_key, &code))
            }
        }
    };
    if let Err(error) = &result {
        guard.failed_launch = Some((error.clone(), Instant::now()));
    }
    result
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
    profile_key: &str,
    slot: &mut Option<ManagedBrowser>,
) -> Result<u16, String> {
    let exe_owned = exe.to_string();
    let port = free_port().map_err(|e| format!("PORT_FAILED: {e}"))?;
    let args = launch_args(port, profile_key);
    match spawn_and_settle(&exe_owned, &args, port).await {
        Ok(child) => {
            *slot = Some(ManagedBrowser { child: Some(child), port, page_id: None });
            Ok(port)
        }
        Err(code) => Err(friendly_launch_error(&exe_owned, &code)),
    }
}

fn friendly_launch_error(exe: &str, code: &str) -> String {
    if let Some(n) = code.strip_prefix("EXITED:") {
        format!("LAUNCH_FAILED: bundled Chromium exited immediately with {n} ({exe}; check missing libs).")
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
        .map_err(|e| format!("LAUNCH_FAILED: cannot start bundled Chromium ({exe}): {e}"))
}

/// Quick existence check for the profile lock.
fn lock_held(profile: &std::path::Path) -> bool {
    // SingletonLock is a hostname-pid symlink, not a filesystem target.
    std::fs::symlink_metadata(profile.join("SingletonLock")).is_ok()
}

/// PID recorded in the profile's SingletonLock (`hostname-pid`), verified
/// alive AND browser-shaped (guards against PID reuse). None = no live holder.
fn lock_holder_pid(profile: &std::path::Path) -> Option<u32> {
    let target = std::fs::read_link(profile.join("SingletonLock")).ok()?;
    let pid: u32 = target.to_string_lossy().rsplit('-').next()?.parse().ok()?;
    if pid < 2 {
        return None;
    }
    let cmd = process_command(pid).to_lowercase();
    const MARKERS: &[&str] = &["chrome", "chromium", "headless_shell"];
    if MARKERS.iter().any(|m| cmd.contains(m)) {
        Some(pid)
    } else {
        None
    }
}

fn process_command(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    if let Ok(command) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
        return command.replace('\0', " ");
    }

    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Remove leftover lock symlinks after an unclean shutdown (only when no
/// live holder exists — see lock_holder_pid). Chrome recreates them.
fn clear_stale_locks(profile: &std::path::Path) {
    for f in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profile.join(f));
    }
}

/// Kill leftover processes launched against the ISOLATED profile only.
/// Scoped to our `--user-data-dir=<profile>` so nothing else is touched.
async fn kill_stale_profile_holders(profile_key: &str) {
    // pkill -f matches the full command line; scoped to our profile dir.
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

/// Outcome of stopping the managed browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// Killed the bundled Chromium Comrade launched.
    Closed,
    /// Kept for API compatibility; the bundled browser is always launched,
    /// never attached, so detaching cannot happen.
    Detached,
    /// Nothing was connected.
    Idle,
}

/// Stop the managed browser (used by browser.close / tests).
pub async fn close_chromium() -> CloseOutcome {
    let mut guard = managed_lock().lock().await;
    guard.failed_launch = None;
    match guard.browser.take() {
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

/// DevTools port of the running instance, if any.
pub async fn managed_port() -> Option<u16> {
    let guard = managed_lock().lock().await;
    let m = guard.browser.as_ref()?;
    m.child.is_some().then_some(m.port)
}

/// Best-effort snapshot for the in-app pane: running flag + current URL/title.
/// Never fails — the pane renders placeholders when the browser is down.
pub async fn state_snapshot() -> serde_json::Value {
    let Some(port) = managed_port().await else {
        return serde_json::json!({ "running": false, "url": "", "title": "" });
    };
    if !http_ok(&format!("{}/json/version", debugger_url(port))).await {
        return serde_json::json!({ "running": false, "url": "", "title": "" });
    }
    let url = current_url(port).await.unwrap_or_default();
    let title = page_title(port).await.map(|(t, _)| t).unwrap_or_default();
    serde_json::json!({ "running": true, "url": url, "title": title })
}

/// Raw PNG bytes of the current tab (for the in-app pane live view).
pub async fn screenshot_bytes() -> Result<Vec<u8>, String> {
    let port = ensure_chromium().await?;
    let page = active_page(port).await?;
    let res = cdp_call(&page.ws_url, "Page.captureScreenshot", serde_json::json!({ "format": "png" })).await?;
    let b64 = res
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| "SCREENSHOT_FAILED: browser returned no image data.".to_string())?;
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("SCREENSHOT_FAILED: bad image data: {e}"))
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
    // Keep one target throughout navigation, reads and clicks, even when
    // /json/list changes order or a different tab is opened in the browser.
    // This lock also prevents concurrent first calls from creating two tabs.
    let mut guard = managed_lock().lock().await;
    let session = guard.browser.as_mut().filter(|m| m.port == port);
    let targets = list_targets(port).await?;
    let remembered = session.as_ref().and_then(|m| m.page_id.as_deref());
    let target = match choose_page(&targets, remembered) {
        Some(target) => target.clone(),
        None => new_tab(port, "about:blank").await?,
    };
    if let Some(session) = session {
        session.page_id = Some(target.id.clone());
    }
    Ok(target)
}

fn choose_page<'a>(targets: &'a [PageTarget], remembered: Option<&str>) -> Option<&'a PageTarget> {
    remembered.and_then(|id| targets.iter().find(|t| t.id == id))
        .or_else(|| targets.iter().find(|t| !t.url.starts_with("chrome://")
            && !t.url.starts_with("devtools://") && t.url != "about:blank"))
        .or_else(|| targets.first())
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

    #[tokio::test]
    async fn bundled_exe_honors_override() {
        let exe = std::env::current_exe().unwrap();
        std::env::set_var("COMRADE_CHROMIUM_BIN", exe.to_string_lossy().to_string());
        assert!(bundled_exe().await.is_ok());
        std::env::remove_var("COMRADE_CHROMIUM_BIN");
    }

    #[test]
    fn install_failure_keeps_setup_code() {
        // The agent stops the task on these codes instead of falling back to
        // another browser: the built-in browser installs itself, and when the
        // install itself fails (offline) the task reports it honestly.
        let msg = super::super::provision::status_json();
        assert!(msg.get("phase").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn launch_is_always_headless_and_isolated() {
        let args = launch_args(9334, "/tmp/comrade-test-profile");
        assert!(args.iter().all(|arg| arg.starts_with("--")), "no startup URL may be forwarded: {args:?}");
        assert!(args.iter().any(|arg| arg == "--no-startup-window"));
        assert!(args.iter().any(|arg| arg == "--headless=new"), "nothing may open outside the app: {args:?}");
        assert!(args.iter().any(|arg| arg.starts_with("--user-data-dir=/tmp/comrade-test-profile")));
        assert!(!args.iter().any(|arg| arg.contains("remote-debugging-port=0")));
    }

    #[test]
    fn stale_locks_are_detected_and_cleared() {
        let dir = std::env::temp_dir().join("comrade-lock-test");
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(lock_holder_pid(&dir), None);
        #[cfg(unix)]
        std::os::unix::fs::symlink("not-a-lock", dir.join("SingletonLock")).unwrap();
        #[cfg(unix)]
        assert!(lock_held(&dir), "SingletonLock symlinks need not point to a file");
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

    #[test]
    fn page_selection_stays_on_same_target_when_another_tab_opens() {
        let page = |id: &str, url: &str| PageTarget { id: id.into(), url: url.into(), ws_url: String::new() };
        let targets = vec![
            page("blank", "about:blank"),
            page("unrelated", "https://example.com/unrelated"),
            page("owned", "https://example.com/owned"),
        ];
        assert_eq!(choose_page(&targets, Some("owned")).unwrap().id, "owned");
        assert_eq!(choose_page(&targets, Some("closed")).unwrap().id, "unrelated");
        assert_eq!(choose_page(&targets, None).unwrap().id, "unrelated");
        assert_eq!(choose_page(&targets[..1], None).unwrap().id, "blank");
        assert!(choose_page(&[], None).is_none());
    }
}
