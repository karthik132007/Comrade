//! Live Chromium CDP checks, ignored by default because they start a browser.
//! Set COMRADE_HOME to a fresh scratch directory and optionally set
//! COMRADE_TEST_BROWSER to an executable; otherwise native Chrome is detected.
//! Run with `cargo test -p comrade-core --test browser_cdp -- --ignored`.
//! All profiles and pages stay in the supplied scratch directory. Tests are
//! serialized because the driver and COMRADE_HOME are process-wide.

use comrade_core::tools::browser_driver;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

struct TestContext {
    home: PathBuf,
    original_home: std::ffi::OsString,
    browser: PathBuf,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl TestContext {
    async fn new(name: &str) -> Self {
        let guard = TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let original_home = std::env::var_os("COMRADE_HOME").expect(
            "set COMRADE_HOME to a fresh scratch directory before running live browser tests",
        );
        let root = PathBuf::from(&original_home);
        assert!(
            !root.join("comrade.conf").exists() && !root.join("browser-profile").exists(),
            "COMRADE_HOME must be a scratch directory, not an existing Comrade home"
        );
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = root.join(format!("{name}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&home).expect("create test scratch directory");
        let home = std::fs::canonicalize(home).unwrap();
        let browser = test_browser();
        browser_driver::close_chromium().await;
        std::env::set_var("COMRADE_HOME", &home);
        Self {
            home,
            original_home,
            browser,
            _guard: guard,
        }
    }

    fn configure(&self, exe: &Path, profile: &str, debug_port: u16) {
        std::fs::write(
            self.home.join("comrade.conf"),
            format!(
                "[browser]\nexe = {}\nkind = binary\nheadless = true\nprofile = {profile}\ndebug_port = {debug_port}\n",
                exe.display()
            ),
        )
        .expect("write scratch browser settings");
    }

    fn page(&self, name: &str) -> String {
        let path = self.home.join(format!("{name}.html"));
        std::fs::write(
            &path,
            format!("<!doctype html><title>{name}</title><h1>{name}</h1>"),
        )
        .expect("write local test page");
        reqwest::Url::from_file_path(path)
            .expect("local file URL")
            .to_string()
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        std::env::set_var("COMRADE_HOME", &self.original_home);
    }
}

fn test_browser() -> PathBuf {
    if let Some(exe) = std::env::var_os("COMRADE_TEST_BROWSER") {
        let exe = PathBuf::from(exe);
        assert!(
            exe.is_file(),
            "COMRADE_TEST_BROWSER is not an executable file: {}",
            exe.display()
        );
        return std::fs::canonicalize(exe).unwrap();
    }
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        candidates.extend([
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ]);
    }
    if cfg!(target_os = "windows") {
        for base in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(base) {
                candidates.push(PathBuf::from(base).join("Google/Chrome/Application/chrome.exe"));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in [
                "google-chrome-stable",
                "google-chrome",
                "chromium",
                "chromium-browser",
            ] {
                candidates.push(dir.join(name));
            }
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| std::fs::canonicalize(path).unwrap())
        .expect("no native Chrome found; set COMRADE_TEST_BROWSER to its executable")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}

async fn page_targets(port: u16) -> Vec<Value> {
    let targets: Vec<Value> = client()
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await
        .expect("list browser targets")
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    targets
        .into_iter()
        .filter(|target| target["type"] == "page")
        .collect()
}

async fn debugger_alive(port: u16) -> bool {
    client()
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

async fn start_external_browser(context: &TestContext) -> (tokio::process::Child, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let profile = context.home.join("attach-profile");
    std::fs::create_dir_all(&profile).unwrap();
    let child = tokio::process::Command::new(&context.browser)
        .args([
            format!("--remote-debugging-port={port}"),
            "--remote-allow-origins=*".to_string(),
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-dev-shm-usage".to_string(),
            "--no-sandbox".to_string(),
            "--headless=new".to_string(),
            "--disable-gpu".to_string(),
            "about:blank".to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("start external browser with a scratch profile");
    for _ in 0..60 {
        if debugger_alive(port).await {
            return (child, port);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("external browser never exposed DevTools");
}

#[tokio::test]
#[ignore]
async fn live_chromium_reuses_one_tab_and_keeps_it_selected() {
    let context = TestContext::new("navigation").await;
    context.configure(&context.browser, "comrade", 0);
    let port = browser_driver::ensure_chromium()
        .await
        .expect("launch Chromium");
    let initial = page_targets(port).await;
    assert!(initial.len() <= 1, "a fresh browser must not create extra tabs");
    let first_url = context.page("initial");
    browser_driver::page_navigate(port, &first_url)
        .await
        .expect("establish the controlled tab");
    let first_targets = page_targets(port).await;
    assert_eq!(first_targets.len(), 1, "first navigation should establish exactly one tab");
    let target_id = first_targets[0]["id"].clone();

    for name in ["first", "second", "third"] {
        assert_eq!(
            browser_driver::ensure_chromium().await.unwrap(),
            port,
            "reuse browser process"
        );
        let url = context.page(name);
        let (_, current_url) = browser_driver::page_navigate(port, &url)
            .await
            .expect("navigate");
        assert_eq!(current_url, url);
        let (title, _) = browser_driver::page_title(port).await.expect("read title");
        assert_eq!(title, name);
        let (text, _) = browser_driver::page_text(port, 2000)
            .await
            .expect("read text");
        assert!(text.contains(name), "unexpected page text: {text}");
        let targets = page_targets(port).await;
        assert_eq!(
            targets.len(),
            1,
            "navigation and reads must not create tabs"
        );
        assert_eq!(targets[0]["id"], target_id, "reuse the controlled tab");
    }

    let screenshot = browser_driver::page_screenshot(port)
        .await
        .expect("screenshot");
    assert!(
        screenshot.exists(),
        "screenshot missing: {}",
        screenshot.display()
    );

    // A tab opened separately must not steal subsequent reads/navigation.
    let unrelated: Value = client()
        .put(format!("http://127.0.0.1:{port}/json/new?about:blank"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let (title, _) = browser_driver::page_title(port)
        .await
        .expect("read selected tab");
    assert_eq!(
        title, "third",
        "new unrelated tabs must not steal selection"
    );
    let url = context.page("fourth");
    browser_driver::page_navigate(port, &url)
        .await
        .expect("navigate selected tab");
    let targets = page_targets(port).await;
    assert_eq!(
        targets.len(),
        2,
        "reuse selected tab even when another tab exists"
    );
    assert_eq!(
        targets
            .iter()
            .find(|target| target["id"] == target_id)
            .unwrap()["url"],
        url
    );
    assert_eq!(
        targets
            .iter()
            .find(|target| target["id"] == unrelated["id"])
            .unwrap()["url"],
        "about:blank",
        "unrelated tab should remain untouched"
    );
    assert_eq!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Closed
    );
}

#[tokio::test]
#[ignore]
async fn live_attachment_honors_browser_and_profile_changes() {
    let context = TestContext::new("attachment").await;
    let (mut child, external_port) = start_external_browser(&context).await;
    context.configure(&context.browser, "user", external_port);
    let port = browser_driver::ensure_chromium()
        .await
        .expect("attach to external browser");
    assert_eq!(
        port, external_port,
        "attach without launching a second browser"
    );
    let url = context.page("attached");
    browser_driver::page_navigate(port, &url)
        .await
        .expect("navigate attached browser");

    // Changing the chosen executable must invalidate the cached attachment.
    context.configure(&context.home.join("missing-browser"), "user", external_port);
    let error = browser_driver::ensure_chromium()
        .await
        .expect_err("missing selection must fail");
    assert!(
        error.starts_with("BROWSER_NOT_FOUND:"),
        "unexpected error: {error}"
    );

    // The listener is real Chrome, but this selected executable is different.
    // A test executable has no real browser profile; even a failed check cannot
    // reach the user's Chrome profile.
    context.configure(&std::env::current_exe().unwrap(), "user", external_port);
    let error = browser_driver::ensure_chromium()
        .await
        .expect_err("wrong browser must not attach");
    assert!(
        error.starts_with("DEBUG_BROWSER_MISMATCH:"),
        "unexpected error: {error}"
    );
    assert!(
        debugger_alive(external_port).await,
        "selection changes must not kill an attached browser"
    );

    context.configure(&context.browser, "user", external_port);
    assert_eq!(
        browser_driver::ensure_chromium().await.unwrap(),
        external_port
    );
    assert_eq!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Detached
    );
    assert!(
        debugger_alive(external_port).await,
        "detach must leave the external browser running"
    );
    assert_eq!(
        browser_driver::ensure_chromium().await.unwrap(),
        external_port
    );

    // Switching to an isolated profile must not keep using the live user tab,
    // even if the saved debug port still points at that external browser.
    context.configure(&context.browser, "comrade", external_port);
    let isolated_port = browser_driver::ensure_chromium()
        .await
        .expect("launch isolated browser");
    assert_ne!(
        isolated_port, external_port,
        "profile changes must replace the cached attachment"
    );
    assert_eq!(page_targets(external_port).await.len(), 1);
    assert_eq!(page_targets(external_port).await[0]["url"], url);
    assert_eq!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Closed
    );
    assert!(
        debugger_alive(external_port).await,
        "closing isolated browser must preserve external browser"
    );
    child.kill().await.expect("stop scratch external browser");
}
