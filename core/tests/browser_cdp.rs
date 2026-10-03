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

/// End-to-end stream against a real isolated Chromium, including nested wheel
/// input, viewport reflow, and stop/restart. No external websites or accounts.
#[tokio::test]
#[ignore]
async fn live_stream_reflows_and_delivers_input_without_snapshot_polling() {
    let context = TestContext::new("stream").await;
    let port = browser_driver::ensure_chromium().await.unwrap();
    let path = context.home.join("stream.html");
    std::fs::write(&path, "<!doctype html><title>stream</title><style>body{margin:0}#scroll{position:absolute;left:20px;top:20px;width:250px;height:180px;overflow:auto}#inside{height:2000px;background:linear-gradient(red,blue)}</style><div id=scroll><div id=inside></div></div><input id=field style='position:absolute;left:300px;top:20px'>").unwrap();
    let url = reqwest::Url::from_file_path(path).unwrap().to_string();
    browser_driver::page_navigate(port, &url).await.unwrap();
    let start = std::time::Instant::now();
    let mut stream = browser_driver::start_screencast(640, 480).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next_frame()).await.unwrap().unwrap();
    assert!(first.data_url.starts_with("data:image/jpeg;base64,"));
    assert_eq!((first.width, first.height), (640.0, 480.0));
    println!("stream initial frame: {:?}, JPEG payload {} bytes", start.elapsed(), first.data_url.len());
    browser_driver::acknowledge_frame(&stream.ws_url, first.session_id).await.unwrap();
    assert_eq!(browser_driver::viewport_metrics(port).await.unwrap(), (640.0, 480.0));

    // Scrolling must hit the nested element underneath the pointer; the old
    // window.scrollBy path could never scroll this page's inner panel.
    let action = std::time::Instant::now();
    browser_driver::pointer_event(port, serde_json::json!({"type":"mouseWheel", "x":100, "y":100, "deltaX":0, "deltaY":300})).await.unwrap();
    let mut scrolled = false;
    for _ in 0..20 {
        let v = browser_driver::cdp_eval_via_active(port, "String(document.querySelector('#scroll').scrollTop)").await.unwrap();
        if v.parse::<f64>().unwrap_or(0.0) > 0.0 { scrolled = true; break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(scrolled, "trusted wheel should scroll nested panel");
    let changed = tokio::time::timeout(Duration::from_secs(3), stream.next_frame()).await.unwrap().unwrap();
    browser_driver::acknowledge_frame(&stream.ws_url, changed.session_id).await.unwrap();
    println!("wheel dispatch + page scroll + next frame: {:?}", action.elapsed());

    browser_driver::mouse_click(port, 330.0, 30.0).await.unwrap();
    browser_driver::insert_text(port, "streamed input").await.unwrap();
    assert_eq!(browser_driver::cdp_eval_via_active(port, "document.querySelector('#field').value").await.unwrap(), "streamed input");
    browser_driver::page_press_with_modifiers(port, "a", 2).await.unwrap();
    let pasted = "long paste ".repeat(50);
    browser_driver::insert_text(port, &pasted).await.unwrap();
    assert_eq!(browser_driver::cdp_eval_via_active(port, "document.querySelector('#field').value").await.unwrap(), pasted);
    browser_driver::resize_viewport(&stream.ws_url, 800, 600).await.unwrap();
    assert_eq!(browser_driver::viewport_metrics(port).await.unwrap(), (800.0, 600.0));
    let mut resized = false;
    for _ in 0..10 {
        let frame = tokio::time::timeout(Duration::from_secs(3), stream.next_frame()).await.unwrap().unwrap();
        browser_driver::acknowledge_frame(&stream.ws_url, frame.session_id).await.unwrap();
        if (frame.width, frame.height) == (800.0, 600.0) { resized = true; break; }
    }
    assert!(resized, "stream frame metadata must follow resized viewport");
    browser_driver::stop_screencast(&stream.ws_url).await.unwrap();
    let mut restarted = browser_driver::start_screencast(480, 360).await.unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(3), restarted.next_frame()).await.unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (480.0, 360.0));
    browser_driver::acknowledge_frame(&restarted.ws_url, frame.session_id).await.unwrap();
    browser_driver::stop_screencast(&restarted.ws_url).await.unwrap();

    // Report the connection overhead on the exact same target and page.
    // This baseline reproduces the old per-command WebSocket transport.
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let repeats = 30;
    let baseline = std::time::Instant::now();
    for _ in 0..repeats {
        let (mut ws, _) = tokio_tungstenite::connect_async(&restarted.ws_url).await.unwrap();
        ws.send(Message::Text(serde_json::json!({"id":1,"method":"Runtime.evaluate","params":{"expression":"1+1","returnByValue":true}}).to_string().into())).await.unwrap();
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let result: Value = serde_json::from_str(&text).unwrap();
            if result["id"] == 1 { break; }
        }
        let _ = ws.close(None).await;
    }
    let baseline_elapsed = baseline.elapsed();
    let persistent = std::time::Instant::now();
    for _ in 0..repeats {
        let value = browser_driver::cdp_eval_via_active(port, "String(1+1)").await.unwrap();
        assert_eq!(value, "2");
    }
    println!("{repeats} commands: old reconnect transport {:?}; persistent transport {:?}", baseline_elapsed, persistent.elapsed());
    assert_eq!(browser_driver::close_chromium().await, browser_driver::CloseOutcome::Closed);
}

/// Tab metadata must remain responsive when a background renderer is busy.
#[tokio::test]
#[ignore]
async fn live_tabs_stay_responsive_with_busy_background_page() {
    let context = TestContext::new("tab-performance").await;
    let port = browser_driver::ensure_chromium().await.unwrap();
    browser_driver::page_navigate(port, &context.page("first")).await.unwrap();
    let first = browser_driver::tabs(port).await.unwrap().into_iter().find(|tab| tab["active"] == true).unwrap();
    let second = browser_driver::create_tab(port, &context.page("second")).await.unwrap();
    let targets: Vec<Value> = client().get(format!("http://127.0.0.1:{port}/json/list")).send().await.unwrap().json().await.unwrap();
    let ws = targets.iter().find(|target| target["id"] == first["id"]).unwrap()["webSocketDebuggerUrl"].as_str().unwrap().to_owned();
    // Run a long synchronous script on the background page, as a heavy site can.
    let busy = tokio::spawn(async move {
        browser_driver::cdp_call(&ws, "Runtime.evaluate", serde_json::json!({
            "expression": "{ const until = performance.now() + 1500; while (performance.now() < until) {} }"
        })).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let start = std::time::Instant::now();
    let snapshot = tokio::time::timeout(Duration::from_millis(800), browser_driver::state_snapshot()).await
        .expect("metadata must not wait for background JavaScript");
    println!("tab snapshot with busy background renderer: {:?}", start.elapsed());
    assert_eq!(snapshot["tabs"].as_array().unwrap().len(), 2);
    assert_eq!(snapshot["url"], second["url"]);
    assert_eq!(browser_driver::interactive_port().await.unwrap(), port);
    busy.await.unwrap();
    let after_close = browser_driver::close_tab(port, first["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(after_close["id"], second["id"], "closing a background tab must preserve the active page");
    assert_eq!(browser_driver::close_chromium().await, browser_driver::CloseOutcome::Closed);
}

/// Activation and background-tab closure must preserve strip order and page identity.
#[tokio::test]
#[ignore]
async fn live_tab_order_and_active_page_remain_stable() {
    let context = TestContext::new("stable-tabs").await;
    let port = browser_driver::ensure_chromium().await.unwrap();
    browser_driver::page_navigate(port, &context.page("video-tab")).await.unwrap();
    let first = browser_driver::tabs(port).await.unwrap().into_iter().find(|tab| tab["active"] == true).unwrap();
    let second = browser_driver::create_tab(port, &context.page("second-tab")).await.unwrap();
    let third = browser_driver::create_tab(port, &context.page("third-tab")).await.unwrap();
    let order = vec![first["id"].clone(), second["id"].clone(), third["id"].clone()];
    for tab in [&first, &third, &second, &first] {
        browser_driver::select_tab(port, tab["id"].as_str().unwrap()).await.unwrap();
        let tabs = browser_driver::tabs(port).await.unwrap();
        assert_eq!(tabs.iter().map(|tab| tab["id"].clone()).collect::<Vec<_>>(), order);
        assert_eq!(tabs.iter().find(|tab| tab["active"] == true).unwrap()["id"], tab["id"]);
        assert_eq!(browser_driver::current_url(port).await.unwrap(), tab["url"].as_str().unwrap());
    }
    browser_driver::close_tab(port, third["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(browser_driver::current_url(port).await.unwrap(), first["url"].as_str().unwrap());
    let neighbor = browser_driver::close_tab(port, first["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(neighbor["id"], second["id"]);
    assert_eq!(browser_driver::close_chromium().await, browser_driver::CloseOutcome::Closed);
}

/// Slow local fixtures verify early navigation, cached assets and real icon metadata.
#[tokio::test]
#[ignore]
async fn live_loading_returns_early_keeps_cache_and_reports_favicon() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let context = TestContext::new("page-loading").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let image_requests = Arc::new(AtomicUsize::new(0));
    let counter = image_requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 8192];
                let n = socket.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap();
                let (kind, body, cache) = match path {
                    "/heavy" => ("text/html", "<!doctype html><title>heavy</title><h1>Loading test</h1><script>const until=performance.now()+1500;while(performance.now()<until){}</script>", "no-store"),
                    "/slow.svg" => {
                        counter.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        ("image/svg+xml", "<svg xmlns='http://www.w3.org/2000/svg' width='40' height='40'><rect width='40' height='40' fill='blue'/></svg>", "public, max-age=3600")
                    }
                    "/brand.svg" => ("image/svg+xml", "<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'><rect width='16' height='16' fill='red'/></svg>", "public, max-age=3600"),
                    _ => ("text/html", "<!doctype html><title>early content</title><link rel='icon' type='image/svg+xml' href='/brand.svg'><h1>Visible content</h1><img src='/slow.svg'>", "no-store"),
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCache-Control: {cache}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    let port = browser_driver::ensure_chromium().await.unwrap();
    let start = std::time::Instant::now();
    browser_driver::page_navigate_interactive(port, &format!("{origin}/heavy")).await.unwrap();
    let elapsed = start.elapsed();
    println!("interactive navigation with 1.5s startup script: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(1000), "toolbar must not wait for page JavaScript");
    let pane = tokio::time::timeout(Duration::from_millis(500), browser_driver::pane_snapshot(port)).await.unwrap().unwrap();
    assert_eq!(pane["running"], true);
    // Let this intentionally busy page finish before testing DOM readiness.
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let start = std::time::Instant::now();
    browser_driver::page_navigate(port, &origin).await.unwrap();
    println!("DOM-ready navigation while image is still loading: {:?}", start.elapsed());
    assert!(start.elapsed() < Duration::from_millis(1000), "usable HTML must not wait for slow image downloads");
    let mut favicon = String::new();
    for _ in 0..40 {
        let snapshot = browser_driver::pane_snapshot(port).await.unwrap();
        let active = snapshot["tabs"].as_array().unwrap().iter().find(|tab| tab["active"] == true).unwrap();
        favicon = active["favicon_url"].as_str().unwrap_or_default().to_string();
        let ready = browser_driver::cdp_eval_via_active(port, "document.readyState").await.unwrap();
        if ready == "complete" && favicon.ends_with("/brand.svg") { break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(favicon, format!("{origin}/brand.svg"));
    assert_eq!(image_requests.load(Ordering::Relaxed), 1);
    let start = std::time::Instant::now();
    browser_driver::page_navigate(port, &format!("{origin}/?repeat=1")).await.unwrap();
    println!("repeat navigation with cached image: {:?}", start.elapsed());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(image_requests.load(Ordering::Relaxed), 1, "repeat visits should reuse Chromium's HTTP cache");
    assert_eq!(browser_driver::close_chromium().await, browser_driver::CloseOutcome::Closed);
    server.abort();
    drop(context);
}

/// Every network assertion uses a local fixture: blocked requests must never
/// reach the server, normal scripts must execute, and CSS hides dynamic ads.
#[tokio::test]
#[ignore]
async fn live_adblock_blocks_before_download_and_can_be_disabled() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let context = TestContext::new("adblock").await;
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let logged = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let logged = logged.clone();
            tokio::spawn(async move {
                let mut bytes = vec![0; 8192];
                let Ok(size) = socket.read(&mut bytes).await else { return };
                let request = String::from_utf8_lossy(&bytes[..size]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                logged.lock().unwrap().push(path.clone());
                let (kind, body) = match path.as_str() {
                    "/" => ("text/html", "<!doctype html><title>shield fixture</title><script src='/normal.js'></script><script src='/banner_ads/blocked.js'></script><div id='ad' class='ad-banner-container'>advert</div><main id='article'>Article content</main>"),
                    "/normal.js" => ("application/javascript", "window.normalLoaded = true"),
                    "/banner_ads/blocked.js" => ("application/javascript", "window.adLoaded = true"),
                    _ => ("text/plain", "fixture"),
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    let port = browser_driver::ensure_chromium().await.unwrap();
    let url = format!("http://{address}/");
    browser_driver::page_navigate(port, &url).await.unwrap();
    let eval = |script: &'static str| browser_driver::cdp_eval_via_active(port, script);
    assert_eq!(eval("String(window.normalLoaded === true)").await.unwrap(), "true");
    assert_eq!(eval("String(window.adLoaded === undefined)").await.unwrap(), "true");
    assert!(!requests.lock().unwrap().iter().any(|p| p == "/banner_ads/blocked.js"), "blocked script reached server");
    async fn wait_hidden(port: u16, id: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let expression = format!("getComputedStyle(document.getElementById('{id}')).display");
            if browser_driver::cdp_eval_via_active(port, &expression).await.unwrap() == "none" { break; }
            assert!(tokio::time::Instant::now() < deadline, "uBlock cosmetic filter did not hide {id}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    wait_hidden(port, "ad").await;
    eval("const dynamic = document.createElement('div'); dynamic.id = 'dynamic'; dynamic.className = 'ad-banner-container'; document.body.appendChild(dynamic)").await.unwrap();
    wait_hidden(port, "dynamic").await;
    assert_eq!(eval("getComputedStyle(document.getElementById('article')).display").await.unwrap(), "block");
    browser_driver::set_adblock_enabled(false).await.unwrap();
    // Registered extension content scripts are removed for subsequent documents.
    browser_driver::page_navigate(port, &url).await.unwrap();
    assert_ne!(eval("getComputedStyle(document.getElementById('ad')).display").await.unwrap(), "none");
    assert_eq!(eval("String(window.adLoaded === true)").await.unwrap(), "true");
    assert!(requests.lock().unwrap().iter().any(|p| p == "/banner_ads/blocked.js"));
    requests.lock().unwrap().clear();
    browser_driver::set_adblock_enabled(true).await.unwrap();
    browser_driver::page_navigate(port, &url).await.unwrap();
    assert_eq!(eval("String(window.adLoaded === undefined)").await.unwrap(), "true");
    assert!(!requests.lock().unwrap().iter().any(|p| p == "/banner_ads/blocked.js"));
    wait_hidden(port, "ad").await;
    browser_driver::close_chromium().await;
    server.abort();
    drop(context);
}

/// Exercise the upstream extension's real MAIN-world YouTube scriptlets,
/// including their absence after disabling protection. All responses are local.
#[tokio::test]
#[ignore]
async fn live_ublock_youtube_scriptlets_filter_player_responses() {
    let _context = TestContext::new("ublock-youtube").await;
    let port = browser_driver::ensure_chromium().await.unwrap();
    browser_driver::page_navigate(port, "about:blank").await.unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/test-ublock-youtube.cjs");
    for enabled in [true, false, true] {
        browser_driver::set_adblock_enabled(enabled).await.unwrap();
        let output = tokio::process::Command::new("node").arg(&script).arg(port.to_string())
            .arg(if enabled { "on" } else { "off" }).output().await.expect("npm ci and Node are required for this fixture");
        assert!(output.status.success(), "YouTube scriptlet fixture failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        println!("{}", String::from_utf8_lossy(&output.stdout));
    }
    browser_driver::close_chromium().await;
}

#[tokio::test]
#[ignore]
async fn live_ublock_loads_into_owned_legacy_browser_and_preserves_page() {
    let context = TestContext::new("extension-upgrade").await;
    let profile = context.home.join("browser-profile");
    std::fs::create_dir_all(&profile).unwrap();
    let url = context.page("restored-after-extension-upgrade");
    // Simulate a still-running Comrade browser from before extension support.
    let mut legacy = tokio::process::Command::new(test_chromium_exe(&context.home))
        .args(["--headless=new", "--no-sandbox", "--no-first-run", "--remote-debugging-port=0"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let old_port = loop {
        if let Ok(info) = std::fs::read_to_string(profile.join("DevToolsActivePort")) {
            if let Some(port) = info.lines().next().and_then(|s| s.parse::<u16>().ok()) {
                if page_targets(port).await.iter().any(|page| page["url"] == url) { break port; }
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "legacy browser did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let port = browser_driver::ensure_chromium().await.expect("load extension into owned legacy browser");
    assert_eq!(port, old_port, "reuse our existing Chromium process");
    assert!(page_targets(port).await.iter().any(|page| page["url"] == url), "restore previous URL");
    let result = tokio::process::Command::new("node")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/test-ublock-youtube.cjs"))
        .arg(port.to_string()).arg("on").output().await.unwrap();
    assert!(result.status.success(), "extension in existing browser: {}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    browser_driver::close_chromium().await;
    legacy.kill().await.unwrap();
    legacy.wait().await.unwrap();
}
