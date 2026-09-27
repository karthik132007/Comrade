/** Structured logs. Never logs secrets — callers must pass redacted values. */
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
pub enum Level {
    Info,
    Agent,
    Tool,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Info => "INFO ",
            Level::Agent => "AGENT",
            Level::Tool => "TOOL ",
            Level::Warn => "WARN ",
            Level::Error => "ERROR",
        }
    }
}

fn timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // UTC wall clock is enough for local diagnostics.
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

const REDACT_KEYS: &[&str] = &["key", "token", "secret", "password", "cookie", "auth"];

pub fn redact(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > 64 {
                serde_json::Value::String(format!("{}…(redacted,len={})", &s[..8.min(s.len())], s.len()))
            } else {
                value.clone()
            }
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let kl = k.to_lowercase();
                if REDACT_KEYS.iter().any(|r| kl.contains(r)) {
                    out.insert(k.clone(), serde_json::Value::String("(redacted)".into()));
                } else {
                    out.insert(k.clone(), redact(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact).collect())
        }
        _ => value.clone(),
    }
}

pub fn log(level: Level, tag: &str, message: &str, data: Option<&serde_json::Value>) {
    match data {
        Some(d) => println!("[{}] {} {tag}: {message} {}", timestamp(), level.label(), redact(d)),
        None => println!("[{}] {} {tag}: {message}", timestamp(), level.label()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_secret_keys_and_long_strings() {
        let v = json!({"api_key": "sk-abcdef", "note": "short", "blob": "x".repeat(100)});
        let r = redact(&v);
        assert_eq!(r["api_key"], json!("(redacted)"));
        assert_eq!(r["note"], json!("short"));
        assert!(r["blob"].as_str().unwrap().contains("redacted"));
    }
}
