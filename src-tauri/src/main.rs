/** Comrade desktop shell (Tauri v2): window + commands + agent wiring. */
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use comrade_core::agent::{Agent, AgentCallbacks, AgentDeps};
use comrade_core::config;
use comrade_core::llm::factory::{create_configured, embedding_key};
use comrade_core::llm::{Embedder, OpenRouterEmbedder, OpenRouterProvider};
use comrade_core::logger::{log, Level};
use comrade_core::history::{title_for, ChatMessageRow, ChatSession, HistoryStore};
use comrade_core::memory::{import_chatgpt_json, import_text, migrate_legacy_json, MemoryItem, MemoryStore, ScoredMemory};
use comrade_core::paths;
use comrade_core::speech::{FallbackTts, OpenRouterStt, OpenRouterTts, SpeechToText, TextToSpeech};
use comrade_core::task_state::{TaskState, TaskStatus};
use comrade_core::tools::{build_tools, opencode};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

type Shared = Arc<AppState>;

struct AppState {
    agent: Agent<OpenRouterProvider, OpenRouterEmbedder>,
    memory: Arc<std::sync::Mutex<MemoryStore>>,
    history: Arc<std::sync::Mutex<HistoryStore>>,
    embedder: Arc<OpenRouterEmbedder>,
    provider: String,
    model: String,
    embedding_model: String,
    stt: OpenRouterStt,
    tts: FallbackTts,
    cancel: AtomicBool,
    task_lock: tokio::sync::Mutex<()>,
    pending: tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    perm_counter: AtomicU64,
}

fn truncate_err(e: impl std::fmt::Display, n: usize) -> String {
    format!("{e}").chars().take(n).collect()
}

fn emit(app: &AppHandle, payload: serde_json::Value) {
    let _ = app.emit("agent-event", payload);
}

struct Callbacks {
    app: AppHandle,
    shared: Shared,
}

impl AgentCallbacks for Callbacks {
    fn on_ui_state(&self, state: &str) {
        emit(&self.app, serde_json::json!({ "type": "state", "state": state }));
    }

    fn on_step(&self, label: &str, status: &str, detail: Option<&str>) {
        emit(
            &self.app,
            serde_json::json!({ "type": "step", "label": label, "status": status, "detail": detail }),
        );
    }

    fn on_token(&self, token: &str) {
        emit(&self.app, serde_json::json!({ "type": "token", "token": token }));
    }

    fn is_cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::SeqCst)
    }

    async fn request_approval(&self, summary: &str) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let n = self.shared.perm_counter.fetch_add(1, Ordering::SeqCst);
        let id = format!("perm-{}-{n}", now_ms());
        self.shared.pending.lock().await.insert(id.clone(), tx);
        let _ = self.app.emit(
            "permission-request",
            serde_json::json!({ "id": id, "summary": summary }),
        );
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                // Timeout or sender dropped: default deny.
                self.shared.pending.lock().await.remove(&id);
                false
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Rootless WebKit: distro builds hard-code /usr/lib/webkit2gtk-4.1 for helper
/// processes. When that path is absent (no sudo to install it), re-exec under
/// the LD_PRELOAD path shim + vendored sysroot so spawns/dlopens redirect.
/// See shim/webkit-path-shim.c. No-op on systems with a real install.
fn ensure_webkit_paths() {
    const SYSTEM_HELPER: &str = "/usr/lib/webkit2gtk-4.1/WebKitNetworkProcess";
    if std::path::Path::new(SYSTEM_HELPER).exists() {
        return;
    }
    if std::env::var("COMRADE_SHIMMED").is_ok() {
        return; // already under the shim; nothing more to do here.
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let helper_dir = std::env::var("COMRADE_WEBKIT_DIR").unwrap_or_else(|_| {
        paths::webkit_helper_dir().to_string_lossy().to_string()
    });
    let shim = std::env::var("COMRADE_SHIM").unwrap_or_else(|_| {
        paths::webkit_shim_path().to_string_lossy().to_string()
    });
    if !std::path::Path::new(&format!("{helper_dir}/WebKitNetworkProcess")).exists()
        || !std::path::Path::new(&shim).exists()
    {
        eprintln!(
            "Comrade: {SYSTEM_HELPER} missing and no vendored copy at {helper_dir}. \
             Run ./scripts/setup-linux.sh (or: sudo pacman -S webkit2gtk-4.1)."
        );
        return;
    }
    eprintln!("Comrade: using vendored WebKit helpers via path shim; re-execing.");
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().expect("current exe");
    let sys_lib = std::path::Path::new(&helper_dir)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{home}/comrade-sysroot/usr/lib"));
    let preload = match std::env::var("LD_PRELOAD") {
        Ok(p) if !p.is_empty() => format!("{shim}:{p}"),
        _ => shim.clone(),
    };
    let lib_path = match std::env::var("LD_LIBRARY_PATH") {
        Ok(p) if !p.is_empty() => format!("{sys_lib}:{p}"),
        _ => sys_lib,
    };
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("LD_PRELOAD", preload)
        .env("LD_LIBRARY_PATH", lib_path)
        .env("COMRADE_WEBKIT_DIR", &helper_dir)
        .env("COMRADE_SHIMMED", "1")
        .exec();
    eprintln!("Comrade: re-exec failed: {err}");
}

fn create_session(shared: &Shared, text: &str) -> String {
    shared
        .history
        .lock()
        .ok()
        .and_then(|h| h.create_session(&title_for(text)).ok())
        .unwrap_or_else(|| format!("ses-fallback-{}", now_ms()))
}

async fn persist_turn(shared: &Shared, sid: &str, user_text: &str, task: &TaskState) {
    let Ok(h) = shared.history.lock() else {
        return;
    };
    let _ = h.add_message(sid, "user", user_text);
    if let Some(r) = &task.result {
        let _ = h.add_message(sid, "comrade", r);
    } else if let Some(e) = &task.error {
        let _ = h.add_message(sid, "comrade", &format!("Error: {e}"));
    }
    let _ = h.touch(sid);
}

async fn execute_task(
    app: &AppHandle,
    shared: &Shared,
    text: &str,
    session_id: Option<String>,
) -> (TaskState, String) {
    log(Level::Info, "USER", &text.chars().take(300).collect::<String>(), None);
    // Resolve the chat session, creating one for the first message.
    let wanted = session_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let sid = match wanted {
        Some(s) => {
            let exists = shared.history.lock().map(|h| h.session_exists(&s)).unwrap_or(false);
            if exists {
                s
            } else {
                create_session(shared, text)
            }
        }
        None => create_session(shared, text),
    };
    let cb = Callbacks { app: app.clone(), shared: shared.clone() };
    let task = shared.agent.run_task(text, &cb).await;
    emit(app, serde_json::json!({ "type": "state", "state": "idle" }));
    emit(
        app,
        serde_json::json!({
            "type": "done",
            "status": task.status.as_str(),
            "result": task.result,
            "error": task.error,
        }),
    );
    log(
        Level::Agent,
        "done",
        task.status.as_str(),
        Some(&serde_json::json!({
            "result": task.result.clone().unwrap_or_default().chars().take(300).collect::<String>(),
        })),
    );
    persist_turn(shared, &sid, text, &task).await;
    (task, sid)
}

#[derive(Serialize)]
struct TaskSummary {
    status: String,
    result: Option<String>,
    error: Option<String>,
    session_id: String,
}

#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: State<'_, Shared>,
    text: String,
    session_id: Option<String>,
) -> Result<TaskSummary, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Empty message.".into());
    }
    let shared = state.inner().clone();
    let _guard = shared
        .task_lock
        .try_lock()
        .map_err(|_| "A task is already running. Cancel it first.".to_string())?;
    shared.cancel.store(false, Ordering::SeqCst);
    let (task, sid) = execute_task(&app, &shared, &text, session_id).await;
    Ok(TaskSummary {
        status: task.status.as_str().to_string(),
        result: task.result,
        error: task.error,
        session_id: sid,
    })
}

#[derive(Serialize)]
struct VoiceResult {
    status: String,
    session_id: String,
    transcript: Option<String>,
    result: Option<String>,
    error: Option<String>,
    audio_base64: Option<String>,
    mime_type: Option<String>,
    tts_error: Option<String>,
}

#[tauri::command]
async fn voice_input(
    app: AppHandle,
    state: State<'_, Shared>,
    audio_base64: String,
    format: String,
    session_id: Option<String>,
) -> Result<VoiceResult, String> {
    let shared = state.inner().clone();
    let _guard = shared
        .task_lock
        .try_lock()
        .map_err(|_| "A task is already running. Cancel it first.".to_string())?;
    if audio_base64.is_empty() {
        return Err("No audio received.".into());
    }
    shared.cancel.store(false, Ordering::SeqCst);
    emit(&app, serde_json::json!({ "type": "state", "state": "thinking" }));

    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &audio_base64)
        .map_err(|e| truncate_err(format!("Invalid audio encoding: {e}"), 200))?;
    let format = match format.as_str() {
        "wav" | "mp3" | "flac" | "m4a" | "ogg" | "webm" | "aac" => format,
        _ => "webm".to_string(),
    };
    let transcript = shared
        .stt
        .transcribe(&decoded, None, &format)
        .await
        .map_err(|e| truncate_err(e, 500))?;
    log(
        Level::Info,
        "STT",
        "transcribed voice input",
        Some(&serde_json::json!({ "transcript": transcript.chars().take(200).collect::<String>() })),
    );
    if transcript.trim().is_empty() {
        emit(&app, serde_json::json!({ "type": "state", "state": "idle" }));
        return Err("I could not hear anything. Try again.".into());
    }
    emit(&app, serde_json::json!({ "type": "transcript", "transcript": transcript }));

    let (task, sid) = execute_task(&app, &shared, &transcript, session_id).await;
    let spoken = task.result.clone().or_else(|| task.error.clone()).unwrap_or_default();
    let mut audio_out = None;
    let mut mime_out = None;
    let mut tts_error = None;
    if task.status == TaskStatus::Done && !spoken.trim().is_empty() {
        match shared.tts.synthesize(&spoken, None).await {
            Ok(speech) => {
                audio_out = Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &speech.audio,
                ));
                mime_out = Some(speech.mime_type);
            }
            Err(e) => {
                tts_error = Some(truncate_err(e, 200));
                log(Level::Warn, "TTS", "speech synthesis failed, returning text only", None);
            }
        }
    }
    let status = task.status.as_str().to_string();
    Ok(VoiceResult {
        status,
        session_id: sid,
        transcript: Some(transcript),
        result: task.result,
        error: task.error,
        audio_base64: audio_out,
        mime_type: mime_out,
        tts_error,
    })
}

#[tauri::command]
async fn cancel_task(app: AppHandle, state: State<'_, Shared>) -> Result<(), String> {
    state.cancel.store(true, Ordering::SeqCst);
    emit(&app, serde_json::json!({ "type": "state", "state": "idle" }));
    Ok(())
}

#[tauri::command]
async fn permission_response(state: State<'_, Shared>, id: String, approved: bool) -> Result<(), String> {
    let sender = state.pending.lock().await.remove(&id);
    if let Some(tx) = sender {
        let _ = tx.send(approved);
    }
    Ok(())
}

#[derive(Serialize)]
struct AppInfo {
    provider: String,
    model: String,
    embedding_model: String,
    memory_count: i64,
    memory_kinds: Vec<(String, i64)>,
}

#[tauri::command]
async fn app_info(state: State<'_, Shared>) -> Result<AppInfo, String> {
    let mem = state.memory.lock().unwrap();
    Ok(AppInfo {
        provider: state.provider.clone(),
        model: state.model.clone(),
        embedding_model: state.embedding_model.clone(),
        memory_count: mem.count(),
        memory_kinds: mem.kinds(),
    })
}

#[derive(Serialize)]
struct StoreResult {
    stored: usize,
    ids: Vec<i64>,
}

/// Add a memory. Long text is chunked automatically. kind: fact|preference|note|project|imported.
#[tauri::command]
async fn memory_add(state: State<'_, Shared>, text: String, kind: String) -> Result<StoreResult, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Empty memory text.".into());
    }
    let kind = match kind.as_str() {
        "fact" | "preference" | "note" | "project" | "imported" => kind,
        _ => "fact".to_string(),
    };
    if kind == "project" && text.contains("->") {
        // "Name -> /path" registers a known project.
        let mut parts = text.splitn(2, "->");
        let name = parts.next().unwrap_or("").trim();
        let path = parts.next().unwrap_or("").trim();
        if name.is_empty() || path.is_empty() {
            return Err("Project format: Name -> /path/to/project".into());
        }
        let vecs = state.embedder.embed(&[name.to_string()]).await.map_err(|e| truncate_err(e, 300))?;
        state.memory.lock().unwrap().remember_project(name, path, &vecs[0]).map_err(|e| truncate_err(e, 300))?;
        return Ok(StoreResult { stored: 1, ids: vec![] });
    }
    let n = import_text(&state.memory, &*state.embedder, &text, &kind, "manual")
        .await
        .map_err(|e| truncate_err(e, 300))?;
    Ok(StoreResult { stored: n, ids: vec![] })
}

/// Import a ChatGPT `conversations.json` export (pasted or picked from disk).
#[tauri::command]
async fn memory_import_chatgpt(state: State<'_, Shared>, json_text: String) -> Result<StoreResult, String> {
    if json_text.trim().is_empty() {
        return Err("Empty import.".into());
    }
    let n = import_chatgpt_json(&state.memory, &*state.embedder, &json_text)
        .await
        .map_err(|e| truncate_err(e, 300))?;
    Ok(StoreResult { stored: n, ids: vec![] })
}

#[tauri::command]
async fn memory_list(state: State<'_, Shared>, limit: u32) -> Result<Vec<MemoryItem>, String> {
    let limit = (limit as usize).clamp(1, 100);
    Ok(state.memory.lock().unwrap().list_recent(limit))
}

#[tauri::command]
async fn memory_search(state: State<'_, Shared>, query: String) -> Result<Vec<ScoredMemory>, String> {
    if query.trim().is_empty() {
        return Err("Empty query.".into());
    }
    let vecs = state.embedder.embed(&[query]).await.map_err(|e| truncate_err(e, 300))?;
    Ok(state.memory.lock().unwrap().recall(&vecs[0], 8, 0.2))
}

#[tauri::command]
async fn memory_delete(state: State<'_, Shared>, id: i64) -> Result<bool, String> {
    Ok(state.memory.lock().unwrap().delete(id))
}

#[tauri::command]
async fn history_list(state: State<'_, Shared>, limit: u32) -> Result<Vec<ChatSession>, String> {
    let limit = (limit as usize).clamp(1, 100);
    state
        .history
        .lock()
        .map(|h| h.list_sessions(limit))
        .map_err(|e| truncate_err(format!("History locked: {e}"), 200))
}

#[tauri::command]
async fn history_get(state: State<'_, Shared>, session_id: String) -> Result<Vec<ChatMessageRow>, String> {
    state
        .history
        .lock()
        .map(|h| h.get_messages(&session_id))
        .map_err(|e| truncate_err(format!("History locked: {e}"), 200))
}

#[tauri::command]
async fn history_delete(state: State<'_, Shared>, session_id: String) -> Result<bool, String> {
    state
        .history
        .lock()
        .map(|h| h.delete_session(&session_id))
        .map_err(|e| truncate_err(format!("History locked: {e}"), 200))
}

#[tauri::command]
async fn history_rename(
    state: State<'_, Shared>,
    session_id: String,
    title: String,
) -> Result<bool, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Empty title.".into());
    }
    state
        .history
        .lock()
        .map(|h| h.rename_session(&session_id, &title))
        .map_err(|e| truncate_err(format!("History locked: {e}"), 200))
}

fn main() {
    ensure_webkit_paths();
    tauri::Builder::default()
        .setup(|app| {
            let root = config::find_project_root();
            let cfg = config::load_config(&root);
            log(
                Level::Info,
                "BOOT",
                "Comrade starting",
                Some(&serde_json::json!({
                    "provider": cfg.llm_provider,
                    "model": cfg.llm_model,
                    "presence": config::presence(&cfg),
                })),
            );

            // Everything Comrade owns lives under comrade-agent/ (see core::paths).
            let home = paths::ensure_home().expect("failed to create comrade home directory");
            let db_path = home.join("comrade-memory.db");
            // One-time move of the pre-consolidation database, if present.
            if !db_path.exists() {
                let legacy_db = paths::legacy_db_path();
                if legacy_db.exists() {
                    match std::fs::copy(&legacy_db, &db_path) {
                        Ok(_) => log(Level::Info, "BOOT", "moved legacy database into comrade-agent", None),
                        Err(e) => log(
                            Level::Warn,
                            "BOOT",
                            "legacy database copy failed; starting fresh",
                            Some(&serde_json::json!({ "error": format!("{e}").chars().take(120).collect::<String>() })),
                        ),
                    }
                }
            }
            let memory: Arc<std::sync::Mutex<MemoryStore>> = Arc::new(std::sync::Mutex::new(
                MemoryStore::open(&db_path, cfg.embedding_dim)
                    .expect("failed to open memory database"),
            ));
            let history: Arc<std::sync::Mutex<HistoryStore>> = Arc::new(std::sync::Mutex::new(
                HistoryStore::open(&home.join("history.db")).expect("failed to open history database"),
            ));
            // One-time retirement of auto-created chat-summary memories: raw chat
            // belongs in history.db; vector memory now only keeps distilled facts.
            if let Ok(mem) = memory.lock() {
                if mem.meta_get("task_cleanup_v1").is_none() {
                    let n = mem.delete_by_kind("task");
                    mem.meta_set("task_cleanup_v1", "done");
                    if n > 0 {
                        log(
                            Level::Info,
                            "BOOT",
                            "retired auto chat-summary memories",
                            Some(&serde_json::json!({ "removed": n })),
                        );
                    }
                }
            }
            // Legacy JSON migration sources: new home first, old app-data dir second.
            let legacy_json = [
                home.join("comrade-memory.json"),
                paths::legacy_json_path(),
            ]
            .into_iter()
            .find(|p| p.exists());

            let llm = Arc::new(create_configured(&cfg));
            let embedder = Arc::new(OpenRouterEmbedder::new(
                embedding_key(&cfg),
                cfg.embedding_model.clone(),
            ));
            let stt = OpenRouterStt::new(cfg.openrouter_key.clone(), cfg.stt_model.clone());
            let tts = FallbackTts::new(
                OpenRouterTts::new(
                    cfg.openrouter_key.clone(),
                    cfg.tts_model.clone(),
                    cfg.tts_voice.clone(),
                ),
                Some(Box::new(|reason: String| {
                    log(
                        Level::Warn,
                        "TTS",
                        "primary TTS failed, using local fallback",
                        Some(&serde_json::json!({ "reason": reason })),
                    );
                })),
            );
            let tools = build_tools(&cfg.opencode_bin);
            let cwd = app
                .path()
                .home_dir()
                .unwrap_or_else(|_| PathBuf::from("/home/electron"));
            let agent = Agent::new(AgentDeps {
                llm,
                embedder: embedder.clone(),
                tools,
                memory: memory.clone(),
                cwd: cwd.clone(),
                model: Some(cfg.llm_model.clone()),
                max_steps: cfg.max_steps,
                timeout_ms: cfg.timeout_ms,
            });

            app.manage(Arc::new(AppState {
                agent,
                memory: memory.clone(),
                history: history.clone(),
                embedder: embedder.clone(),
                provider: cfg.llm_provider.clone(),
                model: cfg.llm_model.clone(),
                embedding_model: cfg.embedding_model.clone(),
                stt,
                tts,
                cancel: AtomicBool::new(false),
                task_lock: tokio::sync::Mutex::new(()),
                pending: tokio::sync::Mutex::new(HashMap::new()),
                perm_counter: AtomicU64::new(0),
            }));

            // One-time migration of the legacy JSON memory file (background).
            if let Some(json_path) = legacy_json {
                tauri::async_runtime::spawn(async move {
                    match migrate_legacy_json(&memory, &*embedder, &json_path).await {
                        Ok(0) => {}
                        Ok(n) => log(Level::Info, "BOOT", "migrated legacy memories", Some(&serde_json::json!({ "count": n }))),
                        Err(e) => {
                            let msg: String = format!("{e}").chars().take(200).collect();
                            log(Level::Warn, "BOOT", "legacy migration skipped", Some(&serde_json::json!({ "error": msg })));
                        }
                    }
                });
            }

            let bin = cfg.opencode_bin.clone();
            let webview_data = home.join("webview");
            if let Err(e) = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Comrade")
                .inner_size(980.0, 760.0)
                .data_directory(webview_data)
                .build()
            {
                eprintln!("Comrade: failed to create main window: {e}");
            }
            tauri::async_runtime::spawn(async move {
                let (available, version) = opencode::is_opencode_available(&bin).await;
                log(
                    Level::Info,
                    "BOOT",
                    "OpenCode availability",
                    Some(&serde_json::json!({ "available": available, "version": version })),
                );
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            send_message,
            voice_input,
            cancel_task,
            permission_response,
            app_info,
            memory_add,
            memory_import_chatgpt,
            memory_list,
            memory_search,
            memory_delete,
            history_list,
            history_get,
            history_delete,
            history_rename
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Comrade");
}
