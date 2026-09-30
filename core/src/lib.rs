pub mod agent;
pub mod config;
pub mod environment;
pub mod history;
pub mod intent;
pub mod llm;
pub mod logger;
pub mod memory;
pub mod paths;
pub mod permissions;
pub mod prefs;
pub mod service;
pub mod task_state;
pub mod tools;
pub mod voice;

/// Process-global env vars (COMRADE_HOME, COMRADE_CHROMIUM_BIN) are mutated
/// by tests — holders of this lock serialize those tests so parallel runs
/// can't observe each other's values. Grab it at the top of any test that
/// sets or removes env vars.
#[cfg(test)]
pub static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap()
}
