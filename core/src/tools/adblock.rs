//! Brave's adblock-rust engine. Bundled lists protect the first request offline;
//! validated cached/downloaded lists replace the engine off the browser IO task.
use ::adblock::{
    lists::{FilterSet, ParseOptions},
    request::Request,
    Engine,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::{Duration, SystemTime},
};
use tokio::sync::OnceCell;

const EASYLIST: &str = include_str!("../../assets/adblock/easylist.txt");
const EASYPRIVACY: &str = include_str!("../../assets/adblock/easyprivacy.txt");
const MAX_LIST_BYTES: usize = 16 * 1024 * 1024;
const UPDATE_INTERVAL: Duration = Duration::from_secs(4 * 24 * 60 * 60);
static SHIELD: OnceCell<Arc<Shield>> = OnceCell::const_new();

pub(super) struct Shield {
    enabled: AtomicBool,
    engine: RwLock<Engine>,
}
fn compile(lists: [String; 2]) -> Engine {
    let mut filters = FilterSet::new(false);
    for list in lists {
        filters.add_filter_list(list, ParseOptions::default());
    }
    Engine::new_with_filter_set(filters)
}
fn valid_list(text: &str, title: &str) -> bool {
    text.len() <= MAX_LIST_BYTES
        && text.starts_with("[Adblock Plus ")
        && text
            .lines()
            .take(30)
            .any(|line| line == format!("! Title: {title}"))
        && text.lines().count() > 1000
}
fn cached_list(dir: &std::path::Path, name: &str, title: &str, bundled: &str) -> String {
    let path = dir.join(name);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_LIST_BYTES as u64) {
        if let Ok(text) = std::fs::read_to_string(path) {
            if valid_list(&text, title) {
                return text;
            }
        }
    }
    bundled.to_owned()
}

pub(super) async fn ensure() -> Result<Arc<Shield>, String> {
    SHIELD
        .get_or_try_init(|| async {
            let dir = crate::paths::comrade_home().join("adblock");
            let load_dir = dir.clone();
            let engine = tokio::task::spawn_blocking(move || {
                compile([
                    cached_list(&load_dir, "easylist.txt", "EasyList", EASYLIST),
                    cached_list(&load_dir, "easyprivacy.txt", "EasyPrivacy", EASYPRIVACY),
                ])
            })
            .await
            .map_err(|e| format!("ADBLOCK_INIT: {e}"))?;
            let enabled = crate::prefs::load().browser.adblock_enabled;
            let shield = Arc::new(Shield {
                enabled: AtomicBool::new(enabled),
                engine: RwLock::new(engine),
            });
            if std::env::var("COMRADE_ADBLOCK_UPDATE").as_deref() != Ok("0") {
                let updating = shield.clone();
                tokio::spawn(async move {
                    loop {
                        refresh(&updating, &dir).await;
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                });
            }
            Ok(shield)
        })
        .await
        .cloned()
}

pub(super) fn current() -> Option<&'static Arc<Shield>> {
    SHIELD.get()
}
pub(super) fn set_enabled(enabled: bool) {
    if let Some(shield) = current() {
        shield.enabled.store(enabled, Ordering::Release);
    }
}
impl Shield {
    pub(super) fn blocks(&self, url: &str, source: &str, kind: &str, method: &str) -> bool {
        if !self.enabled.load(Ordering::Acquire) || kind == "document" {
            return false;
        }
        let Ok(request) = Request::new(url, source, kind, method) else {
            return false;
        };
        self.engine
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .check_network_request(&request)
            .should_block()
    }
    pub(super) fn selectors(&self, url: &str, classes: &[String], ids: &[String]) -> Vec<String> {
        if !self.enabled.load(Ordering::Acquire) {
            return vec![];
        }
        let engine = self.engine.read().unwrap_or_else(|e| e.into_inner());
        let resources = engine.url_cosmetic_resources(url);
        let mut selectors: Vec<_> = resources.hide_selectors.into_iter().collect();
        if !resources.generichide {
            selectors.extend(engine.hidden_class_id_selectors(classes, ids, &resources.exceptions));
        }
        selectors
    }
}
async fn refresh(shield: &Arc<Shield>, dir: &PathBuf) {
    let fresh = ["easylist.txt", "easyprivacy.txt"].iter().all(|name| {
        std::fs::metadata(dir.join(name))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|when| SystemTime::now().duration_since(when).ok())
            .is_some_and(|age| age < UPDATE_INTERVAL)
    });
    if fresh {
        return;
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
    else {
        return;
    };
    let download = async |name: &str, title: &str| -> Result<String, String> {
        let mut response = client
            .get(format!("https://easylist.to/easylist/{name}"))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            if bytes.len() + chunk.len() > MAX_LIST_BYTES {
                return Err("filter list too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        if !valid_list(&text, title) {
            return Err("invalid filter list".into());
        }
        Ok(text)
    };
    let (Ok(easylist), Ok(easyprivacy)) = tokio::join!(
        download("easylist.txt", "EasyList"),
        download("easyprivacy.txt", "EasyPrivacy")
    ) else {
        return;
    };
    let cache = dir.clone();
    let Ok(engine) = tokio::task::spawn_blocking(move || {
        // Keep the previous valid cache if a download or write fails.
        if std::fs::create_dir_all(&cache).is_ok() {
            for (name, text) in [
                ("easylist.txt", &easylist),
                ("easyprivacy.txt", &easyprivacy),
            ] {
                let temporary = cache.join(format!("{name}.tmp"));
                if std::fs::write(&temporary, text).is_ok() {
                    let _ = std::fs::rename(temporary, cache.join(name));
                }
            }
        }
        compile([easylist, easyprivacy])
    })
    .await
    else {
        return;
    };
    let previous = std::mem::replace(
        &mut *shield.engine.write().unwrap_or_else(|e| e.into_inner()),
        engine,
    );
    tokio::task::spawn_blocking(move || drop(previous));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brave_engine_honors_request_types_third_party_and_exceptions() {
        let shield = Shield { enabled: AtomicBool::new(true), engine: RwLock::new(compile([
            "||ads.example^$script,third-party\n@@||ads.example/allowed.js$script\n||tracker.example^\n##.advertisement\nnews.example###sponsor\nnews.example#@#.advertisement".into(), String::new()
        ])) };
        assert!(shield.blocks(
            "https://ads.example/ad.js",
            "https://news.example",
            "script",
            "GET"
        ));
        assert!(!shield.blocks(
            "https://ads.example/allowed.js",
            "https://news.example",
            "script",
            "GET"
        ));
        assert!(!shield.blocks(
            "https://ads.example/ad.js",
            "https://ads.example",
            "script",
            "GET"
        ));
        assert!(!shield.blocks(
            "https://ads.example/ad.js",
            "https://news.example",
            "image",
            "GET"
        ));
        assert!(shield.blocks(
            "https://tracker.example/pixel",
            "https://news.example",
            "image",
            "GET"
        ));
        let selectors = shield.selectors(
            "https://news.example",
            &["advertisement".into()],
            &["sponsor".into()],
        );
        assert!(selectors.contains(&"#sponsor".into()));
        assert!(!selectors.contains(&".advertisement".into()));
        shield.enabled.store(false, Ordering::Release);
        assert!(!shield.blocks(
            "https://tracker.example/pixel",
            "https://news.example",
            "image",
            "GET"
        ));
        assert!(shield
            .selectors("https://news.example", &[], &[])
            .is_empty());
    }
    #[test]
    fn bundled_lists_are_valid_and_block_known_ad_and_analytics_hosts() {
        assert!(valid_list(EASYLIST, "EasyList"));
        assert!(valid_list(EASYPRIVACY, "EasyPrivacy"));
        assert!(!valid_list("<html>server error</html>", "EasyList"));
        let shield = Shield {
            enabled: AtomicBool::new(true),
            engine: RwLock::new(compile([EASYLIST.into(), EASYPRIVACY.into()])),
        };
        assert!(shield.blocks(
            "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
            "https://news.example",
            "script",
            "GET"
        ));
        assert!(shield.blocks(
            "https://www.google-analytics.com/analytics.js",
            "https://news.example",
            "script",
            "GET"
        ));
        assert!(!shield.blocks(
            "https://news.example/article.js",
            "https://news.example",
            "script",
            "GET"
        ));
        assert!(shield
            .selectors(
                "http://127.0.0.2:43210/",
                &["ad-banner-container".into()],
                &[]
            )
            .contains(&".ad-banner-container".into()));
        // EasyList deliberately exempts localhost from generic cosmetic rules.
        assert!(shield
            .selectors(
                "http://127.0.0.1:43210/",
                &["ad-banner-container".into()],
                &[]
            )
            .is_empty());
    }
}
