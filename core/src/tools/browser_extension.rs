//! Install the pinned, unmodified official uBlock Origin Lite package into
//! Comrade's own profile. Wait for Complete-mode registration before browsing.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub const VERSION: &str = "2026.930.1227";
const SHA256: &str = "13dd17bcc9720abbf5006793b0a4f7b672d34b394db2e6198daa7ef282b20361";
const PACKAGE: &[u8] =
    include_bytes!("../../assets/extensions/ublock-origin-lite-2026.930.1227.zip");
struct Loaded {
    port: u16,
    home: PathBuf,
    browser_ws: String,
    id: String,
    enabled: bool,
}
static LOADED: OnceLock<Mutex<Option<Loaded>>> = OnceLock::new();
fn state() -> &'static Mutex<Option<Loaded>> {
    LOADED.get_or_init(|| Mutex::new(None))
}

fn unpack(home: PathBuf) -> Result<PathBuf, String> {
    let parent = home.join("browser-extensions");
    let target = parent.join(format!("ublock-origin-lite-{VERSION}"));
    let marker = target.join(".comrade-package-sha256");
    if std::fs::read_to_string(&marker).is_ok_and(|value| value == SHA256)
        && std::fs::read_to_string(target.join("manifest.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .is_some_and(|m| m["version"] == VERSION && m["manifest_version"] == 3)
    {
        return Ok(target);
    }
    if format!("{:x}", Sha256::digest(PACKAGE)) != SHA256 {
        return Err("UBLOCK_PACKAGE: bundled checksum mismatch".into());
    }
    std::fs::create_dir_all(&parent).map_err(|e| format!("UBLOCK_INSTALL: {e}"))?;
    let staging = parent.join(format!(".ublock-{VERSION}-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(PACKAGE)).map_err(|e| e.to_string())?;
        let mut total = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            total += entry.size();
            if total > 256 * 1024 * 1024 {
                return Err("extension exceeds extraction limit".into());
            }
            if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err("extension contains symlink".into());
            }
            let relative = entry
                .enclosed_name()
                .ok_or("unsafe extension archive path")?;
            let output = staging.join(relative);
            if entry.is_dir() {
                std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
            } else {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut file = std::fs::File::create(output).map_err(|e| e.to_string())?;
                std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
            }
        }
        std::fs::write(staging.join(".comrade-package-sha256"), SHA256)
            .map_err(|e| e.to_string())?;
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(staging);
    }
    result.map_err(|e| format!("UBLOCK_INSTALL: {e}"))?;
    Ok(target)
}
async fn browser_session(port: u16) -> Result<String, String> {
    let info: Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    info["webSocketDebuggerUrl"]
        .as_str()
        .map(str::to_owned)
        .ok_or("UBLOCK_INIT: no browser websocket".into())
}
async fn call(ws: &str, method: &str, params: Value) -> Result<Value, String> {
    super::cdp_transport::session(ws)
        .await?
        .call(method, params)
        .await
}

async fn configure(loaded: &Loaded, enabled: bool) -> Result<(), String> {
    // The upstream message handler only accepts messages from its own origin.
    // A temporary extension page configures it, then is always closed.
    let result = call(
        &loaded.browser_ws,
        "Target.createTarget",
        json!({
            "url": format!("chrome-extension://{}/dashboard.html", loaded.id), "background": true
        }),
    )
    .await?;
    let id = result["targetId"]
        .as_str()
        .ok_or("UBLOCK_INIT: no settings target")?;
    let operation = async {
        let deadline = Instant::now() + Duration::from_secs(15);
        let client = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().map_err(|e| e.to_string())?;
        let ws = loop {
            let targets: Vec<Value> = client.get(format!("http://127.0.0.1:{}/json/list", loaded.port)).send().await
                .map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
            if let Some(ws) = targets.iter().find(|t| t["id"] == id).and_then(|t| t["webSocketDebuggerUrl"].as_str()) { break ws.to_owned(); }
            if Instant::now() > deadline { return Err("UBLOCK_INIT: extension settings unavailable".into()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let level = if enabled { 3 } else { 0 };
        let expression = format!(r#"(async () => {{
            if (!globalThis.chrome?.runtime?.id) return {{ready:false}};
            const data = await chrome.runtime.sendMessage({{what:'getOptionsPageData'}});
            if (!data?.hasOmnipotence) throw new Error('uBlock site permissions unavailable');
            await chrome.runtime.sendMessage({{what:'setDefaultFilteringMode',level:{level}}});
            const mode = await chrome.runtime.sendMessage({{what:'getDefaultFilteringMode'}});
            const scripts = await chrome.scripting.getRegisteredContentScripts();
            const rules = await chrome.declarativeNetRequest.getEnabledRulesets();
            return {{ready:true,mode,scripts:scripts.length,mainScriptlets:scripts.some(s=>s.world==='MAIN'&&s.runAt==='document_start'),rules:rules.length,version:chrome.runtime.getManifest().version}};
        }})()"#);
        loop {
            let result = call(&ws, "Runtime.evaluate", json!({"expression":expression, "awaitPromise":true,"returnByValue":true})).await?;
            if result.get("exceptionDetails").is_some() { return Err(format!("UBLOCK_INIT: extension configuration failed: {}", result["exceptionDetails"])); }
            let value = &result["result"]["value"];
            if value["ready"] == true {
                if value["version"] != VERSION || value["mode"] != level || value["rules"].as_u64().unwrap_or(0) == 0
                    || (enabled && (value["scripts"].as_u64().unwrap_or(0) == 0 || value["mainScriptlets"] != true)) {
                    return Err(format!("UBLOCK_INIT: filtering not ready: {value}"));
                }
                return Ok(());
            }
            if Instant::now() > deadline { return Err("UBLOCK_INIT: extension did not initialize".into()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }.await;
    let _ = call(
        &loaded.browser_ws,
        "Target.closeTarget",
        json!({"targetId":id}),
    )
    .await;
    operation
}

pub(super) async fn ensure_loaded(port: u16) -> Result<(), String> {
    let mut slot = state().lock().await;
    let home = crate::paths::comrade_home();
    if slot
        .as_ref()
        .is_some_and(|s| s.port == port && s.home == home)
    {
        return Ok(());
    }
    let install_home = home.clone();
    let path = tokio::task::spawn_blocking(move || unpack(install_home))
        .await
        .map_err(|e| e.to_string())??;
    let browser_ws = browser_session(port).await?;
    let extensions = call(&browser_ws, "Extensions.getExtensions", json!({}))
        .await
        .map_err(|e| format!("UBLOCK_LOAD: {e}"))?;
    let path_string = path.to_string_lossy();
    let existing = extensions["extensions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| {
            e["path"] == path_string.as_ref() && e["version"] == VERSION && e["enabled"] == true
        })
        .and_then(|e| e["id"].as_str());
    let id = match existing {
        Some(id) => id.to_owned(),
        None => call(
            &browser_ws,
            "Extensions.loadUnpacked",
            json!({"path":path_string}),
        )
        .await
        .map_err(|e| format!("UBLOCK_LOAD: {e}"))?["id"]
            .as_str()
            .ok_or("UBLOCK_LOAD: no extension id")?
            .to_owned(),
    };
    let enabled = crate::prefs::load().browser.adblock_enabled;
    let loaded = Loaded {
        port,
        home,
        browser_ws,
        id,
        enabled,
    };
    configure(&loaded, enabled).await?;
    *slot = Some(loaded);
    Ok(())
}
pub(super) async fn set_enabled(port: u16, enabled: bool) -> Result<(), String> {
    ensure_loaded(port).await?;
    let mut slot = state().lock().await;
    if let Some(loaded) = slot.as_mut() {
        if loaded.enabled != enabled {
            configure(loaded, enabled).await?;
            loaded.enabled = enabled;
        }
    }
    Ok(())
}
pub(super) async fn forget(port: u16) {
    let mut slot = state().lock().await;
    if slot.as_ref().is_some_and(|s| s.port == port) {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_package_is_pinned_and_extracts_offline() {
        let home =
            std::env::temp_dir().join(format!("comrade-ubol-package-{}", std::process::id()));
        let path = unpack(home.clone()).unwrap();
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(path.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["manifest_version"], 3);
        assert_eq!(manifest["version"], VERSION);
        assert_eq!(manifest["host_permissions"], json!(["<all_urls>"]));
        assert!(path.join("LICENSE.txt").is_file());
        assert_eq!(unpack(home.clone()).unwrap(), path);
        std::fs::remove_dir_all(home).unwrap();
    }
}
