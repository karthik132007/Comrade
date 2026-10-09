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
 * which receives live compositor frames of this same instance (same tabs, same
 * session), streamed via CDP with decode acknowledgements. Tabs are managed
 * via the HTTP DevTools endpoints (`/json/list`, `/json/new`); page automation
 * uses a persistent, multiplexed WebSocket (`tokio-tungstenite`). One tab is
 * reused across navigation, reading, and clicking.
 */

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

pub const DEFAULT_HOME_URL: &str = "https://duckduckgo.com/";

// ---------------------------------------------------------------------------
// Managed bundled-Chromium process (singleton)
// ---------------------------------------------------------------------------

struct ManagedBrowser {
    /// Launched by us (killed on close) vs adopted healthy orphan from an
    /// earlier app run (never killed by us — detached on close, re-adopted
    /// on next launch).
    child: Option<tokio::process::Child>,
    port: u16,
    /// Absolute path of our bundled executable (ownership checks).
    exe: String,
    page_id: Option<String>,
    page: Option<PageTarget>,
    checked_at: Instant,
}

#[derive(Default)]
struct BrowserState {
    browser: Option<ManagedBrowser>,
    failed_launch: Option<(String, Instant)>,
    tab_order: Vec<String>,
}

static MANAGED: OnceLock<tokio::sync::Mutex<BrowserState>> = OnceLock::new();

fn managed_lock() -> &'static tokio::sync::Mutex<BrowserState> {
    MANAGED.get_or_init(|| tokio::sync::Mutex::new(BrowserState::default()))
}

/// Adopt a healthy live instance of OUR browser holding OUR profile lock —
/// typically an orphan that outlived its app process (app restart, crash).
/// Only adopts when the lock holder's command line proves it is our own
/// bundled executable running our own profile; anything else is refused.
/// Returns its DevTools port, read from Chrome's DevToolsActivePort file.
async fn adopt_orphan(exe_key: &str, profile: &std::path::Path) -> Option<u16> {
    let pid = holder_is_ours(exe_key, profile)?;
    // Preferred: the port the holder itself was started with.
    let cmd = process_command(pid);
    if let Some(port) = debug_port_from_cmd(&cmd) {
        if http_ok(&format!("{}/json/version", debugger_url(port))).await {
            return Some(port);
        }
    }
    // Fallback: Chrome's DevToolsActivePort file in the profile dir.
    let content = std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    let port: u16 = content.lines().next()?.trim().parse().ok()?;
    http_ok(&format!("{}/json/version", debugger_url(port))).await.then_some(port)
}

/// `--remote-debugging-port=NNN` from a holder command line, if present.
fn debug_port_from_cmd(cmd: &str) -> Option<u16> {
    cmd.split_whitespace()
        .find_map(|arg| arg.strip_prefix("--remote-debugging-port="))
        .and_then(|n| n.parse().ok())
        .filter(|&p| p != 0)
}

/// The process holding the profile lock, if it is verifiably our own
/// bundled browser on our own profile. None otherwise (stale lock, or
/// something we must never touch).
fn holder_is_ours(exe_key: &str, profile: &std::path::Path) -> Option<u32> {
    #[cfg(windows)]
    return windows_managed_processes(exe_key, &profile.to_string_lossy(), false).first().map(|process| process.pid);
    #[cfg(not(windows))]
    {
    let pid = lock_holder_pid(profile)?;
    let cmd = process_command(pid);
    let profile_arg = format!("--user-data-dir={}", profile.to_string_lossy());
    (cmd.contains(exe_key) && cmd.contains(&profile_arg)).then_some(pid)
    }
}

/// An adopted (not launched) instance is reusable while its process is still
/// ours and its DevTools endpoint still answers.
async fn adopted_alive(exe_key: &str, profile: &std::path::Path, port: u16) -> bool {
    holder_is_ours(exe_key, profile).is_some()
        && http_ok(&format!("{}/json/version", debugger_url(port))).await
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

fn devtools_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(5)).build().expect("DevTools HTTP client"))
}

async fn http_ok(url: &str) -> bool {
    let client = devtools_client();
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
        // The streamed page has no native visible window; keep its renderer
        // responsive instead of lowering its priority because it is occluded.
        "--disable-backgrounding-occluded-windows".to_string(),
        "--disable-renderer-backgrounding".to_string(),
        "--enable-unsafe-extension-debugging".to_string(),
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
    let port = ensure_process().await?;
    super::browser_extension::ensure_loaded(port).await
        .map_err(|e| format!("{e}. Extension support requires Comrade's current bundled Chromium."))?;
    Ok(port)
}

async fn ensure_process() -> Result<u16, String> {
    let exe = bundled_exe().await?;
    let exe_key = exe.to_string_lossy().to_string();
    let profile = profile_dir();
    let profile_key = profile.to_string_lossy().to_string();
    // Option-wrapped so the self-heal path can release the lock before the
    // (possibly long) re-provision + relaunch.
    let mut slot = Some(managed_lock().lock().await);
    let guard = slot.as_mut().unwrap();

    // Reuse the live instance while its process is still ours and answering
    // (launched children via wait(); adopted orphans via ownership + HTTP).
    if let Some(m) = guard.browser.as_mut() {
        let usable = match m.child.as_mut() {
            Some(child) => {
                child.try_wait().map(|s| s.is_none()).unwrap_or(false)
                    && (m.checked_at.elapsed() < Duration::from_secs(2)
                        || http_ok(&format!("{}/json/version", debugger_url(m.port))).await)
            }
            None => m.checked_at.elapsed() < Duration::from_secs(2)
                || adopted_alive(&m.exe, &profile, m.port).await,
        };
        if usable {
            if m.checked_at.elapsed() >= Duration::from_secs(2) { m.checked_at = Instant::now(); }
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

    // Fast path for the common restart case: our previous browser is still
    // alive and holding the profile (healthy orphan). Adopt it instead of
    // spawning into a locked profile and fighting over it.
    if cfg!(windows) || lock_held(&profile) {
        if let Some(port) = adopt_orphan(&exe_key, &profile).await {
            guard.browser = Some(ManagedBrowser { child: None, exe: exe_key, port, page_id: None, page: None, checked_at: Instant::now() });
            return Ok(port);
        }
    }

    let port = free_port().map_err(|e| format!("PORT_FAILED: {e}"))?;
    let result = match spawn_and_settle(&exe_key, &launch_args(port, &profile_key), port).await {
        Ok(child) => {
            guard.browser = Some(ManagedBrowser { child: Some(child), exe: exe_key, port, page_id: None, page: None, checked_at: Instant::now() });
            Ok(port)
        }
        Err(code) => {
            let locked_exit = code.starts_with("EXITED:21") || lock_held(&profile)
                || (cfg!(windows) && holder_is_ours(&exe_key, &profile).is_some());
            let hung = code.starts_with("NO_DEVTOOLS");
            if locked_exit || hung {
                // Prefer adoption again (holder may have appeared since):
                // only a dead or unreachable holder gets killed + relaunched.
                if let Some(port) = adopt_orphan(&exe_key, &profile).await {
                    guard.browser = Some(ManagedBrowser { child: None, exe: exe_key, port, page_id: None, page: None, checked_at: Instant::now() });
                    return Ok(port);
                }
                kill_stale_profile_holders(&exe_key, &profile_key).await;
                if lock_holder_pid(&profile).is_none() {
                    clear_stale_locks(&profile);
                }
                retry_launch(&exe_key, &profile_key, &mut guard.browser).await
            } else if code.starts_with("EXITED") && super::provision::quarantine_owned(&exe_key) {
                // A binary we installed ourselves that dies instantly is
                // corrupt or an incompatible legacy build (e.g. the retired
                // headless-shell): quarantine it, re-provision once, retry.
                // NLL: `guard` is not used after this point in the arm.
                slot.take();
                let exe = bundled_exe().await?;
                let exe_key = exe.to_string_lossy().to_string();
                let mut healed = managed_lock().lock().await;
                healed.failed_launch = None;
                let port = free_port().map_err(|e| format!("PORT_FAILED: {e}"))?;
                match spawn_and_settle(&exe_key, &launch_args(port, &profile_key), port).await {
                    Ok(mut child) => {
                        // Another task may have launched while we re-provisioned:
                        // prefer the instance already in the slot when live.
                        let slot_live = healed.browser.as_mut().is_some_and(|m| {
                            m.child.as_mut().map(|c| c.try_wait().map(|s| s.is_none()).unwrap_or(false)).unwrap_or(false)
                        });
                        if slot_live {
                            let _ = child.kill().await;
                            Ok(healed.browser.as_ref().map(|m| m.port).unwrap_or(port))
                        } else {
                            healed.browser = Some(ManagedBrowser { child: Some(child), exe: exe_key, port, page_id: None, page: None, checked_at: Instant::now() });
                            Ok(port)
                        }
                    }
                    Err(code) => Err(friendly_launch_error(&exe_key, &code)),
                }
            } else {
                Err(friendly_launch_error(&exe_key, &code))
            }
        }
    };
    // The self-heal arm returned already (and released the lock via slot.take()).
    if let (Err(error), Some(guard)) = (&result, slot.as_mut()) {
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
            *slot = Some(ManagedBrowser { child: Some(child), exe: exe_owned, port, page_id: None, page: None, checked_at: Instant::now() });
            Ok(port)
        }
        Err(code) => Err(friendly_launch_error(&exe_owned, &code)),
    }
}

fn friendly_launch_error(exe: &str, code: &str) -> String {
    if let Some(n) = code.strip_prefix("EXITED:") {
        // 21 almost always means the profile is still locked: name the
        // holder so the message is actionable instead of cryptic.
        if n == "21" {
            let holder = lock_holder_pid(&profile_dir())
                .map(|pid| format!(" (held by live process pid {pid})"))
                .unwrap_or_default();
            return format!(
                "LAUNCH_FAILED: bundled Chromium exited with 21{holder}: its profile is still in use. \
                 Close any other Comrade window holding the browser, then retry — a healthy previous \
                 instance is adopted automatically, otherwise it is reclaimed."
            );
        }
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
    #[cfg(windows)]
    {
        return windows_powershell()
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
                "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $p=Get-CimInstance Win32_Process -Filter ('ProcessId=' + $env:COMRADE_QUERY_PID); ConvertTo-Json -Compress -InputObject ([string]$p.CommandLine)"])
            .env("COMRADE_QUERY_PID", pid.to_string())
            .output().ok().filter(|output| output.status.success())
            .and_then(|output| serde_json::from_slice::<String>(&output.stdout).ok())
            .unwrap_or_default();
    }
    #[cfg(target_os = "linux")]
    if let Ok(command) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
        return command.replace('\0', " ");
    }

    #[cfg(not(windows))]
    { std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default() }
}

#[cfg(windows)]
fn windows_powershell() -> std::process::Command {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::process::Command::new(std::path::PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe"))
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct WindowsManagedProcess {
    pid: u32,
}

/// Windows Chromium uses a named mutex rather than Unix SingletonLock
/// symlinks. Read native process identities instead; never match a substring
/// of an executable or profile (e.g. browser-profile-other).
#[cfg(windows)]
fn windows_managed_processes(exe: &str, profile: &str, terminate: bool) -> Vec<WindowsManagedProcess> {
    const SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new()
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ComradeCommandLine {
    [DllImport("shell32.dll", SetLastError=true)]
    static extern IntPtr CommandLineToArgvW([MarshalAs(UnmanagedType.LPWStr)] string line, out int count);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr memory);
    public static string[] Args(string line) {
        int count; IntPtr memory=CommandLineToArgvW(line, out count);
        if(memory==IntPtr.Zero) return new string[0];
        try {
            string[] args=new string[count];
            for(int i=0;i<count;i++) args[i]=Marshal.PtrToStringUni(Marshal.ReadIntPtr(memory,i*IntPtr.Size));
            return args;
        } finally { LocalFree(memory); }
    }
}
'@
function ExecutableIdentity([string]$path) {
    # Canonical Rust paths may use the extended-length representation while
    # CIM returns the ordinary representation. Strip only a real drive/UNC
    # prefix, never arbitrary device namespaces or substrings of a path.
    if($path.StartsWith('\\?\UNC\',[StringComparison]::OrdinalIgnoreCase)) {
        $path='\\'+$path.Substring(8)
    } elseif($path.Length -ge 7 -and $path.StartsWith('\\?\',[StringComparison]::Ordinal) -and
        [char]::IsLetter($path[4]) -and $path[5] -eq ':' -and $path[6] -eq '\') {
        $path=$path.Substring(4)
    }
    return [IO.Path]::GetFullPath($path)
}
function IsManaged($p) {
    if(-not $p.ExecutablePath -or -not $p.CommandLine) { return $false }
    $actualExe=ExecutableIdentity $p.ExecutablePath
    $expectedExe=ExecutableIdentity $env:COMRADE_QUERY_EXE
    if(-not [string]::Equals($actualExe,$expectedExe,[StringComparison]::OrdinalIgnoreCase)) { return $false }
    $argv=[ComradeCommandLine]::Args($p.CommandLine)
    $found=$false
    for($i=1;$i -lt $argv.Length;$i++) {
        $value=$null
        if($argv[$i].StartsWith('--user-data-dir=',[StringComparison]::Ordinal)) { $value=$argv[$i].Substring(16) }
        elseif($argv[$i] -ceq '--user-data-dir' -and ($i+1) -lt $argv.Length) { $i++; $value=$argv[$i] }
        if($null -ne $value) {
            if(-not [string]::Equals($value,$env:COMRADE_QUERY_PROFILE,[StringComparison]::OrdinalIgnoreCase)) { return $false }
            $found=$true
        }
    }
    return $found
}
$ownedProcesses=@(Get-CimInstance Win32_Process | Where-Object { IsManaged $_ })
if($env:COMRADE_QUERY_TERMINATE -eq '1') {
    foreach($p in $ownedProcesses) {
        # Recheck identity and creation time before termination to reject a
        # reused PID; the native CIM method targets this process instance.
        $live=Get-CimInstance Win32_Process -Filter ('ProcessId='+$p.ProcessId)
        if($live -and $p.CreationDate -and $live.CreationDate -eq $p.CreationDate -and (IsManaged $live)) {
            $null=Invoke-CimMethod -InputObject $live -MethodName Terminate
        }
    }
}
ConvertTo-Json -Compress -InputObject @($ownedProcesses | ForEach-Object { @{pid=[uint32]$_.ProcessId} })
"#;
    windows_powershell()
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        // Values travel as environment data, never interpolated PowerShell.
        .env("COMRADE_QUERY_EXE", exe)
        .env("COMRADE_QUERY_PROFILE", profile)
        .env("COMRADE_QUERY_TERMINATE", if terminate { "1" } else { "0" })
        .output().ok().filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice(&output.stdout).ok())
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
async fn kill_stale_profile_holders(_exe_key: &str, profile_key: &str) {
    #[cfg(windows)]
    {
        windows_managed_processes(_exe_key, profile_key, true);
    }
    // pkill -f matches the full command line; scoped to our profile dir.
    #[cfg(not(windows))]
    let _ = tokio::process::Command::new("pkill")
        .args(["-f", &format!("--user-data-dir={profile_key}")])
        .output()
        .await;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        #[cfg(windows)]
        let locked = holder_running(_exe_key, profile_key).await;
        #[cfg(not(windows))]
        let locked = std::path::Path::new(profile_key).join("SingletonSocket").exists()
            && holder_running(_exe_key, profile_key).await;
        if !locked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
}

/// True when a live process still carries our profile dir in its cmdline.
async fn holder_running(_exe_key: &str, profile_key: &str) -> bool {
    #[cfg(windows)]
    return !windows_managed_processes(_exe_key, profile_key, false).is_empty();
    #[cfg(not(windows))]
    {
    tokio::process::Command::new("pgrep")
        .args(["-f", &format!("--user-data-dir={profile_key}")])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
    }
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
    if let Some(browser) = guard.browser.as_ref() { super::browser_extension::forget(browser.port).await; }
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

/// DevTools port of the running instance, if any (launched or adopted).
pub async fn managed_port() -> Option<u16> {
    let guard = managed_lock().lock().await;
    let m = guard.browser.as_ref()?;
    match m.child.is_some() {
        true => Some(m.port),
        // Don't await network while holding the lock: copy out and check after.
        false => {
            let (exe, port) = (m.exe.clone(), m.port);
            drop(guard);
            adopted_alive(&exe, &profile_dir(), port).await.then_some(port)
        }
    }
}

/// Input is already scoped to an initialized browser. Avoid filesystem,
/// provisioning and extension checks on every keystroke and pointer event.
/// Failed CDP connections invalidate the cached page and force full recovery.
pub async fn interactive_port() -> Result<u16, String> {
    {
        let mut guard = managed_lock().lock().await;
        if let Some(browser) = guard.browser.as_mut().filter(|browser| browser.page.is_some()) {
            let alive = match browser.child.as_mut() {
                Some(child) => child.try_wait().is_ok_and(|status| status.is_none()),
                None => true,
            };
            if alive { return Ok(browser.port); }
        }
    }
    ensure_chromium().await
}

/// Best-effort snapshot for the in-app pane: running flag + current URL/title.
/// Never fails — the pane renders placeholders when the browser is down.
pub async fn state_snapshot() -> serde_json::Value {
    let Some(port) = managed_port().await else {
        return serde_json::json!({ "running": false, "url": "", "title": "" });
    };
    let Ok(tabs) = tabs(port).await else {
        return serde_json::json!({ "running": false, "url": "", "title": "" });
    };
    let active = tabs.iter().find(|tab| tab["active"] == true);
    serde_json::json!({
        "running": true,
        "url": active.map(|tab| &tab["url"]),
        "title": active.map(|tab| &tab["title"]),
        "tabs": tabs,
    })
}

/// Initialize a page without asking its JavaScript thread for metadata.
pub async fn pane_snapshot(port: u16) -> Result<Value, String> {
    active_page(port).await?;
    Ok(state_snapshot().await)
}

/// User-visible browser tabs, backed by Chromium page targets.
pub async fn tabs(port: u16) -> Result<Vec<Value>, String> {
    let targets = ordered_targets(port).await?;
    let active_id = {
        let guard = managed_lock().lock().await;
        guard.browser.as_ref().and_then(|browser| browser.page_id.clone())
    };
    let mut tabs = Vec::with_capacity(targets.len());
    for target in targets {
        // /json/list already supplies metadata without waiting for any page's
        // JS main thread. Never evaluate all background tabs during UI polling.
        tabs.push(serde_json::json!({
            "id": target.id,
            "url": target.url,
            "title": target.title,
            "favicon_url": target.favicon_url,
            "active": active_id.as_deref() == Some(target.id.as_str()),
        }));
    }
    Ok(tabs)
}

pub async fn create_tab(port: u16, url: &str) -> Result<Value, String> {
    ordered_targets(port).await?;
    let target = new_tab(port, &normalize_url(url)).await?;
    select_tab(port, &target.id).await
}

pub async fn select_tab(port: u16, id: &str) -> Result<Value, String> {
    let target = ordered_targets(port).await?.into_iter().find(|target| target.id == id)
        .ok_or_else(|| "TAB_NOT_FOUND: that browser tab is no longer open.".to_string())?;
    let _ = devtools_client().get(format!("{}/json/activate/{}", debugger_url(port), target.id)).send().await;
    let mut guard = managed_lock().lock().await;
    let browser = guard.browser.as_mut().filter(|browser| browser.port == port)
        .ok_or_else(|| "BROWSER_CLOSED: the browser is not running.".to_string())?;
    browser.page_id = Some(target.id.clone());
    browser.page = Some(target.clone());
    drop(guard);
    Ok(serde_json::json!({ "id": target.id, "url": target.url, "title": target.title }))
}

pub async fn close_tab(port: u16, id: &str) -> Result<Value, String> {
    let targets = ordered_targets(port).await?;
    let active_id = managed_lock().lock().await.browser.as_ref().and_then(|browser| browser.page_id.clone());
    let closed_index = targets.iter().position(|target| target.id == id)
        .ok_or_else(|| "TAB_NOT_FOUND: that browser tab is no longer open.".to_string())?;
    let response = devtools_client().get(format!("{}/json/close/{}", debugger_url(port), id)).send().await
        .map_err(|e| format!("CLOSE_FAILED: cannot close tab: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("CLOSE_FAILED: DevTools refused to close tab ({}).", response.status()));
    }
    let remaining: Vec<PageTarget> = targets.into_iter().filter(|target| target.id != id).collect();
    // Closing a background tab must not switch the page the user is viewing.
    if let Some(active) = remaining.iter().find(|target| Some(&target.id) == active_id.as_ref()) {
        return Ok(serde_json::json!({ "id": active.id, "url": active.url, "title": active.title }));
    }
    let next = match remaining.get(closed_index.min(remaining.len().saturating_sub(1))) {
        Some(target) => target.clone(),
        None => new_tab(port, DEFAULT_HOME_URL).await?,
    };
    select_tab(port, &next.id).await
}

/// Raw PNG bytes of the current tab (for the in-app pane live view).
pub async fn screenshot_bytes() -> Result<Vec<u8>, String> {
    Ok(capture_frame().await?.png)
}

/// One live frame for the interactive in-app pane: screenshot PNG plus the
/// page's CSS viewport size, so UI clicks/scrolls map 1:1 onto the page.
pub struct Frame {
    pub png: Vec<u8>,
    pub viewport_w: f64,
    pub viewport_h: f64,
}

pub async fn capture_frame() -> Result<Frame, String> {
    let port = ensure_chromium().await?;
    let (viewport_w, viewport_h) = viewport_metrics(port).await.unwrap_or((1280.0, 860.0));
    let page = active_page(port).await?;
    let res = cdp_call(&page.ws_url, "Page.captureScreenshot", serde_json::json!({ "format": "png" })).await?;
    let b64 = res
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| "SCREENSHOT_FAILED: browser returned no image data.".to_string())?;
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("SCREENSHOT_FAILED: bad image data: {e}"))?;
    Ok(Frame { png, viewport_w, viewport_h })
}

/// CSS viewport size (`window.innerWidth/innerHeight`) of the active tab.
/// The pane scales image clicks by these so they land on the right element.
pub async fn viewport_metrics(port: u16) -> Result<(f64, f64), String> {
    let page = active_page(port).await?;
    let v = cdp_eval(&page.ws_url, "({w: window.innerWidth, h: window.innerHeight})").await?;
    let w = v.get("w").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let h = v.get("h").and_then(|n| n.as_f64()).unwrap_or(0.0);
    if w > 0.0 && h > 0.0 {
        Ok((w, h))
    } else {
        Err("VIEWPORT_UNKNOWN: could not read the page size.".to_string())
    }
}

/// Click at CSS-pixel coordinates in the active tab (from the in-app pane).
/// Real trusted input via CDP: hits buttons, links, and focuses fields.
pub async fn mouse_click(port: u16, x: f64, y: f64) -> Result<(), String> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x > 10000.0 || y > 10000.0 {
        return Err(format!("BAD_COORDS: click position out of range ({x}, {y})."));
    }
    let page = active_page(port).await?;
    let base = serde_json::json!({ "x": x, "y": y });
    let mut moved = base.clone();
    moved["type"] = serde_json::Value::String("mouseMoved".into());
    cdp_call(&page.ws_url, "Input.dispatchMouseEvent", moved).await?;
    for kind in ["mousePressed", "mouseReleased"] {
        let mut ev = base.clone();
        ev["type"] = serde_json::Value::String(kind.into());
        ev["button"] = serde_json::Value::String("left".into());
        ev["clickCount"] = serde_json::Value::Number(1.into());
        cdp_call(&page.ws_url, "Input.dispatchMouseEvent", ev).await?;
    }
    Ok(())
}

/// Type text into whatever is focused in the active tab (in-app pane keys).
pub async fn insert_text(port: u16, text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    if text.len() > 65536 {
        return Err("TEXT_TOO_LONG: paste at most 64 KiB at a time.".to_string());
    }
    let page = active_page(port).await?;
    cdp_call(&page.ws_url, "Input.insertText", serde_json::json!({ "text": text })).await?;
    Ok(())
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
    title: String,
    favicon_url: String,
}

fn keep_tab_order(order: &mut Vec<String>, targets: &mut [PageTarget]) {
    order.retain(|id| targets.iter().any(|target| &target.id == id));
    for target in targets.iter() {
        if !order.contains(&target.id) { order.push(target.id.clone()); }
    }
    targets.sort_by_key(|target| order.iter().position(|id| id == &target.id).unwrap_or(usize::MAX));
}

async fn ordered_targets(port: u16) -> Result<Vec<PageTarget>, String> {
    let mut targets = list_targets(port).await?;
    let mut guard = managed_lock().lock().await;
    if guard.browser.as_ref().is_some_and(|browser| browser.port == port) {
        // DevTools ordering changes on activation. Preserve the user's strip
        // order and append new pages; activation never moves a tab.
        keep_tab_order(&mut guard.tab_order, &mut targets);
    }
    Ok(targets)
}

async fn list_targets(port: u16) -> Result<Vec<PageTarget>, String> {
    let client = devtools_client();
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
        if kind != "page" || item["url"].as_str().is_some_and(|u| u.starts_with("chrome-extension://")) {
            continue;
        }
        let (Some(id), Some(ws), Some(url)) = (
            item.get("id").and_then(|v| v.as_str()),
            item.get("webSocketDebuggerUrl").and_then(|v| v.as_str()),
            item.get("url").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        out.push(PageTarget { id: id.to_string(), ws_url: ws.to_string(), url: url.to_string(), title: item["title"].as_str().unwrap_or_default().to_string(), favicon_url: item["faviconUrl"].as_str().unwrap_or_default().to_string() });
    }
    Ok(out)
}

async fn new_tab(port: u16, url: &str) -> Result<PageTarget, String> {
    let client = devtools_client();
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
    Ok(PageTarget { id: id.to_string(), ws_url: ws.to_string(), url: target_url.to_string(), title: item["title"].as_str().unwrap_or_default().to_string(), favicon_url: item["faviconUrl"].as_str().unwrap_or_default().to_string() })
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
    if let Some(page) = session.as_ref().and_then(|m| m.page.as_ref()) {
        let page = page.clone();
        drop(guard);
        return Ok(page);
    }
    let targets = list_targets(port).await?;
    let remembered = session.as_ref().and_then(|m| m.page_id.as_deref());
    let target = match choose_page(&targets, remembered) {
        Some(target) => target.clone(),
        None => new_tab(port, DEFAULT_HOME_URL).await?,
    };
    if let Some(session) = session {
        session.page_id = Some(target.id.clone());
        session.page = Some(target.clone());
    }
    Ok(target)
}

/// Apply protection preferences to the running tab without restarting Chromium.
pub async fn set_adblock_enabled(enabled: bool) -> Result<(), String> {
    if let Some(port) = managed_port().await {
        super::browser_extension::set_enabled(port, enabled).await?;
    }
    Ok(())
}

fn choose_page<'a>(targets: &'a [PageTarget], remembered: Option<&str>) -> Option<&'a PageTarget> {
    remembered.and_then(|id| targets.iter().find(|t| t.id == id))
        .or_else(|| targets.iter().find(|t| !t.url.starts_with("chrome://")
            && !t.url.starts_with("devtools://") && t.url != "about:blank"))
        .or_else(|| targets.first())
}

/// Reuse a multiplexed target connection; page events flow independently of commands.
pub async fn cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value, String> {
    let result = match super::cdp_transport::session(ws_url).await {
        Ok(session) => session.call(method, params).await,
        Err(error) => Err(error),
    };
    if result.as_ref().err().is_some_and(|e| e.starts_with("CDP_CLOSED") || e.starts_with("CDP_WS")) {
        let mut managed = managed_lock().lock().await;
        if let Some(browser) = managed.browser.as_mut() {
            if browser.page.as_ref().is_some_and(|p| p.ws_url == ws_url) {
                browser.page = None;
                browser.checked_at = Instant::now() - Duration::from_secs(3);
            }
        }
    }
    result
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
            // Text and controls are usable at DOM readiness. Waiting for
            // every image/analytics request delays agent navigation needlessly.
            Ok(v) if matches!(v.as_str(), Some("interactive" | "complete")) => return v,
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
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
    // Bare words become a privacy-friendly DuckDuckGo search.
    format!("https://duckduckgo.com/?q={}", url_query(t))
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

/// Navigation for the human toolbar returns as soon as Chromium accepts it.
pub async fn page_navigate_interactive(port: u16, url: &str) -> Result<(String, String), String> {
    let page = active_page(port).await?;
    let target = normalize_url(url);
    if target.is_empty() {
        return Err("EMPTY_URL: no URL provided.".to_string());
    }
    let res = cdp_call(&page.ws_url, "Page.navigate", serde_json::json!({ "url": target })).await?;
    if let Some(err) = res.get("errorText").and_then(|e| e.as_str()) {
        return Err(format!("NAVIGATE_FAILED: {err}"));
    }
    // Do not run JS in the freshly navigating renderer. Its startup scripts
    // can stall this toolbar command; redirects arrive through pane metadata.
    Ok((target.clone(), target))
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
    let state = cdp_eval(&page.ws_url, "({title: document.title, url: location.href})").await?;
    let title = state["title"].as_str().unwrap_or("").to_string();
    let url = state["url"].as_str().unwrap_or("").to_string();
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

/// Agent key presses use the same input pipeline as the visible pane.
pub async fn page_press(port: u16, key: &str) -> Result<(), String> {
    page_press_with_modifiers(port, key, 0).await
}

pub async fn page_press_with_modifiers(port: u16, key: &str, modifiers: i64) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() { return Err("EMPTY_KEY: no key provided.".into()); }
    let page = active_page(port).await?;
    let modifiers = modifiers & 15;
    if key.chars().count() == 1 && modifiers & 7 == 0 {
        cdp_call(&page.ws_url, "Input.insertText", serde_json::json!({"text": key})).await?;
        return Ok(());
    }
    let windows_code = match key {
        "Enter" => 13, "Tab" => 9, "Escape" | "Esc" => 27,
        "Backspace" => 8, "Delete" => 46, "ArrowLeft" => 37,
        "ArrowUp" => 38, "ArrowRight" => 39, "ArrowDown" => 40,
        "Home" => 36, "End" => 35, "PageUp" => 33, "PageDown" => 34,
        _ if key.len() == 1 && key.is_ascii() => key.to_ascii_uppercase().as_bytes()[0] as i32,
        _ if key.len() <= 8 && modifiers == 0 => {
            cdp_call(&page.ws_url, "Input.insertText", serde_json::json!({"text": key})).await?;
            return Ok(());
        }
        _ => return Err(format!("UNKNOWN_KEY: {key:?}")),
    };
    let code = if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphabetic()) {
        format!("Key{}", key.to_ascii_uppercase())
    } else { key.to_string() };
    for kind in ["rawKeyDown", "keyUp"] {
        cdp_call(&page.ws_url, "Input.dispatchKeyEvent", serde_json::json!({
            "type": kind, "key": key, "code": code,
            "windowsVirtualKeyCode": windows_code, "modifiers": modifiers,
        })).await?;
        if kind == "rawKeyDown" && key == "Enter" && modifiers & 7 == 0 {
            cdp_call(&page.ws_url, "Input.dispatchKeyEvent", serde_json::json!({
                "type": "char", "key": "Enter", "text": "\r", "windowsVirtualKeyCode": 13, "modifiers": modifiers,
            })).await?;
        }
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


/// Change the actual Chromium viewport to match the pane rather than scaling
/// a fixed desktop screenshot into a small column. Stream dimensions remain
/// CSS pixels (deviceScaleFactor=1), so input can be mapped without guessing.
pub async fn resize_viewport(ws_url: &str, width: u32, height: u32) -> Result<(), String> {
    cdp_call(ws_url, "Emulation.setDeviceMetricsOverride", serde_json::json!({
        "width": width.clamp(240, 1920), "height": height.clamp(120, 1440),
        "deviceScaleFactor": 1, "mobile": false,
    })).await?;
    Ok(())
}

#[derive(Clone, serde::Serialize)]
pub struct StreamFrame {
    pub session_id: i64,
    pub data_url: String,
    pub width: f64,
    pub height: f64,
}

pub struct Screencast {
    pub ws_url: String,
    events: tokio::sync::broadcast::Receiver<std::sync::Arc<Value>>,
}

pub async fn start_screencast(width: u32, height: u32) -> Result<Screencast, String> {
    let port = ensure_chromium().await?;
    let page = active_page(port).await?;
    let session = super::cdp_transport::session(&page.ws_url).await?;
    let events = session.subscribe();
    cdp_call(&page.ws_url, "Page.enable", serde_json::json!({})).await?;
    resize_viewport(&page.ws_url, width, height).await?;
    cdp_call(&page.ws_url, "Page.startScreencast", serde_json::json!({
        "format": "jpeg", "quality": 80, "maxWidth": 1920, "maxHeight": 1440, "everyNthFrame": 1,
    })).await?;
    Ok(Screencast { ws_url: page.ws_url, events })
}

impl Screencast {
    pub async fn next_frame(&mut self) -> Result<StreamFrame, String> {
        loop {
            let event = match self.events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return Err("CDP_CLOSED: browser stream ended".into()),
            };
            match event["method"].as_str() {
                Some("Comrade.disconnected") => return Err("CDP_CLOSED: browser stream ended".into()),
                Some("Page.screencastFrame") => {
                    let params = &event["params"];
                    let Some(data) = params["data"].as_str() else { continue };
                    let Some(session_id) = params["sessionId"].as_i64() else { continue };
                    let metadata = &params["metadata"];
                    return Ok(StreamFrame {
                        session_id,
                        // Forward Chromium's JPEG directly. No PNG capture,
                        // disk write, base64 decode/re-encode, or metadata RPC.
                        data_url: format!("data:image/jpeg;base64,{data}"),
                        width: metadata["deviceWidth"].as_f64().unwrap_or(1280.0),
                        height: metadata["deviceHeight"].as_f64().unwrap_or(860.0),
                    });
                }
                _ => {}
            }
        }
    }
}

pub async fn acknowledge_frame(ws_url: &str, session_id: i64) -> Result<(), String> {
    cdp_call(ws_url, "Page.screencastFrameAck", serde_json::json!({ "sessionId": session_id })).await?;
    Ok(())
}
pub async fn stop_screencast(ws_url: &str) -> Result<(), String> {
    cdp_call(ws_url, "Page.stopScreencast", serde_json::json!({})).await?;
    Ok(())
}

/// Trusted pointer events, including hover, drag and scrolling nested panes.
/// These enter Blink's input pipeline instead of evaluating window.scrollBy.
pub async fn pointer_event(port: u16, mut event: Value) -> Result<(), String> {
    let kind = event["type"].as_str().unwrap_or("");
    if !matches!(kind, "mouseMoved" | "mousePressed" | "mouseReleased" | "mouseWheel") {
        return Err("BAD_POINTER: unsupported event type".into());
    }
    for coordinate in ["x", "y"] {
        let value = event[coordinate].as_f64().unwrap_or(f64::NAN);
        if !value.is_finite() || !(0.0..=10000.0).contains(&value) {
            return Err("BAD_POINTER: coordinates out of range".into());
        }
    }
    if kind == "mouseWheel" {
        for axis in ["deltaX", "deltaY"] {
            let value = event[axis].as_f64().unwrap_or(0.0);
            event[axis] = serde_json::json!(value.clamp(-10000.0, 10000.0));
        }
    }
    let page = active_page(port).await?;
    cdp_call(&page.ws_url, "Input.dispatchMouseEvent", event).await?;
    Ok(())
}

/// Toolbar navigation returns immediately; the live stream shows progress.
pub async fn history_interactive(port: u16, forward: bool) -> Result<(), String> {
    let page = active_page(port).await?;
    let history = cdp_call(&page.ws_url, "Page.getNavigationHistory", serde_json::json!({})).await?;
    let current = history["currentIndex"].as_i64().unwrap_or(0);
    let index = if forward { current + 1 } else { current - 1 };
    if let Some(entry) = history["entries"].as_array().and_then(|entries| usize::try_from(index).ok().and_then(|i| entries.get(i))) {
        cdp_call(&page.ws_url, "Page.navigateToHistoryEntry", serde_json::json!({"entryId": entry["id"]})).await?;
    }
    Ok(())
}
pub async fn reload_interactive(port: u16) -> Result<(), String> {
    let page = active_page(port).await?;
    cdp_call(&page.ws_url, "Page.reload", serde_json::json!({})).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_profile_cleanup_fixture() {
        if std::env::var_os("COMRADE_PROCESS_FIXTURE").is_some() {
            std::thread::sleep(Duration::from_secs(90));
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_profile_cleanup_verifies_executable_and_exact_profile() {
        struct Fixture(std::process::Child);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let exe = std::env::current_exe().unwrap();
        let exe_key = exe.to_string_lossy().to_string();
        let profile = std::env::temp_dir().join(format!("comrade profile ' {}", std::process::id()));
        let profile_key = profile.to_string_lossy().to_string();
        let spawn = |profiles: &[String]| {
            let mut command = std::process::Command::new(&exe);
            command.args(["--exact", "tools::browser_driver::tests::windows_profile_cleanup_fixture", "--nocapture"]);
            // libtest accepts arbitrary --skip values, allowing a benign
            // disposable fixture to carry Chromium-shaped arguments.
            for profile in profiles {
                command.args(["--skip", &format!("--user-data-dir={profile}")]);
            }
            Fixture(command.env("COMRADE_PROCESS_FIXTURE", "1")
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null()).spawn().unwrap())
        };
        let mut owned = spawn(&[profile_key.clone()]);
        let mut sibling = spawn(&[format!("{profile_key}-other")]);
        let mut conflicting = spawn(&[profile_key.clone(), format!("{profile_key}-other")]);
        let expected = owned.0.id();
        let matches = windows_managed_processes(&exe_key, &profile_key, false);
        assert_eq!(matches.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![expected]);
        let extended_exe = std::fs::canonicalize(&exe).unwrap().to_string_lossy().to_string();
        assert_eq!(windows_managed_processes(&extended_exe, &profile_key, false)
            .iter().map(|p| p.pid).collect::<Vec<_>>(), vec![expected],
            "extended and ordinary executable paths identify the same process");
        assert_eq!(holder_is_ours(&exe_key, &profile), Some(expected));
        assert!(!process_command(expected).is_empty());
        assert!(windows_managed_processes("C:\\not-comrade\\chrome.exe", &profile_key, false).is_empty());
        kill_stale_profile_holders(&exe_key, &profile_key).await;
        assert!(owned.0.try_wait().unwrap().is_some(), "managed stale process must exit");
        assert!(sibling.0.try_wait().unwrap().is_none(), "different profile must survive");
        assert!(conflicting.0.try_wait().unwrap().is_none(), "ambiguous profile must survive");
        assert!(!holder_running(&exe_key, &profile_key).await);
    }

    #[tokio::test]
    async fn bundled_exe_honors_override() {
        let _lock = crate::env_lock();
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
    fn debug_port_parses_from_holder_cmdline() {
        assert_eq!(
            debug_port_from_cmd("/x/chrome --remote-debugging-port=44897 --user-data-dir=/y"),
            Some(44897)
        );
        assert_eq!(debug_port_from_cmd("/x/chrome --headless"), None);
        assert_eq!(debug_port_from_cmd("/x/chrome --remote-debugging-port=0"), None);
        assert_eq!(debug_port_from_cmd(""), None);
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
        assert_eq!(normalize_url("imagine dragons"), "https://duckduckgo.com/?q=imagine%20dragons");
    }

    #[test]
    fn page_selection_stays_on_same_target_when_another_tab_opens() {
        let page = |id: &str, url: &str| PageTarget { id: id.into(), url: url.into(), title: String::new(), favicon_url: String::new(), ws_url: String::new() };
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
