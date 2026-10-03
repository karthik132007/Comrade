/** Comrade desktop shell (Tauri v2): window + commands + agent wiring. */
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use comrade_core::agent::{Agent, AgentCallbacks, AgentDeps};
use comrade_core::config;
use comrade_core::llm::factory::{create_backends, embedding_dim};
use comrade_core::llm::Embedder;
use comrade_core::logger::{log, Level};
use comrade_core::service::{AnyEmbedder, AnyLlm, ServiceClient};
use comrade_core::history::{title_for, ChatMessageRow, ChatSession, HistoryStore};
use comrade_core::memory::{import_chatgpt_json, import_text, migrate_legacy_json, MemoryItem, MemoryStore, ScoredMemory};
use comrade_core::paths;
use comrade_core::prefs::{self, Prefs};
use comrade_core::task_state::{TaskState, TaskStatus};
use comrade_core::tools::{build_tools, coding};
use comrade_core::tools::types::Tool;
use comrade_core::voice::capture::{list_input_devices, list_output_devices, AudioDeviceInfo};
use comrade_core::voice::manager::{
    ensure_server_engines, ensure_shared_engines, AgentOutcome, AgentRunner, ServerVoice,
    SharedEngines, VoiceController, VoiceEvent, VoiceHooks, VoiceState,
};
use comrade_core::voice::models::models_dir;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

mod browser_stream;

type Shared = Arc<AppState>;

struct AppState {
    tools: Vec<Arc<dyn Tool>>,
    cwd: PathBuf,
    memory: Arc<std::sync::Mutex<MemoryStore>>,
    history: Arc<std::sync::Mutex<HistoryStore>>,
    embedder: Arc<AnyEmbedder>,
    provider: String,
    model: String,
    embedding_model: String,
    server: Option<ServiceClient>,
    voice_engines: std::sync::Mutex<Option<SharedEngines>>,
    voice_session: std::sync::Mutex<Option<VoiceController>>,
    model_download: AtomicBool,
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

fn emit_voice(app: &AppHandle, event: VoiceEvent) {
    match event {
        VoiceEvent::AgentToken { token } => {
            emit(app, serde_json::json!({ "type": "token", "token": token }));
        }
        VoiceEvent::StateChanged { state } => {
            // The chat pill already knows thinking/executing/speaking.
            let ui = match state {
                VoiceState::Listening => "listening",
                VoiceState::Processing => "thinking",
                VoiceState::Speaking => "speaking",
                VoiceState::Interrupted => "interrupted",
                VoiceState::Idle => "idle",
            };
            emit(app, serde_json::json!({ "type": "state", "state": ui }));
        }
        VoiceEvent::Started => {
            emit(app, serde_json::json!({ "type": "state", "state": "listening" }));
        }
        VoiceEvent::PartialTranscript { text } => {
            emit(app, serde_json::json!({ "type": "voice-partial", "text": text }));
        }
        VoiceEvent::AudioLevel { level } => {
            emit(app, serde_json::json!({ "type": "voice-level", "level": level }));
        }
        VoiceEvent::FinalTranscript { text } => {
            emit(app, serde_json::json!({ "type": "transcript", "transcript": text }));
        }
        VoiceEvent::Stopped { .. } => {
            emit(app, serde_json::json!({ "type": "state", "state": "idle" }));
        }
        VoiceEvent::Error { message } => {
            emit(app, serde_json::json!({ "type": "done", "status": "failed", "error": message }));
        }
        VoiceEvent::ModelsStatus { ready, missing } => {
            emit(app, serde_json::json!({ "type": "voice-models", "ready": ready, "missing": missing }));
        }
        VoiceEvent::ModelsDownloading { pack, file, downloaded, total } => {
            emit(app, serde_json::json!({
                "type": "voice-download", "pack": pack, "file": file,
                "downloaded": downloaded, "total": total,
            }));
        }
        VoiceEvent::ModelsReady => {
            emit(app, serde_json::json!({ "type": "voice-models", "ready": true, "missing": [] }));
        }
        // TTS lifecycle is covered by Speaking state + metrics logs.
        VoiceEvent::TtsStarted
        | VoiceEvent::TtsChunk { .. }
        | VoiceEvent::TtsFinished
        | VoiceEvent::TtsInterrupted => {}
    }
}

async fn request_approval(app: &AppHandle, shared: &Shared, summary: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let n = shared.perm_counter.fetch_add(1, Ordering::SeqCst);
    let id = format!("perm-{}-{n}", now_ms());
    shared.pending.lock().await.insert(id.clone(), tx);
    let _ = app.emit(
        "permission-request",
        serde_json::json!({ "id": id, "summary": summary }),
    );
    match tokio::time::timeout(Duration::from_secs(30), rx).await {
        Ok(Ok(approved)) => approved,
        _ => {
            // Timeout or sender dropped: default deny.
            shared.pending.lock().await.remove(&id);
            false
        }
    }
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
        request_approval(&self.app, &self.shared, summary).await
    }
}

/// Agent callbacks for voice turns: the manager owns UI state, steps stream
/// to the task panel, and tokens feed the TTS chunker (the session also
/// forwards them to the chat UI as AgentToken events).
struct VoiceAgentCallbacks {
    app: AppHandle,
    shared: Shared,
    chunk_feed: Arc<dyn Fn(String) + Send + Sync>,
    cancel: Arc<AtomicBool>,
}

impl AgentCallbacks for VoiceAgentCallbacks {
    fn on_ui_state(&self, _state: &str) {}

    fn on_step(&self, label: &str, status: &str, detail: Option<&str>) {
        emit(
            &self.app,
            serde_json::json!({ "type": "step", "label": label, "status": status, "detail": detail }),
        );
    }

    fn on_token(&self, token: &str) {
        (self.chunk_feed)(token.to_string());
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || self.shared.cancel.load(Ordering::SeqCst)
    }

    async fn request_approval(&self, summary: &str) -> bool {
        request_approval(&self.app, &self.shared, summary).await
    }
}

/// Bridges voice sessions to the Comrade agent (history + task state shared
/// with text chat).
struct VoiceAgentRunner {
    app: AppHandle,
    shared: Shared,
    session: Arc<std::sync::Mutex<Option<String>>>,
}

impl AgentRunner for VoiceAgentRunner {
    fn run<'a>(
        &'a self,
        transcript: String,
        hooks: &'a VoiceHooks,
    ) -> Pin<Box<dyn Future<Output = AgentOutcome> + Send + 'a>> {
        Box::pin(async move {
            let sid = self.session.lock().unwrap().clone();
            let cb = VoiceAgentCallbacks {
                app: self.app.clone(),
                shared: self.shared.clone(),
                chunk_feed: hooks.on_token.clone(),
                cancel: hooks.cancel.clone(),
            };
            let (task, new_sid) =
                execute_task(&self.app, &self.shared, &transcript, sid, None, &cb).await;
            *self.session.lock().unwrap() = Some(new_sid);
            let ok = task.status == TaskStatus::Done;
            let text = task
                .result
                .clone()
                .or_else(|| task.error.clone())
                .unwrap_or_default();
            AgentOutcome { text, ok }
        })
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
#[cfg(target_os = "linux")]
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

#[cfg(not(target_os = "linux"))]
fn ensure_webkit_paths() {}

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

/// Build the agent from live config on every task, so switching between
/// direct APIs and the service server (Settings → Service, or .env) takes
/// effect immediately — no restart needed. Tools/memory/cwd are shared;
/// only the LLM + embedder backends rebind.
fn build_live_agent(shared: &Shared, model_override: Option<String>) -> Agent<AnyLlm, AnyEmbedder> {
    let root = config::find_project_root();
    let live = config::load_config(&root);
    let (llm_backend, embed_backend) = create_backends(&live);
    // Per-message model pick (composer dropdown): exact allowlist id, and
    // only in service mode — direct mode keeps its configured model.
    let model = if live.use_service() {
        match model_override.map(|m| comrade_core::service::normalize_llm_model(&m)) {
            Some(m) => m,
            None => live.server_llm_model.clone(),
        }
    } else {
        live.llm_model.clone()
    };
    log(
        Level::Agent,
        "backend",
        if live.use_service() { "service" } else { "direct" },
        Some(&serde_json::json!({ "model": model })),
    );
    Agent::new(AgentDeps {
        llm: Arc::new(llm_backend),
        embedder: Arc::new(embed_backend),
        tools: shared.tools.clone(),
        memory: shared.memory.clone(),
        cwd: shared.cwd.clone(),
        model: Some(model),
        max_steps: live.max_steps,
        timeout_ms: live.timeout_ms,
    })
}

async fn execute_task<C: AgentCallbacks>(
    app: &AppHandle,
    shared: &Shared,
    text: &str,
    session_id: Option<String>,
    model: Option<String>,
    cb: &C,
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
    let task = build_live_agent(shared, model).run_task(text, cb).await;
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
    model: Option<String>,
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
    let cb = Callbacks { app: app.clone(), shared: shared.clone() };
    let (task, sid) = execute_task(&app, &shared, &text, session_id, model, &cb).await;
    Ok(TaskSummary {
        status: task.status.as_str().to_string(),
        result: task.result,
        error: task.error,
        session_id: sid,
    })
}

/// Resolve server-voice config for this session: voice backend set to
/// `server` in Settings plus a configured service server. Returns None for
/// fully local voice (on-device models required).
fn cfg_server_voice(shared: &Shared, prefs: &Prefs) -> Option<ServerVoice> {
    if prefs.voice.backend.trim().to_lowercase() != "server" {
        return None;
    }
    // Prefer the startup client; fall back to live prefs (+env key) so
    // enabling the server in Settings works without a restart. An empty
    // URL means the built-in backend (never shown in UI or conf).
    let client = shared.server.clone().or_else(|| {
        let base_url = comrade_core::service::resolve_server_url(&prefs.server.base_url);
        let api_key = std::env::var("COMRADE_SERVER_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| prefs.server.api_key.clone());
        Some(ServiceClient::new(comrade_core::service::ServiceConfig {
            base_url,
            api_key,
            llm_model: prefs.server.llm_model.clone(),
            embedding_model: prefs.server.embedding_model.clone(),
            embedding_dim: prefs.server.embedding_dim,
            stt_model: prefs.server.stt_model.clone(),
            tts_voice: prefs.server.tts_voice.clone(),
            timeout_secs: 120,
        }))
    })?;
    let tts_voice = prefs.server.tts_voice.clone();
    Some(ServerVoice {
        language: prefs.voice.stt.language.clone(),
        tts_voice: if tts_voice.trim().is_empty() { "default".into() } else { tts_voice },
        tts_speed: prefs.voice.tts.speed,
        client,
    })
}

/// Begin a local voice session. `one_shot` ends after the first reply
/// (push-to-talk); otherwise the conversation loops until stopped.
#[tauri::command]
async fn start_voice_input(
    app: AppHandle,
    state: State<'_, Shared>,
    one_shot: bool,
    session_id: Option<String>,
) -> Result<String, String> {
    use comrade_core::voice::manager::{EngineBuildConfig, VoiceController, VoiceRuntimeConfig};
    use comrade_core::voice::models as vm;
    let shared = state.inner().clone();
    let live = shared.voice_session.lock().unwrap().clone();
    if let Some(c) = live {
        if c.state() != VoiceState::Idle {
            return Err("Voice session already active.".into());
        }
    }
    let prefs = prefs::load();
    if !prefs.voice.enabled {
        return Err("Voice is disabled in Settings.".into());
    }
    // Server voice: STT/TTS via the service server, energy VAD, no downloads.
    // Local voice: on-device sherpa-onnx models (downloaded once).
    let wants_server_voice = prefs.voice.backend.trim().to_lowercase() == "server";
    let server_voice = cfg_server_voice(&shared, &prefs);
    if wants_server_voice && server_voice.is_none() {
        return Err(
            "Server voice selected but no service server is configured. Set it in Settings → Service."
                .into(),
        );
    }
    let engines = if server_voice.is_some() {
        ensure_server_engines(&shared.voice_engines, prefs.voice.vad.threshold)
            .await
            .map_err(|e| truncate_err(e, 300))?
    } else {
        let base = models_dir();
        if !vm::all_ready(&base) {
            let missing: Vec<String> = vm::required_packs()
                .iter()
                .flat_map(|p| {
                    vm::missing_files(&base, p).into_iter().map(|f| format!("{}/{}", p.id, f))
                })
                .collect();
            emit(&app, serde_json::json!({ "type": "voice-models", "ready": false, "missing": missing }));
            return Err(format!(
                "Voice models incomplete: {}. Open Settings → Download to resume, or switch Voice backend to Server.",
                missing.join(", ")
            ));
        }
        ensure_shared_engines(&shared.voice_engines, EngineBuildConfig {
            models_base: base,
            vad_threshold: prefs.voice.vad.threshold,
            vad_silence_ms: prefs.voice.vad.silence_ms,
            vad_min_speech_ms: prefs.voice.vad.min_speech_ms,
            tts_voice: prefs.voice.tts.voice.clone(),
            tts_speed: prefs.voice.tts.speed,
            num_threads: prefs.voice.runtime.num_threads,
        })
        .await
        .map_err(|e| truncate_err(e, 300))?
    };
    let runner = Arc::new(VoiceAgentRunner {
        app: app.clone(),
        shared: shared.clone(),
        session: Arc::new(std::sync::Mutex::new(session_id)),
    });
    let controller = VoiceController::new(move |e| emit_voice(&app, e), runner);
    controller.set_microphone(&prefs.voice.mic);
    controller
        .start_session(
            engines,
            VoiceRuntimeConfig {
                silence_ms: prefs.voice.vad.silence_ms,
                min_speech_ms: prefs.voice.vad.min_speech_ms,
                max_utterance_ms: prefs.voice.runtime.max_utterance_ms,
                vad_threshold: prefs.voice.vad.threshold,
                decode_every_frames: prefs.voice.runtime.decode_every_frames,
                chunk_max_chars: prefs.voice.runtime.chunk_max_chars,
                chunk_min_merge: prefs.voice.runtime.chunk_min_merge,
                tts_voice: prefs.voice.tts.voice.clone(),
                tts_speed: prefs.voice.tts.speed,
                num_threads: prefs.voice.runtime.num_threads,
                speak: prefs.voice.autoplay,
                server: server_voice,
            },
            one_shot,
        )
        .await
        .map_err(|e| truncate_err(e, 300))?;
    *shared.voice_session.lock().unwrap() = Some(controller);
    Ok("started".into())
}


/// Push-to-talk release (or conversation stop): finalize the current
/// utterance when `finalize` is set, else end the session immediately.
#[tauri::command]
async fn stop_voice_input(state: State<'_, Shared>, finalize: bool) -> Result<String, String> {
    let shared = state.inner().clone();
    let controller = shared.voice_session.lock().unwrap().clone();
    match controller {
        Some(c) => {
            if finalize {
                c.finish_turn().await;
            } else {
                c.stop_session("stopped").await;
            }
            Ok("ok".into())
        }
        None => Err("No voice session active.".into()),
    }
}

#[tauri::command]
async fn cancel_voice_input(state: State<'_, Shared>) -> Result<String, String> {
    let shared = state.inner().clone();
    let controller = shared.voice_session.lock().unwrap().clone();
    if let Some(c) = controller {
        c.cancel_session().await;
    }
    shared.cancel.store(true, Ordering::SeqCst);
    Ok("ok".into())
}

#[tauri::command]
async fn set_microphone_device(device: String) -> Result<String, String> {    let mut prefs = prefs::load();
    prefs.voice.mic = device.trim().to_string();
    prefs::save(&prefs).map_err(|e| truncate_err(e, 300))?;
    Ok("ok".into())
}

#[derive(Serialize)]
struct AudioDevicesDto {
    inputs: Vec<AudioDeviceInfo>,
    outputs: Vec<AudioDeviceInfo>,
}

#[derive(Serialize)]
struct PackStatusDto {
    id: String,
    ready: bool,
    missing: Vec<String>,
}

#[derive(Serialize)]
struct VoiceModelsStatus {
    ready: bool,
    missing: Vec<String>,
    packs: Vec<PackStatusDto>,
}

#[tauri::command]
async fn get_audio_devices() -> Result<AudioDevicesDto, String> {
    Ok(AudioDevicesDto {
        inputs: list_input_devices(),
        outputs: list_output_devices(),
    })
}

#[tauri::command]
async fn voice_models_status() -> Result<VoiceModelsStatus, String> {
    use comrade_core::voice::models as vm;
    let base = models_dir();
    let mut packs = Vec::new();
    let mut all_missing = Vec::new();
    for pack in vm::required_packs() {
        let missing = vm::missing_files(&base, &pack);
        if !missing.is_empty() {
            all_missing.extend(missing.iter().map(|f| format!("{}/{}", pack.id, f)));
        }
        packs.push(PackStatusDto {
            id: pack.id.to_string(),
            ready: missing.is_empty(),
            missing,
        });
    }
    Ok(VoiceModelsStatus { ready: all_missing.is_empty(), packs, missing: all_missing })
}

#[tauri::command]
async fn voice_download_models(app: AppHandle, state: State<'_, Shared>) -> Result<String, String> {
    use comrade_core::voice::models as vm;
    let shared = state.inner().clone();
    if shared.model_download.swap(true, Ordering::SeqCst) {
        return Err("Model download already running.".into());
    }
    let base = models_dir();
    tauri::async_runtime::spawn(async move {
        let out = vm::ensure_models(&base, &|p| {
            app.emit(
                "voice-event",
                serde_json::json!({
                    "type": "voice-download",
                    "pack": p.pack, "file": p.file,
                    "downloaded": p.downloaded, "total": p.total,
                }),
            )
            .ok();
        })
        .await;
        shared.model_download.store(false, Ordering::SeqCst);
        match out {
            Ok(_) => {
                app.emit("voice-event", serde_json::json!({ "type": "voice-models", "ready": true, "missing": [] })).ok();
                log(Level::Info, "VOICE", "models ready", None);
            }
            Err(e) => {
                let msg: String = format!("{e}").chars().take(300).collect();
                app.emit("voice-event", serde_json::json!({ "type": "voice-download-error", "message": msg })).ok();
                log(Level::Warn, "VOICE", "model download failed", Some(&serde_json::json!({ "error": msg })));
            }
        }
    });
    Ok("downloading".into())
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
    backend: String,
    voice_backend: String,
    server_url: String,
    onboarded: bool,
    memory_count: i64,
    memory_kinds: Vec<(String, i64)>,
}

#[tauri::command]
async fn app_info(state: State<'_, Shared>) -> Result<AppInfo, String> {
    // Live config so the Brain line reflects Settings immediately.
    // The built-in backend URL is never exposed to the UI: server_url is
    // only populated when a custom URL was explicitly configured.
    let live = config::load_config(&config::find_project_root());
    let custom_url = live.server_url.trim().trim_end_matches('/')
        != comrade_core::service::COMRADE_DEFAULT_SERVER_URL.trim_end_matches('/');
    let (provider, model, embedding_model, backend, server_url) = if live.use_service() {
        (
            "service".to_string(),
            live.server_llm_model.clone(),
            live.server_embedding_model.clone(),
            "service".to_string(),
            if custom_url { live.server_url.clone() } else { String::new() },
        )
    } else {
        (
            state.provider.clone(),
            state.model.clone(),
            state.embedding_model.clone(),
            "direct".to_string(),
            String::new(),
        )
    };
    let voice_backend =
        if live.voice_backend == "server" && !live.server_url.trim().is_empty() {
            "server".to_string()
        } else {
            "local".to_string()
        };
    let mem = state.memory.lock().unwrap();
    Ok(AppInfo {
        provider,
        model,
        embedding_model,
        backend,
        voice_backend,
        server_url,
        onboarded: prefs::load().onboarded(),
        memory_count: mem.count(),
        memory_kinds: mem.kinds(),
    })
}

/// Probe the service server (`GET /v1/models`, else `/health`).
/// Uses live Settings (+env key) so the Test button works before restart.
#[tauri::command]
async fn server_status() -> Result<String, String> {
    let prefs = prefs::load();
    let base_url = comrade_core::service::resolve_server_url(&prefs.server.base_url);
    let api_key = std::env::var("COMRADE_SERVER_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| prefs.server.api_key.clone());
    let client = ServiceClient::new(comrade_core::service::ServiceConfig {
        base_url,
        api_key,
        llm_model: prefs.server.llm_model.clone(),
        embedding_model: prefs.server.embedding_model.clone(),
        embedding_dim: prefs.server.embedding_dim,
        stt_model: prefs.server.stt_model.clone(),
        tts_voice: prefs.server.tts_voice.clone(),
        timeout_secs: 120,
    });
    client.health().await.map_err(|e| truncate_err(e, 300))
}

/// Fetch the server routing table (`GET /v1/models`, no secrets) using
/// live Settings (+env key) so it works before restart.
#[tauri::command]
async fn server_models() -> Result<serde_json::Value, String> {
    let prefs = prefs::load();
    let base_url = comrade_core::service::resolve_server_url(&prefs.server.base_url);
    let api_key = std::env::var("COMRADE_SERVER_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| prefs.server.api_key.clone());
    let client = ServiceClient::new(comrade_core::service::ServiceConfig {
        base_url,
        api_key,
        llm_model: prefs.server.llm_model.clone(),
        embedding_model: prefs.server.embedding_model.clone(),
        embedding_dim: prefs.server.embedding_dim,
        stt_model: prefs.server.stt_model.clone(),
        tts_voice: prefs.server.tts_voice.clone(),
        timeout_secs: 120,
    });
    client.models().await.map_err(|e| truncate_err(e, 300))
}

/// Coding agents installed on this machine (for onboarding + Settings).
#[tauri::command]
async fn system_coding_agents() -> Result<Vec<comrade_core::prefs::CodingAgentInfo>, String> {
    Ok(comrade_core::prefs::detect_coding_agents())
}

/// Navigate Comrade's own in-app browser (bundled Chromium) to a URL.
/// Used by the in-app browser pane's address bar — same tab the agent drives.
#[tauri::command]
async fn browser_open(url: String) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("EMPTY_URL: no URL provided.".into());
    }
    let port = browser_driver::interactive_port().await.map_err(|m| trim_msg(m, 400))?;
    let (requested, final_url) =
        browser_driver::page_navigate_interactive(port, &url).await.map_err(|m| trim_msg(m, 400))?;
    Ok(serde_json::json!({ "requested": requested, "url": final_url }))
}

#[tauri::command]
async fn browser_tabs() -> Result<Vec<serde_json::Value>, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::ensure_chromium().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::tabs(port).await.map_err(|m| trim_msg(m, 400))
}

#[tauri::command]
async fn browser_tab_new(url: Option<String>) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::ensure_chromium().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::create_tab(port, url.as_deref().unwrap_or(browser_driver::DEFAULT_HOME_URL)).await.map_err(|m| trim_msg(m, 400))
}

#[tauri::command]
async fn browser_tab_select(id: String) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::ensure_chromium().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::select_tab(port, &id).await.map_err(|m| trim_msg(m, 400))
}

#[tauri::command]
async fn browser_tab_close(id: String) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::ensure_chromium().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::close_tab(port, &id).await.map_err(|m| trim_msg(m, 400))
}

#[derive(Serialize)]
struct BrowserPaneState {
    running: bool,
    url: String,
    title: String,
    tabs: Vec<serde_json::Value>,
}

/// Snapshot for the in-app browser pane (never fails — placeholders when down).
#[tauri::command]
async fn browser_state() -> Result<BrowserPaneState, String> {
    use comrade_core::tools::browser_driver;
    Ok(state_from_snapshot(browser_driver::state_snapshot().await))
}

fn state_from_snapshot(snap: serde_json::Value) -> BrowserPaneState {
    BrowserPaneState {
        running: snap.get("running").and_then(|v| v.as_bool()).unwrap_or(false),
        url: snap.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        title: snap.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        tabs: snap.get("tabs").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
    }
}

/// Make sure the built-in browser is installed and running (installs itself
/// on first use — one-time download with progress via
/// `browser_provision_status`). Used when the in-app pane opens.
#[tauri::command]
async fn browser_ensure() -> Result<BrowserPaneState, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::ensure_chromium().await.map_err(|m| trim_msg(m, 400))?;
    let snapshot = browser_driver::pane_snapshot(port).await.map_err(|m| trim_msg(m, 400))?;
    Ok(state_from_snapshot(snapshot))
}

/// Self-install progress for the built-in browser
/// ({installed, phase, downloaded, total, error}).
#[tauri::command]
async fn browser_provision_status() -> Result<serde_json::Value, String> {
    use comrade_core::tools::provision;
    Ok(provision::status_json())
}

/// Live screenshot of the in-app browser tab as a data URL (PNG).
#[tauri::command]
async fn browser_screenshot() -> Result<String, String> {
    use base64::Engine as _;
    use comrade_core::tools::browser_driver;
    let bytes = browser_driver::screenshot_bytes().await.map_err(|m| trim_msg(m, 400))?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

#[derive(Serialize)]
struct BrowserFrame {
    data_url: String,
    width: f64,
    height: f64,
}

/// One interactive frame for the in-app pane: screenshot plus the page's CSS
/// viewport size, so UI clicks map 1:1 onto page coordinates.
#[tauri::command]
async fn browser_frame() -> Result<BrowserFrame, String> {
    use base64::Engine as _;
    use comrade_core::tools::browser_driver;
    let frame = browser_driver::capture_frame().await.map_err(|m| trim_msg(m, 400))?;
    Ok(BrowserFrame {
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&frame.png)
        ),
        width: frame.viewport_w,
        height: frame.viewport_h,
    })
}

/// Click at page coordinates in the in-app browser (pane interaction).
#[tauri::command]
async fn browser_click_at(x: f64, y: f64) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::ensure_chromium().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::mouse_click(port, x, y).await.map_err(|m| trim_msg(m, 400))?;
    let url = browser_driver::current_url(port).await.unwrap_or_default();
    Ok(serde_json::json!({ "url": url }))
}

/// Forward trusted hover, drag, click and nested-scroll input to Chromium.
#[tauri::command]
async fn browser_pointer(event: serde_json::Value) -> Result<(), String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::interactive_port().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::pointer_event(port, event).await
}

/// Type into the focused element of the in-app browser (pane interaction).
#[tauri::command]
async fn browser_type_text(text: String) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::interactive_port().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::insert_text(port, &text).await.map_err(|m| trim_msg(m, 400))?;
    Ok(serde_json::json!({ "typed": text.len() }))
}

/// Press a key in the in-app browser (pane interaction).
#[tauri::command]
async fn browser_press_key(key: String, modifiers: Option<i64>) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::interactive_port().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::page_press_with_modifiers(port, key.trim(), modifiers.unwrap_or(0)).await.map_err(|m| trim_msg(m, 400))?;
    Ok(serde_json::json!({ "key": key }))
}

/// Scroll the in-app browser by pixels (pane wheel interaction).
#[tauri::command]
async fn browser_scroll(x: f64, y: f64) -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::ensure_chromium().await.map_err(|m| trim_msg(m, 400))?;
    let (sx, sy) =
        browser_driver::page_scroll(port, x.round() as i64, y.round() as i64).await.map_err(|m| trim_msg(m, 400))?;
    Ok(serde_json::json!({ "scrollX": sx, "scrollY": sy }))
}

/// Stop the bundled Chromium (in-app browser pane goes idle).
#[tauri::command]
async fn browser_close() -> Result<bool, String> {
    use comrade_core::tools::browser_driver;
    Ok(matches!(
        browser_driver::close_chromium().await,
        browser_driver::CloseOutcome::Closed
    ))
}

/// Step back in the in-app browser history (pane toolbar).
#[tauri::command]
async fn browser_back() -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::interactive_port().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::history_interactive(port, false).await.map_err(|m| trim_msg(m, 400))?;
    Ok(browser_driver::state_snapshot().await)
}

/// Step forward in the in-app browser history (pane toolbar).
#[tauri::command]
async fn browser_forward() -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::interactive_port().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::history_interactive(port, true).await.map_err(|m| trim_msg(m, 400))?;
    Ok(browser_driver::state_snapshot().await)
}

/// Reload the in-app browser tab (pane toolbar).
#[tauri::command]
async fn browser_reload() -> Result<serde_json::Value, String> {
    use comrade_core::tools::browser_driver;
    let port = browser_driver::interactive_port().await.map_err(|m| trim_msg(m, 400))?;
    browser_driver::reload_interactive(port).await.map_err(|m| trim_msg(m, 400))?;
    Ok(browser_driver::state_snapshot().await)
}

fn trim_msg(msg: String, n: usize) -> String {
    msg.chars().take(n).collect()
}

#[tauri::command]
async fn get_prefs() -> Result<Prefs, String> {
    Ok(prefs::load())
}

/// Save preferences to comrade.conf (validated). Returns what was stored.
#[tauri::command]
async fn save_prefs(prefs: Prefs) -> Result<Prefs, String> {
    prefs::save(&prefs).map_err(|e| truncate_err(e, 300))?;
    let saved = prefs::load();
    comrade_core::tools::browser_driver::set_adblock_enabled(saved.browser.adblock_enabled).await?;
    Ok(saved)
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
                    "service_mode": cfg.use_service(),
                    "server_voice": cfg.use_server_voice(),
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
            // One-time cleanup of the pre-consolidation app-data dir: it only ever
            // held Comrade's own webview caches. DB/JSON backups are left alone.
            let sentinel = home.join(".legacy-cleaned");
            if !sentinel.exists() {
                let legacy_dir = paths::legacy_db_path()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                if !legacy_dir.as_os_str().is_empty() && legacy_dir != home && legacy_dir.exists() {
                    let mut removed = 0usize;
                    if let Ok(entries) = std::fs::read_dir(&legacy_dir) {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name == "comrade-memory.db" || name == "comrade-memory.json" {
                                continue; // cold backups stay.
                            }
                            let path = entry.path();
                            let ok = if path.is_dir() {
                                std::fs::remove_dir_all(&path).is_ok()
                            } else {
                                std::fs::remove_file(&path).is_ok()
                            };
                            if ok {
                                removed += 1;
                            }
                        }
                    }
                    log(Level::Info, "BOOT", "cleaned pre-consolidation residue", Some(&serde_json::json!({ "removed": removed })));
                }
                let _ = std::fs::write(&sentinel, "1");
            }
            let memory: Arc<std::sync::Mutex<MemoryStore>> = Arc::new(std::sync::Mutex::new(
                MemoryStore::open(&db_path, embedding_dim(&cfg))
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

            let (_, embed_backend) = create_backends(&cfg);
            let embedder = Arc::new(embed_backend);
            // Brain/model labels + server handle for the Settings UI.
            let (provider_label, model_label, embedding_label, server) =
                if cfg.use_service() {
                    (
                        "service".to_string(),
                        cfg.server_llm_model.clone(),
                        cfg.server_embedding_model.clone(),
                        Some(ServiceClient::new(cfg.service_config())),
                    )
                } else {
                    (
                        cfg.llm_provider.clone(),
                        cfg.llm_model.clone(),
                        cfg.embedding_model.clone(),
                        None,
                    )
                };
            let tools = build_tools();
            let cwd = app
                .path()
                .home_dir()
                .unwrap_or_else(|_| PathBuf::from("/home/electron"));
            // Note: the chat brain rebinds from live config on every task
            // (build_live_agent), so Service/direct switches apply instantly.

            app.manage(Arc::new(AppState {
                tools,
                cwd,
                memory: memory.clone(),
                history: history.clone(),
                embedder: embedder.clone(),
                provider: provider_label,
                model: model_label,
                embedding_model: embedding_label,
                server,
                voice_engines: std::sync::Mutex::new(None),
                voice_session: std::sync::Mutex::new(None),
                model_download: AtomicBool::new(false),
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

            let coding_prefs = prefs::load().coding;
            let webview_data = home.join("webview");
            if let Err(e) = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Comrade")
                .inner_size(1180.0, 800.0)
                .min_inner_size(640.0, 560.0)
                .data_directory(webview_data)
                .build()
            {
                eprintln!("Comrade: failed to create main window: {e}");
            }
            tauri::async_runtime::spawn(async move {
                let detected = comrade_core::prefs::detect_coding_agents();
                let ids: Vec<&str> = detected.iter().map(|d| d.id.as_str()).collect();
                let default = coding::resolve_agent(&coding_prefs.agents, &coding_prefs.default, &detected, None)
                    .map(|a| format!("{} ({})", a.name, a.bin))
                    .unwrap_or_else(|e| e);
                log(
                    Level::Info,
                    "BOOT",
                    "coding agents",
                    Some(&serde_json::json!({ "detected": ids, "default": default })),
                );
            });
            // The browser is a core feature: start its one-time self-install
            // in the background so it is ready before first use.
            tauri::async_runtime::spawn(async move {
                match comrade_core::tools::provision::ensure_provisioned().await {
                    Ok(exe) => log(
                        Level::Info,
                        "BOOT",
                        "built-in browser ready",
                        Some(&serde_json::json!({ "exe": exe.to_string_lossy() })),
                    ),
                    Err(e) => {
                        let msg: String = e.chars().take(200).collect();
                        log(Level::Warn, "BOOT", "built-in browser install deferred; will retry on first use",
                            Some(&serde_json::json!({ "error": msg })));
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            send_message,
            start_voice_input,
            stop_voice_input,
            cancel_voice_input,
            set_microphone_device,
            get_audio_devices,
            voice_models_status,
            voice_download_models,
            cancel_task,
            permission_response,
            app_info,
            server_status,
            server_models,
            memory_add,
            memory_import_chatgpt,
            memory_list,
            memory_search,
            memory_delete,
            history_list,
            history_get,
            history_delete,
            history_rename,
            system_coding_agents,
            browser_open,
            browser_tabs,
            browser_tab_new,
            browser_tab_select,
            browser_tab_close,
            browser_state,
            browser_ensure,
            browser_provision_status,
            browser_screenshot,
            browser_frame,
            browser_stream::browser_stream_start,
            browser_stream::browser_stream_stop,
            browser_stream::browser_stream_ack,
            browser_stream::browser_stream_resize,
            browser_pointer,
            browser_click_at,
            browser_type_text,
            browser_press_key,
            browser_scroll,
            browser_close,
            browser_back,
            browser_forward,
            browser_reload,
            get_prefs,
            save_prefs
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Comrade");
}
