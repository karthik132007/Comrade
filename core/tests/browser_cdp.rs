/** Live Chromium CDP checks. Ignored by default: needs a local Chromium browser.
 * Run single-threaded: `cargo test -p comrade-core --test browser_cdp -- --ignored --test-threads=1`
 * (both tests share the driver singleton + COMRADE_HOME).
 */
use comrade_core::tools::browser_driver;

#[tokio::test]
#[ignore]
async fn live_chromium_open_and_read() {
    // Self-contained: this conf (isolated profile, no attach probe).
    let home = std::env::var("COMRADE_HOME").expect("set COMRADE_HOME to a scratch dir");
    std::fs::write(
        format!("{home}/comrade.conf"),
        "[browser]\nexe = /usr/bin/google-chrome-stable\nkind = binary\nheadless = true\nprofile = comrade\ndebug_port = 0\n",
    )
    .unwrap();
    let port = browser_driver::ensure_chromium()
        .await
        .expect("launch chromium (set [browser] exe in comrade.conf)");
    let (req, url) = browser_driver::page_navigate(port, "example.com").await.expect("navigate");
    assert!(url.contains("example.com"), "unexpected url: {url} (req {req})");
    let (title, _) = browser_driver::page_title(port).await.expect("title");
    assert!(title.to_lowercase().contains("example"), "unexpected title: {title}");
    let (text, _) = browser_driver::page_text(port, 2000).await.expect("text");
    assert!(text.contains("Example"), "unexpected text: {}", text.chars().take(120).collect::<String>());
    let path = browser_driver::page_screenshot(port).await.expect("screenshot");
    assert!(path.exists(), "screenshot missing: {}", path.display());
    assert_eq!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Closed,
        "launched browser must be killed on close"
    );
}

/// Attach path: a browser the USER started (their profile, their window)
/// is driven without a relaunch, and close only detaches — never kills it.
#[tokio::test]
#[ignore]
async fn live_attach_to_user_browser() {
    use std::time::Duration;

    let home = std::env::var("COMRADE_HOME").expect("set COMRADE_HOME to a scratch dir");
    let debug_port: u16 = 9334;
    let profile = format!("{home}/attach-profile");
    let _ = std::fs::create_dir_all(&profile);

    // Pretend to be the user's own running browser: real exe, own profile.
    let mut child = tokio::process::Command::new("/usr/bin/google-chrome-stable")
        .args([
            format!("--remote-debugging-port={debug_port}"),
            "--remote-allow-origins=*".to_string(),
            format!("--user-data-dir={profile}"),
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
        .spawn()
        .expect("spawn user browser");
    let probe = format!("http://127.0.0.1:{debug_port}/json/version");
    let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).build().unwrap();
    let mut ready = false;
    for _ in 0..60 {
        if client.get(&probe).send().await.map(|r| r.status().is_success()).unwrap_or(false) {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(ready, "user browser never exposed DevTools");

    // Point Comrade at it: user profile + attach port.
    std::fs::write(
        format!("{home}/comrade.conf"),
        format!(
            "[browser]\nexe = /usr/bin/google-chrome-stable\nkind = binary\nheadless = true\nprofile = user\ndebug_port = {debug_port}\n"
        ),
    )
    .unwrap();

    let port = browser_driver::ensure_chromium().await.expect("attach to user browser");
    assert_eq!(port, debug_port, "must attach, not relaunch on a fresh port");
    let (_, url) = browser_driver::page_navigate(port, "example.com").await.expect("navigate");
    assert!(url.contains("example.com"), "unexpected url: {url}");
    assert_eq!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Detached,
        "close must detach, never kill the user's browser"
    );
    // Still alive after detach.
    assert!(
        client.get(&probe).send().await.map(|r| r.status().is_success()).unwrap_or(false),
        "user browser was killed by detach!"
    );
    let _ = child.kill().await;
}
