//! Live bundled-Chromium CDP checks, ignored by default because they start a browser.
//! Set COMRADE_HOME to a fresh scratch directory and COMRADE_CHROMIUM_BIN to a
//! Chromium executable (or pre-provision comrade-agent/browser/ and unset the override).
//! Run with `cargo test -p comrade-core --test browser_cdp -- --ignored`.
//! `live_self_install_*` needs network: it proves the first-run path — empty
//! home, no binary, the app downloads and drives its own browser.
//! All profiles and pages stay in the supplied scratch directory. Tests are
//! serialized because the driver and COMRADE_HOME are process-wide.

use comrade_core::tools::{browser_driver, provision};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

struct TestContext {
    home: PathBuf,
    original_home: std::ffi::OsString,
    original_bin: Option<std::ffi::OsString>,
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
            !root.join("browser-profile").exists(),
            "COMRADE_HOME must be a scratch directory, not an existing Comrade home"
        );
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = root.join(format!("{name}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&home).expect("create test scratch directory");
        let home = std::fs::canonicalize(home).unwrap();
        // The bundled binary under test: explicit bin wins, else a provisioned
        // browser/ dir under the scratch home.
        let original_bin = std::env::var_os("COMRADE_CHROMIUM_BIN");
        let exe = test_chromium_exe(&home);
        browser_driver::close_chromium().await;
        std::env::set_var("COMRADE_HOME", &home);
        std::env::set_var(
            "COMRADE_CHROMIUM_BIN",
            exe.to_string_lossy().to_string(),
        );
        Self { home, original_home, original_bin, _guard: guard }
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
        match &self.original_bin {
            Some(v) => std::env::set_var("COMRADE_CHROMIUM_BIN", v),
            None => std::env::remove_var("COMRADE_CHROMIUM_BIN"),
        }
    }
}

fn test_chromium_exe(home: &PathBuf) -> PathBuf {
    if let Some(exe) = std::env::var_os("COMRADE_CHROMIUM_BIN") {
        let exe = PathBuf::from(exe);
        assert!(exe.is_file(), "COMRADE_CHROMIUM_BIN is not a file: {}", exe.display());
        return std::fs::canonicalize(exe).unwrap();
    }
    for candidate in comrade_core::paths::bundled_chromium_candidates() {
        if candidate.is_file() {
            return std::fs::canonicalize(candidate).unwrap();
        }
    }
    let _ = home;
    panic!("no bundled Chromium found; set COMRADE_CHROMIUM_BIN or provision browser/ via scripts/fetch-chromium.sh");
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

#[tokio::test]
#[ignore]
async fn live_bundled_chromium_reuses_one_tab_and_stays_headless() {
    let context = TestContext::new("navigation").await;
    let port = browser_driver::ensure_chromium()
        .await
        .expect("launch bundled Chromium");
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
    let bytes = browser_driver::screenshot_bytes()
        .await
        .expect("live-view bytes for the in-app pane");
    assert!(!bytes.is_empty(), "in-app pane screenshot must not be empty");

    let snap = browser_driver::state_snapshot().await;
    assert_eq!(snap["running"], true);

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
    let snap = browser_driver::state_snapshot().await;
    assert_eq!(snap["running"], false);
}

/// First-run path: empty home, no binary, no override — the app must
/// download its own browser and drive it. Needs network (≈100MB download).
#[tokio::test]
#[ignore]
async fn live_self_install_provisions_and_drives_browser() {
    let _guard = TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let original_home = std::env::var_os("COMRADE_HOME").expect(
        "set COMRADE_HOME to a fresh scratch directory before running live browser tests",
    );
    let original_bin = std::env::var_os("COMRADE_CHROMIUM_BIN");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let home = PathBuf::from(&original_home).join(format!("install-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&home).expect("create test scratch directory");
    let home = std::fs::canonicalize(home).unwrap();
    assert!(
        !home.join("browser").exists(),
        "self-install test needs a home with no provisioned browser"
    );
    std::env::set_var("COMRADE_HOME", &home);
    std::env::remove_var("COMRADE_CHROMIUM_BIN");
    browser_driver::close_chromium().await;

    let exe = provision::ensure_provisioned()
        .await
        .expect("self-install the bundled browser");
    assert!(exe.is_file(), "missing binary: {}", exe.display());
    let status = provision::status_json();
    assert_eq!(status["installed"], true);
    assert_eq!(status["phase"], "done");

    // The freshly installed browser must actually drive.
    let port = browser_driver::ensure_chromium()
        .await
        .expect("launch self-installed browser");
    let page = home.join("hello.html");
    std::fs::write(&page, "<!doctype html><title>hello</title><h1>hello</h1>").unwrap();
    let url = reqwest::Url::from_file_path(page).unwrap().to_string();
    let (_, current) = browser_driver::page_navigate(port, &url).await.expect("navigate");
    assert_eq!(current, url);
    let (title, _) = browser_driver::page_title(port).await.expect("title");
    assert_eq!(title, "hello");
    assert_eq!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Closed
    );

    std::env::set_var("COMRADE_HOME", &original_home);
    match &original_bin {
        Some(v) => std::env::set_var("COMRADE_CHROMIUM_BIN", v),
        None => std::env::remove_var("COMRADE_CHROMIUM_BIN"),
    }
}


async fn rect_of(port: u16, sel: &str) -> serde_json::Value {
    let raw = browser_driver::cdp_eval_via_active(
        port,
        &format!(
            "JSON.stringify((()=>{{const r=document.querySelector('{sel}').getBoundingClientRect();\
              return {{x:r.x,y:r.y,w:r.width,h:r.height}};}})())"
        ),
    )
    .await
    .expect("element rect");
    serde_json::from_str::<serde_json::Value>(&raw).expect("rect json")
}

/// In-app pane interaction: CDP mouse clicks press real page buttons and
/// typed text lands in focused fields (trusted input, not JS clicks).
#[tokio::test]
#[ignore]
async fn live_mouse_click_and_type_drive_the_page() {
    let context = TestContext::new("interaction").await;
    let port = browser_driver::ensure_chromium()
        .await
        .expect("launch bundled Chromium");
    let path = context.home.join("click.html");
    std::fs::write(
        &path,
        "<!doctype html><title>ready</title>\
         <button id=b style='position:absolute;left:100px;top:80px;width:120px;height:40px' onclick=\"document.title='clicked'\">go</button>\
         <input id=f style='position:absolute;left:100px;top:200px;width:200px' value=''>",
    )
    .unwrap();
    let url = reqwest::Url::from_file_path(path).unwrap().to_string();
    browser_driver::page_navigate(port, &url)
        .await
        .expect("navigate");
    let (vw, vh) = browser_driver::viewport_metrics(port)
        .await
        .expect("viewport metrics");
    assert!(vw > 0.0 && vh > 0.0, "viewport must be readable");

    let b = rect_of(port, "#b").await;
    let bx = b["x"].as_f64().unwrap() + b["w"].as_f64().unwrap() / 2.0;
    let by = b["y"].as_f64().unwrap() + b["h"].as_f64().unwrap() / 2.0;
    browser_driver::mouse_click(port, bx, by)
        .await
        .expect("click button");
    let (title, _) = browser_driver::page_title(port).await.expect("title");
    assert_eq!(title, "clicked", "real mouse click must press the button");

    let f = rect_of(port, "#f").await;
    browser_driver::mouse_click(
        port,
        f["x"].as_f64().unwrap() + 10.0,
        f["y"].as_f64().unwrap() + 10.0,
    )
    .await
    .expect("focus field");
    browser_driver::insert_text(port, "hello")
        .await
        .expect("type");
    let value = browser_driver::cdp_eval_via_active(port, "document.querySelector('#f').value")
        .await
        .expect("field value");
    assert_eq!(value, "hello", "typed text must land in the focused field");

    // Out-of-range clicks are rejected, not sent to the page.
    assert!(browser_driver::mouse_click(port, -5.0, 10.0).await.is_err());
    assert!(browser_driver::mouse_click(port, f64::NAN, 10.0).await.is_err());

    assert_eq!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Closed
    );
}
