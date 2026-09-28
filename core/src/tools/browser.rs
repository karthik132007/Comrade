/**
 * Browser tools — real Chromium-family automation over CDP.
 *
 * Any Chromium fork (Chrome, Brave, Edge, Opera, Vivaldi, Chromium) works:
 * the exe comes from Settings (`comrade.conf` [browser]), launched with
 * `--remote-debugging-port` against the persistent `browser-profile`.
 * Firefox-family browsers are detected and fail with FIREFOX_UNSUPPORTED
 * (WebDriver BiDi backend planned; same tool names will light up).
 */
use std::future::Future;
use std::pin::Pin;

use super::browser_driver;
use super::types::{Risk, Tool, ToolContext, ToolResult};

async fn port_or_err() -> Result<u16, ToolResult> {
    browser_driver::ensure_chromium()
        .await
        .map_err(|msg| split_err(&msg))
}

fn split_err(msg: &str) -> ToolResult {
    if let Some((code, rest)) = msg.split_once(": ") {
        if code.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            return ToolResult::fail(code, rest.to_string());
        }
    }
    ToolResult::fail("BROWSER_ERROR", msg.to_string())
}

macro_rules! simple_tool {
    ($name:ident, $tool_name:literal, $desc:literal, $params:expr, $risk:expr, $run:path) => {
        pub struct $name;
        impl Tool for $name {
            fn name(&self) -> &'static str { $tool_name }
            fn description(&self) -> &'static str { $desc }
            fn parameters(&self) -> serde_json::Value { $params }
            fn risk(&self, _args: &serde_json::Value) -> Risk { $risk }
            fn execute<'a>(
                &'a self,
                args: &'a serde_json::Value,
                _ctx: &'a ToolContext,
            ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
                Box::pin($run(args))
            }
        }
    };
}

async fn run_open(args: &serde_json::Value) -> ToolResult {
    let url = args.get("url").and_then(|u| u.as_str()).unwrap_or("").trim();
    if url.is_empty() {
        return ToolResult::fail("EMPTY_URL", "No URL provided.");
    }
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_navigate(port, url).await {
        Ok((requested, final_url)) => ToolResult::ok(serde_json::json!({
            "requested": requested, "url": final_url,
        })),
        Err(e) => split_err(&e),
    }
}

async fn run_back(args: &serde_json::Value) -> ToolResult {
    let _ = args;
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match back_forward(port, false).await {
        Ok(url) => ToolResult::ok(serde_json::json!({ "url": url })),
        Err(e) => split_err(&e),
    }
}

async fn run_forward(args: &serde_json::Value) -> ToolResult {
    let _ = args;
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match back_forward(port, true).await {
        Ok(url) => ToolResult::ok(serde_json::json!({ "url": url })),
        Err(e) => split_err(&e),
    }
}

async fn back_forward(port: u16, forward: bool) -> Result<String, String> {
    // Fire the history navigation, wait for the page to settle, then read
    // the fresh URL (the evaluate reply itself carries the stale href).
    let _ = super::browser_driver::cdp_eval_via_active(port, if forward {
        "history.forward()"
    } else {
        "history.back()"
    }).await?;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    super::browser_driver::current_url(port).await
}

async fn run_refresh(args: &serde_json::Value) -> ToolResult {
    let _ = args;
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_reload(port).await {
        Ok(url) => ToolResult::ok(serde_json::json!({ "url": url })),
        Err(e) => split_err(&e),
    }
}

async fn run_click(args: &serde_json::Value) -> ToolResult {
    let selector = args.get("selector").and_then(|s| s.as_str()).unwrap_or("").trim();
    if selector.is_empty() {
        return ToolResult::fail("EMPTY_SELECTOR", "No CSS selector provided.");
    }
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_click(port, selector).await {
        Ok(tag) => ToolResult::ok(serde_json::json!({ "selector": selector, "clicked": tag })),
        Err(e) => split_err(&e),
    }
}

async fn run_type(args: &serde_json::Value) -> ToolResult {
    let selector = args.get("selector").and_then(|s| s.as_str()).unwrap_or("").trim();
    let text = args.get("text").and_then(|s| s.as_str()).unwrap_or("");
    if selector.is_empty() {
        return ToolResult::fail("EMPTY_SELECTOR", "No CSS selector provided.");
    }
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_type(port, selector, text).await {
        Ok(()) => ToolResult::ok(serde_json::json!({ "selector": selector, "typed": text.len() })),
        Err(e) => split_err(&e),
    }
}

async fn run_press(args: &serde_json::Value) -> ToolResult {
    let key = args.get("key").and_then(|s| s.as_str()).unwrap_or("").trim();
    if key.is_empty() {
        return ToolResult::fail("EMPTY_KEY", "No key provided.");
    }
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_press(port, key).await {
        Ok(()) => ToolResult::ok(serde_json::json!({ "key": key })),
        Err(e) => split_err(&e),
    }
}

async fn run_scroll(args: &serde_json::Value) -> ToolResult {
    let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
    let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or_else(|| {
        args.get("direction").and_then(|d| d.as_str()).map(|d| match d {
            "up" => -600,
            _ => 600,
        }).unwrap_or(600)
    });
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_scroll(port, x, y).await {
        Ok((sx, sy)) => ToolResult::ok(serde_json::json!({ "scrollX": sx, "scrollY": sy })),
        Err(e) => split_err(&e),
    }
}

async fn run_text(args: &serde_json::Value) -> ToolResult {
    let max = args.get("maxChars").and_then(|v| v.as_u64()).unwrap_or(12000).clamp(500, 100000) as usize;
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_text(port, max).await {
        Ok((text, url)) => ToolResult::ok(serde_json::json!({ "url": url, "text": text })),
        Err(e) => split_err(&e),
    }
}

async fn run_title(args: &serde_json::Value) -> ToolResult {
    let _ = args;
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_title(port).await {
        Ok((title, url)) => ToolResult::ok(serde_json::json!({ "title": title, "url": url })),
        Err(e) => split_err(&e),
    }
}

async fn run_shot(args: &serde_json::Value) -> ToolResult {
    let _ = args;
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::page_screenshot(port).await {
        Ok(path) => ToolResult::ok(serde_json::json!({ "path": path.to_string_lossy() })),
        Err(e) => split_err(&e),
    }
}

async fn run_url(args: &serde_json::Value) -> ToolResult {
    let _ = args;
    let port = match port_or_err().await { Ok(p) => p, Err(e) => return e };
    match browser_driver::current_url(port).await {
        Ok(url) => ToolResult::ok(serde_json::json!({ "url": url })),
        Err(e) => split_err(&e),
    }
}

async fn run_close(args: &serde_json::Value) -> ToolResult {
    let _ = args;
    match browser_driver::close_chromium().await {
        browser_driver::CloseOutcome::Closed => {
            ToolResult::ok(serde_json::json!({ "closed": true, "detached": false }))
        }
        browser_driver::CloseOutcome::Detached => {
            ToolResult::ok(serde_json::json!({
                "closed": false, "detached": true,
                "note": "Detached from your live browser; left it running.",
            }))
        }
        browser_driver::CloseOutcome::Idle => {
            ToolResult::ok(serde_json::json!({ "closed": false, "detached": false }))
        }
    }
}

simple_tool!(OpenTool, "browser.open", "Navigate the current Comrade tab in the browser selected in Settings. Reuses the same tab for successive URLs. Bare domains and search terms accepted. If browser setup fails, report the error; do not launch another browser or use a terminal URL opener.",
    serde_json::json!({"type":"object","properties":{"url":{"type":"string","description":"URL, domain, or search terms"}},"required":["url"]}), Risk::Safe, run_open);
simple_tool!(BackTool, "browser.back", "Navigate back in history.",
    serde_json::json!({"type":"object","properties":{}}), Risk::Safe, run_back);
simple_tool!(ForwardTool, "browser.forward", "Navigate forward in history.",
    serde_json::json!({"type":"object","properties":{}}), Risk::Safe, run_forward);
simple_tool!(RefreshTool, "browser.refresh", "Reload the current page.",
    serde_json::json!({"type":"object","properties":{}}), Risk::Safe, run_refresh);
simple_tool!(ClickTool, "browser.click", "Click a CSS selector (scrolls into view first). Requires approval.",
    serde_json::json!({"type":"object","properties":{"selector":{"type":"string"}},"required":["selector"]}), Risk::Dangerous, run_click);
simple_tool!(TypeTool, "browser.type", "Type text into a CSS selector (input, textarea, contenteditable). Requires approval.",
    serde_json::json!({"type":"object","properties":{"selector":{"type":"string"},"text":{"type":"string"}},"required":["selector","text"]}), Risk::Dangerous, run_type);
simple_tool!(PressTool, "browser.press", "Press a key: single character or Enter, Tab, Escape, Backspace, Delete, ArrowLeft/Up/Right/Down, Home, End, PageUp, PageDown. Requires approval.",
    serde_json::json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}), Risk::Dangerous, run_press);
simple_tool!(ScrollTool, "browser.scroll", "Scroll by pixels (x, y) or direction up/down.",
    serde_json::json!({"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"direction":{"type":"string"}}}), Risk::Safe, run_scroll);
simple_tool!(TextTool, "browser.getPageText", "Read visible page text (truncated to maxChars).",
    serde_json::json!({"type":"object","properties":{"maxChars":{"type":"number"}}}), Risk::Safe, run_text);
simple_tool!(TitleTool, "browser.getTitle", "Read the page title and URL.",
    serde_json::json!({"type":"object","properties":{}}), Risk::Safe, run_title);
simple_tool!(ShotTool, "browser.screenshot", "Capture a PNG screenshot to comrade-agent/screenshots/. Returns the file path.",
    serde_json::json!({"type":"object","properties":{}}), Risk::Safe, run_shot);
simple_tool!(UrlTool, "browser.currentUrl", "Return the current URL.",
    serde_json::json!({"type":"object","properties":{}}), Risk::Safe, run_url);
simple_tool!(CloseTool, "browser.close", "Close the managed browser.",
    serde_json::json!({"type":"object","properties":{}}), Risk::Safe, run_close);

/// Full registry handed to the agent loop.
pub fn browser_tools() -> Vec<std::sync::Arc<dyn super::types::Tool>> {
    vec![
        std::sync::Arc::new(OpenTool),
        std::sync::Arc::new(BackTool),
        std::sync::Arc::new(ForwardTool),
        std::sync::Arc::new(RefreshTool),
        std::sync::Arc::new(ClickTool),
        std::sync::Arc::new(TypeTool),
        std::sync::Arc::new(PressTool),
        std::sync::Arc::new(ScrollTool),
        std::sync::Arc::new(TextTool),
        std::sync::Arc::new(TitleTool),
        std::sync::Arc::new(ShotTool),
        std::sync::Arc::new(UrlTool),
        std::sync::Arc::new(CloseTool),
    ]
}
