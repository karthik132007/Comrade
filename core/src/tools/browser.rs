/**
 * Browser tools. Honest stubs: backed by Playwright + a persistent
 * Brave/Chromium profile in a later phase — never cookie extraction.
 */
use super::computer::StubTool;

const BROWSER_TOOLS: &[(&str, &str)] = &[
    ("browser.open", "Open a URL in the Comrade browser profile."),
    ("browser.back", "Navigate back."),
    ("browser.forward", "Navigate forward."),
    ("browser.refresh", "Reload the current page."),
    ("browser.click", "Click a selector."),
    ("browser.type", "Type text into a selector."),
    ("browser.press", "Press a key."),
    ("browser.scroll", "Scroll the page."),
    ("browser.getPageText", "Read visible page text."),
    ("browser.getTitle", "Read the page title."),
    ("browser.screenshot", "Capture a page screenshot."),
    ("browser.currentUrl", "Return the current URL."),
];

pub fn browser_stubs() -> Vec<StubTool> {
    BROWSER_TOOLS
        .iter()
        .map(|(tool_name, blurb)| StubTool { tool_name, blurb })
        .collect()
}
