/**
 * Self-installing bundled Chromium.
 *
 * The browser is Comrade's main feature, not an add-on: the app provisions
 * its own dedicated Chromium automatically — at first launch (background)
 * and on first browser use (blocking) — with progress exposed for the UI.
 * No manual steps, no system browsers, no user profiles.
 *
 * Layout: `comrade-agent/browser/` holds the binary; the single isolated
 * profile lives next door in `comrade-agent/browser-profile/`.
 * Override for tests/air-gapped machines: COMRADE_CHROMIUM_BIN.
 * Manual fallback (offline): ./scripts/fetch-chromium.sh
 */
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use tokio::io::AsyncWriteExt;

/// Chrome-for-Testing release index: resolves the current Stable build per
/// platform so no version is ever hard-coded here.
const KNOWN_GOOD_URL: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Resolving,
    Downloading,
    Extracting,
    Done,
    Failed,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Resolving => "resolving",
            Phase::Downloading => "downloading",
            Phase::Extracting => "extracting",
            Phase::Done => "done",
            Phase::Failed => "failed",
        }
    }
}

struct ProvisionState {
    phase: std::sync::Mutex<Phase>,
    error: std::sync::Mutex<String>,
    downloaded: AtomicU64,
    total: AtomicU64,
}

static STATE: OnceLock<ProvisionState> = OnceLock::new();
static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn state() -> &'static ProvisionState {
    STATE.get_or_init(|| ProvisionState {
        phase: std::sync::Mutex::new(Phase::Idle),
        error: std::sync::Mutex::new(String::new()),
        downloaded: AtomicU64::new(0),
        total: AtomicU64::new(0),
    })
}

fn lock() -> &'static tokio::sync::Mutex<()> {
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn set_phase(p: Phase) {
    *state().phase.lock().unwrap() = p;
}

fn set_error(msg: String) {
    *state().error.lock().unwrap() = msg;
}

/// Platform tag used by the Chrome-for-Testing release index.
pub fn platform_tag() -> &'static str {
    if cfg!(target_os = "windows") {
        "win64"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") { "mac-arm64" } else { "mac-x64" }
    } else {
        "linux64"
    }
}

/// Full `chrome` build on every platform. Linux deliberately does NOT use the
/// minimal `chrome-headless-shell`: it crashes on startup (ICU data /
/// SIGTRAP) in common container setups, while full `chrome --headless=new`
/// is the well-tested path — the same flags the driver always launches with.
fn asset_name() -> &'static str {
    "chrome"
}

/// Progress snapshot for the UI (`browser_provision_status` command).
pub fn status_json() -> serde_json::Value {
    let phase = *state().phase.lock().unwrap();
    serde_json::json!({
        "installed": crate::paths::bundled_chromium_exe().is_some(),
        "phase": phase.as_str(),
        "downloaded": state().downloaded.load(Ordering::SeqCst),
        "total": state().total.load(Ordering::SeqCst),
        "error": state().error.lock().unwrap().clone(),
    })
}

/// Ensure the bundled Chromium exists, downloading it on first use.
/// Concurrent callers serialize on one install; the binary is verified with
/// `--version` before success is reported.
pub async fn ensure_provisioned() -> Result<std::path::PathBuf, String> {
    if let Some(exe) = crate::paths::bundled_chromium_exe() {
        set_phase(Phase::Done);
        return Ok(exe);
    }
    let _guard = lock().lock().await;
    // Re-check: another caller may have finished while we waited.
    if let Some(exe) = crate::paths::bundled_chromium_exe() {
        set_phase(Phase::Done);
        return Ok(exe);
    }
    set_error(String::new());
    state().downloaded.store(0, Ordering::SeqCst);
    state().total.store(0, Ordering::SeqCst);
    let out = install().await;
    match &out {
        Ok(_) => set_phase(Phase::Done),
        Err(e) => {
            set_phase(Phase::Failed);
            set_error(e.clone());
        }
    }
    out
}

async fn install() -> Result<std::path::PathBuf, String> {
    let dir = crate::paths::browser_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| install_failed(&format!("cannot create browser dir: {e}")))?;

    set_phase(Phase::Resolving);
    let url = resolve_download_url().await?;

    set_phase(Phase::Downloading);
    let zip_path = dir.join(".download.pkg.zip.part");
    download(&url, &zip_path).await?;

    set_phase(Phase::Extracting);
    let exe = extract(&zip_path, &dir).await?;
    let _ = std::fs::remove_file(&zip_path);

    verify(&exe).await?;
    Ok(exe)
}

fn install_failed(detail: &str) -> String {
    format!(
        "BROWSER_SETUP: couldn't install Comrade's built-in browser automatically ({detail}). \
         Check your connection and retry — it installs itself on first launch. \
         Offline fallback: ./scripts/fetch-chromium.sh"
    )
}

/// Pick the Stable download URL for this platform from the release index.
async fn resolve_download_url() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| install_failed(&format!("http client: {e}")))?;
    let doc: serde_json::Value = client
        .get(KNOWN_GOOD_URL)
        .send()
        .await
        .map_err(|e| install_failed(&format!("release index unreachable: {e}")))?
        .error_for_status()
        .map_err(|e| install_failed(&format!("release index error: {e}")))?
        .json()
        .await
        .map_err(|e| install_failed(&format!("bad release index: {e}")))?;
    let want_asset = asset_name();
    let want_platform = platform_tag();
    let downloads = doc
        .pointer("/channels/Stable/downloads")
        .and_then(|v| v.as_object())
        .ok_or_else(|| install_failed("release index has no Stable channel"))?;
    let entries = downloads
        .get(want_asset)
        .and_then(|v| v.as_array())
        .ok_or_else(|| install_failed(&format!("no {want_asset} builds published")))?;
    entries
        .iter()
        .find(|e| e.get("platform").and_then(|p| p.as_str()) == Some(want_platform))
        .and_then(|e| e.get("url").and_then(|u| u.as_str()))
        .map(|s| s.to_string())
        .ok_or_else(|| install_failed(&format!("no {want_asset} build for {want_platform}")))
}

async fn download(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| install_failed(&format!("http client: {e}")))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| install_failed(&format!("download failed: {e}")))?
        .error_for_status()
        .map_err(|e| install_failed(&format!("download error: {e}")))?;
    if let Some(len) = resp.content_length() {
        state().total.store(len, Ordering::SeqCst);
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| install_failed(&format!("cannot write download: {e}")))?;
    let mut done: u64 = 0;
    while let Some(chunk) = resp.chunk().await.map_err(|e| install_failed(&format!("download interrupted: {e}")))? {
        file.write_all(&chunk)
            .await
            .map_err(|e| install_failed(&format!("cannot write download: {e}")))?;
        done += chunk.len() as u64;
        state().downloaded.store(done, Ordering::SeqCst);
    }
    file.flush().await.map_err(|e| install_failed(&format!("cannot write download: {e}")))?;
    Ok(())
}

/// Unpack the build into `dir` and return the executable path.
/// Blocking zip I/O runs on a blocking thread.
async fn extract(zip_part: &std::path::Path, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let zip_path = zip_part.to_path_buf();
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || extract_blocking(&zip_path, &dir))
        .await
        .map_err(|e| install_failed(&format!("extract task: {e}")))??;
    crate::paths::bundled_chromium_exe()
        .ok_or_else(|| install_failed("archive did not contain a usable browser binary"))
}

fn extract_blocking(zip_part: &std::path::Path, dir: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_part)
        .map_err(|e| install_failed(&format!("cannot read download: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| install_failed(&format!("bad archive: {e}")))?;
    // Full chrome build: needs its whole directory (resources, locales).
    archive
        .extract(dir)
        .map_err(|e| install_failed(&format!("cannot unpack browser: {e}")))?;
    // Ensure executables can run (archives don't always preserve modes).
    for candidate in crate::paths::bundled_chromium_candidates() {
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    Ok(())
}

/// The install only counts when the binary actually starts.
async fn verify(exe: &std::path::Path) -> Result<(), String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(exe)
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| install_failed("browser binary did not respond"))?
    .map_err(|e| install_failed(&format!("cannot start browser binary: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(install_failed("browser binary failed its startup check (missing system libs?)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_tag_is_known() {
        assert!(["linux64", "mac-arm64", "mac-x64", "win64"].contains(&platform_tag()));
        assert!(!asset_name().is_empty());
    }

    #[test]
    fn status_shape_for_ui() {
        let v = status_json();
        assert!(v.get("installed").and_then(|v| v.as_bool()).is_some());
        assert!(v.get("phase").and_then(|v| v.as_str()).is_some());
        assert!(v.get("downloaded").and_then(|v| v.as_u64()).is_some());
        assert!(v.get("total").and_then(|v| v.as_u64()).is_some());
    }
}
