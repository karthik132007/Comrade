/**
 * Voice session manager: explicit state machine, background tasks, channels.
 *
 * ```text
 * Tauri commands
 *   │  start/stop/cancel, mic select, model status
 *   ▼
 * VoiceController ──► session task ──► mic frames ──► VAD ──► streaming STT
 *       │                                 │    partials │         │ endpoint
 *       │                                 ▼             ▼         ▼ final
 *       │                              barge-in      agent (tokens → chunker → Kokoro → playback)
 *       ▼
 *   voice-* events to the frontend
 * ```
 *
 * Engines are loaded once and shared across sessions (never per-request).
 * Blocking inference runs in spawn_blocking; the Tauri thread never blocks.
 */
use serde::Serialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Instant;

use cpal::traits::HostTrait;

use crate::logger::{log, Level};
use crate::voice::chunk::SentenceBuffer;
use crate::voice::stt::{StreamRecognizer, SherpaOnlineZipformer};
use crate::voice::tts::{SherpaKokoro, SpeechSynth};
use crate::voice::vad::{TrackerEvent, UtteranceTracker, VadEngine, SherpaSileroVad};

/// Central voice state. No scattered booleans; UI derives from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceState {
    Idle,
    Listening,
    Processing,
    Speaking,
    Interrupted,
}

impl VoiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            VoiceState::Idle => "idle",
            VoiceState::Listening => "listening",
            VoiceState::Processing => "processing",
            VoiceState::Speaking => "speaking",
            VoiceState::Interrupted => "interrupted",
        }
    }
}

/// Frontend events. One `voice-event` channel, tagged payloads.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceEvent {
    StateChanged { state: VoiceState },
    Started,
    PartialTranscript { text: String },
    FinalTranscript { text: String },
    /// Mic level 0..1 for the listening waveform (10 Hz, tiny numbers only).
    AudioLevel { level: f32 },
    /// A reply token for the chat UI (mirrored as an agent-event upstream).
    AgentToken { token: String },
    Stopped { reason: String },
    Error { message: String },
    TtsStarted,
    TtsChunk { index: usize, chars: usize, latency_ms: u64 },
    TtsFinished,
    TtsInterrupted,
    ModelsStatus { ready: bool, missing: Vec<String> },
    ModelsDownloading { pack: String, file: String, downloaded: u64, total: Option<u64> },
    ModelsReady,
}

/// Outcome of one agent turn inside a voice session.
pub struct AgentOutcome {
    pub text: String,
    pub ok: bool,
}

/// Hooks the session hands to the agent runner.
pub struct VoiceHooks {
    pub on_token: Arc<dyn Fn(String) + Send + Sync>,
    pub cancel: Arc<AtomicBool>,
}

/// Runs the Comrade agent for a voice transcript. Implemented in the shell;
/// the voice core never imports the LLM layer (provider independence).
/// Hand-desugared (no async-trait dep) so it stays object-safe.
pub trait AgentRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        transcript: String,
        hooks: &'a VoiceHooks,
    ) -> Pin<Box<dyn Future<Output = AgentOutcome> + Send + 'a>>;
}

pub struct VoiceRuntimeConfig {
    pub silence_ms: u32,
    pub min_speech_ms: u32,
    pub max_utterance_ms: u32,
    pub vad_threshold: f32,
    pub decode_every_frames: usize,
    pub chunk_max_chars: usize,
    pub chunk_min_merge: usize,
    pub tts_voice: String,
    pub tts_speed: f32,
    pub num_threads: i32,
    /// Speak replies aloud. Off = agent still runs, text only.
    pub speak: bool,
}

impl Default for VoiceRuntimeConfig {
    fn default() -> Self {
        Self {
            silence_ms: 700,
            min_speech_ms: 250,
            max_utterance_ms: 30_000,
            vad_threshold: 0.5,
            decode_every_frames: 16, // ~0.5 s decode cadence
            chunk_max_chars: 220,
            chunk_min_merge: 12,
            tts_voice: "0".into(),
            tts_speed: 1.0,
            num_threads: 2,
            speak: true,
        }
    }
}

#[derive(Default)]
pub struct VoiceMetrics {
    pub init_ms: u64,
    pub first_partial_ms: Option<u64>,
    pub final_ms: Option<u64>,
    pub tts_first_audio_ms: Option<u64>,
    pub underruns: u64,
}

/// Everything needed to build engines once (keeps arg count clippy-clean).
pub struct EngineBuildConfig {
    pub models_base: std::path::PathBuf,
    pub vad_threshold: f32,
    pub vad_silence_ms: u32,
    pub vad_min_speech_ms: u32,
    pub tts_voice: String,
    pub tts_speed: f32,
    pub num_threads: i32,
}

/// Playback backend (cpal) or scripted fake (tests).
pub trait Playback: Send + Sync {
    fn push_pcm(&self, samples: &[f32], from_rate: u32);
    fn interrupt(&self) -> u64;
    fn drained(&self, gen: u64) -> bool;
    fn underruns(&self) -> u64;
}

pub struct CpalPlayback {
    player: crate::voice::playback::Player,
}

impl CpalPlayback {
    pub fn open_default(sample_rate: u32) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("playback_unavailable: no default speaker"))?;
        Ok(Self { player: crate::voice::playback::Player::open(&device, sample_rate)? })
    }
}

impl Playback for CpalPlayback {
    fn push_pcm(&self, samples: &[f32], from_rate: u32) {
        self.player.push_resampled(samples, from_rate);
    }

    fn interrupt(&self) -> u64 {
        self.player.handle().interrupt()
    }

    fn drained(&self, gen: u64) -> bool {
        self.player.handle().drained(gen)
    }

    fn underruns(&self) -> u64 {
        self.player.handle().stats().underruns
    }
}

pub struct Engines {
    pub(crate) vad: Box<dyn VadEngine>,
    pub(crate) stt: Box<dyn StreamRecognizer>,
    pub(crate) tts: Box<dyn SpeechSynth>,
    pub(crate) playback: Arc<dyn Playback>,
}

/// Engines shared across sessions (loaded once, never per-request).
pub type SharedEngines = Arc<Mutex<Engines>>;

enum ControlMsg {
    Stop,
    Cancel,
    /// End the current utterance now (push-to-talk release): finalize STT,
    /// run the turn, then continue/end per session mode.
    FinishTurn,
}

struct SessionCtx {
    engines: SharedEngines,
    audio_rx: tokio::sync::mpsc::Receiver<Vec<f32>>,
    control_rx: tokio::sync::mpsc::Receiver<ControlMsg>,
    emit: Arc<dyn Fn(VoiceEvent) + Send + Sync>,
    runner: Arc<dyn AgentRunner>,
    cfg: VoiceRuntimeConfig,
    one_shot: bool,
    session_cancel: Arc<AtomicBool>,
}

fn emit(ctx: &SessionCtx, event: VoiceEvent) {
    (ctx.emit)(event);
}

fn set_state(ctx: &SessionCtx, state: &Mutex<VoiceState>, next: VoiceState) {
    *state.lock().unwrap() = next;
    emit(ctx, VoiceEvent::StateChanged { state: next });
}

/// Synthesize one chunk off-thread (Kokoro blocks), honoring generation.
async fn synth_chunk(
    engines: SharedEngines,
    text: String,
    gen: u64,
    current_gen: &AtomicU64,
    index: usize,
) -> Option<(Vec<f32>, u32, u64)> {
    let started = Instant::now();
    let out = tokio::task::spawn_blocking(move || {
        let mut engines = engines.lock().unwrap();
        engines.tts.synthesize(&text).map(|pcm| (pcm.samples, pcm.sample_rate))
    })
    .await;
    match out {
        Ok(Ok((samples, rate))) => {
            if current_gen.load(Ordering::SeqCst) != gen {
                return None; // interrupted while synthesizing
            }
            Some((samples, rate, started.elapsed().as_millis() as u64))
        }
        Ok(Err(e)) => {
            log(Level::Warn, "VOICE", "tts chunk failed", Some(&serde_json::json!({ "error": format!("{e}").chars().take(160).collect::<String>() })));
            None
        }
        Err(e) => {
            log(Level::Warn, "VOICE", "tts task failed", Some(&serde_json::json!({ "error": format!("{e}").chars().take(160).collect::<String>() })));
            None
        }
    }
    .map(|(samples, rate, latency)| {
        let _ = index;
        (samples, rate, latency)
    })
}

/// Speak full text through chunk → synth → playback. Returns when drained or
/// interrupted. Drives Speaking state + first-audio metrics.
async fn speak_text(
    ctx: &SessionCtx,
    state: &Mutex<VoiceState>,
    text: &str,
    gen: &AtomicU64,
    my_gen: u64,
    metrics: &mut VoiceMetrics,
    chunk_index_base: &mut usize,
) {
    use crate::voice::chunk::split_sentences;
    let chunks = split_sentences(text, ctx.cfg.chunk_max_chars, ctx.cfg.chunk_min_merge);
    if chunks.is_empty() {
        return;
    }
    set_state(ctx, state, VoiceState::Speaking);
    emit(ctx, VoiceEvent::TtsStarted);
    let session_t0 = metrics_session_start();
    for chunk in chunks {
        if ctx.session_cancel.load(Ordering::SeqCst) {
            break;
        }
        if gen.load(Ordering::SeqCst) != my_gen {
            break; // interrupted by barge-in
        }
        let engines = ctx.engines.clone();
        let idx = *chunk_index_base;
        *chunk_index_base += 1;
        let chars = chunk.chars().count();
        if let Some((samples, rate, latency)) = synth_chunk(engines, chunk, my_gen, gen, idx).await {
            if metrics.tts_first_audio_ms.is_none() {
                metrics.tts_first_audio_ms = Some(session_t0.elapsed().as_millis() as u64);
                log(
                    Level::Info,
                    "VOICE",
                    "tts first audio",
                    Some(&serde_json::json!({ "latency_ms": metrics.tts_first_audio_ms })),
                );
            }
            let playback = {
                let engines = ctx.engines.lock().unwrap();
                engines.playback.clone()
            };
            playback.push_pcm(&samples, rate);
            emit(ctx, VoiceEvent::TtsChunk { index: idx, chars, latency_ms: latency });
        }
    }
}

/// Drain stale buffered audio (e.g. accumulated while the agent was thinking).
fn drain_audio(ctx: &mut SessionCtx) {
    while ctx.audio_rx.try_recv().is_ok() {}
}

enum MonitorOutcome {
    Drained,
    Barged,
    Cancelled,
}

/// Watch playback to completion while consuming mic audio for barge-in.
/// Sustained speech interrupts everything and hands back a fresh utterance.
async fn monitor_playback(
    ctx: &mut SessionCtx,
    state: &Mutex<VoiceState>,
    gen: &AtomicU64,
    my_gen: u64,
) -> MonitorOutcome {
    let mut barge_frames = 0usize;
    let barge_needed = ((ctx.cfg.min_speech_ms * 2 / 32).max(4)) as usize;
    let playback = {
        let engines = ctx.engines.lock().unwrap();
        engines.playback.clone()
    };
    loop {
        if gen.load(Ordering::SeqCst) != my_gen || ctx.session_cancel.load(Ordering::SeqCst) {
            return MonitorOutcome::Cancelled;
        }
        tokio::select! {
            biased;
            ctrl = ctx.control_rx.recv() => {
                match ctrl {
                    Some(ControlMsg::Cancel) | None => {
                        ctx.session_cancel.store(true, Ordering::SeqCst);
                        return MonitorOutcome::Cancelled;
                    }
                    Some(ControlMsg::Stop) => return MonitorOutcome::Cancelled,
                    Some(ControlMsg::FinishTurn) => {} // already speaking; ignore
                }
            }
            frame = ctx.audio_rx.recv() => {
                let Some(frame) = frame else {
                    return MonitorOutcome::Cancelled;
                };
                let is_speech = {
                    let mut engines = ctx.engines.lock().unwrap();
                    engines.vad.is_speech(&frame)
                };
                if is_speech {
                    barge_frames += 1;
                } else {
                    barge_frames = 0;
                }
                if barge_frames >= barge_needed {
                    log(Level::Info, "VOICE", "barge-in: interrupting TTS", None);
                    {
                        let engines = ctx.engines.lock().unwrap();
                        engines.playback.interrupt();
                    }
                    gen.fetch_add(1, Ordering::SeqCst);
                    {
                        let mut engines = ctx.engines.lock().unwrap();
                        engines.vad.reset();
                        engines.stt.reset();
                    }
                    emit(ctx, VoiceEvent::TtsInterrupted);
                    set_state(ctx, state, VoiceState::Interrupted);
                    set_state(ctx, state, VoiceState::Listening);
                    return MonitorOutcome::Barged;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                if playback.drained(my_gen) {
                    return MonitorOutcome::Drained;
                }
            }
        }
    }
}

fn metrics_session_start() -> Instant {
    Instant::now()
}

/// One conversation turn: listen → endpoint → agent → speak. Returns false
/// when the session should end.
async fn run_turn(
    ctx: &mut SessionCtx,
    state: &Mutex<VoiceState>,
    gen: &AtomicU64,
    metrics: &mut VoiceMetrics,
    chunk_index_base: &mut usize,
) -> bool {
    let mut tracker = UtteranceTracker::new(ctx.cfg.silence_ms, ctx.cfg.min_speech_ms, ctx.cfg.max_utterance_ms);
    let mut frames_since_decode = 0usize;
    let mut last_partial = String::new();
    let mut utterance_active = false;
    let mut last_level_emit = Instant::now() - std::time::Duration::from_secs(1);
    let turn_t0 = Instant::now();
    // Diagnostics: what the mic/VAD actually saw this turn.
    let mut diag_frames = 0usize;
    let mut diag_speech = 0usize;
    let mut diag_max_rms = 0.0f32;
    let mut diag_reason: &str;

    loop {
        tokio::select! {
            biased;
            ctrl = ctx.control_rx.recv() => {
                match ctrl {
                    Some(ControlMsg::Cancel) | None => {
                        ctx.session_cancel.store(true, Ordering::SeqCst);
                        return false;
                    }
                    Some(ControlMsg::Stop) => return false,
                    Some(ControlMsg::FinishTurn) => {
                        diag_reason = "release";
                        break;
                    }
                }
            }
            frame = ctx.audio_rx.recv() => {
                let Some(frame) = frame else {
                    emit(ctx, VoiceEvent::Error { message: "microphone ended unexpectedly".into() });
                    return false;
                };
                // Classify (short lock, fast op).
                let is_speech = {
                    let mut engines = ctx.engines.lock().unwrap();
                    engines.vad.is_speech(&frame)
                };
                diag_frames += 1;
                let energy = frame.iter().map(|v| v * v).sum::<f32>() / frame.len().max(1) as f32;
                let rms = energy.sqrt();
                if rms > diag_max_rms {
                    diag_max_rms = rms;
                }
                if is_speech {
                    diag_speech += 1;
                }
                // Mic level for the UI waveform (throttled; numbers only).
                if last_level_emit.elapsed().as_millis() >= 100 {
                    last_level_emit = Instant::now();
                    emit(ctx, VoiceEvent::AudioLevel { level: rms.clamp(0.0, 1.0) });
                }
                let current = *state.lock().unwrap();
                // Speaking-phase audio is consumed by monitor_playback, never here.
                if current != VoiceState::Listening {
                    continue; // drop mic audio while the agent works
                }

                match tracker.push(is_speech) {
                    Some(TrackerEvent::Started) => {
                        utterance_active = true;
                        frames_since_decode = 0;
                    }
                    Some(TrackerEvent::EndpointTooShort) => {
                        tracker.reset();
                        {
                            let mut engines = ctx.engines.lock().unwrap();
                            engines.stt.reset();
                        }
                        utterance_active = false;
                    }
                    Some(TrackerEvent::EndpointSilence) | Some(TrackerEvent::EndpointTooLong) => {
                        diag_reason = "vad-endpoint";
                        break;
                    }
                    None => {}
                }
                if !utterance_active {
                    continue;
                }
                {
                    let mut engines = ctx.engines.lock().unwrap();
                    engines.stt.accept(&frame);
                }
                frames_since_decode += 1;
                if frames_since_decode >= ctx.cfg.decode_every_frames {
                    frames_since_decode = 0;
                    let partial = {
                        let mut engines = ctx.engines.lock().unwrap();
                        engines.stt.decode();
                        engines.stt.partial_text()
                    };
                    if partial != last_partial && !partial.trim().is_empty() {
                        last_partial = partial.clone();
                        if metrics.first_partial_ms.is_none() {
                            metrics.first_partial_ms =
                                Some(turn_t0.elapsed().as_millis() as u64);
                            log(Level::Info, "VOICE", "stt first partial",
                                Some(&serde_json::json!({ "latency_ms": metrics.first_partial_ms })));
                        }
                        emit(ctx, VoiceEvent::PartialTranscript { text: partial });
                    }
                }
            }
        }
        if ctx.session_cancel.load(Ordering::SeqCst) {
            return false;
        }
    }

    // Endpoint reached: finalize.
    let final_text = {
        let mut engines = ctx.engines.lock().unwrap();
        engines.stt.finalize()
    };
    metrics.final_ms = Some(turn_t0.elapsed().as_millis() as u64);
    log(Level::Info, "VOICE", "stt final",
        Some(&serde_json::json!({
            "latency_ms": metrics.final_ms,
            "chars": final_text.chars().count(),
            "endpoint": diag_reason,
            "frames": diag_frames,
            "vad_speech_frames": diag_speech,
            "max_rms": (diag_max_rms * 1000.0).round() / 1000.0,
        })));
    if final_text.trim().is_empty() {
        set_state(ctx, state, VoiceState::Listening);
        return true; // nothing said; keep listening
    }
    emit(ctx, VoiceEvent::FinalTranscript { text: final_text.clone() });
    set_state(ctx, state, VoiceState::Processing);

    // Agent turn with live token → chunk → speak pipeline.
    let (token_tx, mut token_rx) = tokio::sync::mpsc::channel::<String>(64);
    let hooks_cancel = ctx.session_cancel.clone();
    let hooks = VoiceHooks {
        on_token: Arc::new(move |tok: String| {
            let _ = token_tx.try_send(tok);
        }),
        cancel: hooks_cancel,
    };
    let outcome = ctx.runner.run(final_text, &hooks).await;
    drop(hooks);
    // Drop audio buffered while the agent was thinking (stale for barge-in).
    drain_audio(ctx);
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;
    let mut buffer = SentenceBuffer::new(ctx.cfg.chunk_max_chars, ctx.cfg.chunk_min_merge);
    let mut streamed = String::new();
    while let Ok(tok) = token_rx.try_recv() {
        emit(ctx, VoiceEvent::AgentToken { token: tok.clone() });
        streamed.push_str(&tok);
        for chunk in buffer.push_token(&tok) {
            if ctx.cfg.speak {
            speak_text(ctx, state, &chunk, gen, my_gen, metrics, chunk_index_base).await;
        }
            if ctx.session_cancel.load(Ordering::SeqCst) {
                return false;
            }
        }
    }
    for chunk in buffer.finish() {
        if ctx.cfg.speak {
            speak_text(ctx, state, &chunk, gen, my_gen, metrics, chunk_index_base).await;
        }
        if ctx.session_cancel.load(Ordering::SeqCst) {
            return false;
        }
    }
    if !outcome.ok && ctx.cfg.speak {
        speak_text(ctx, state, "Sorry, something went wrong.", gen, my_gen, metrics, chunk_index_base).await;
    }
    // Fallback: agent produced text but no tokens streamed (non-streaming runner).
    if streamed.trim().is_empty() && !outcome.text.trim().is_empty() && outcome.ok && ctx.cfg.speak {
        speak_text(ctx, state, &outcome.text.clone(), gen, my_gen, metrics, chunk_index_base).await;
    }
    let _ = streamed;
    // Barge-aware wait: consumes mic audio until spoken, interrupted, or gone.
    match monitor_playback(ctx, state, gen, my_gen).await {
        MonitorOutcome::Drained => {
            // Deaf window: swallow our own echo tail before listening again.
            drain_audio(ctx);
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
            while std::time::Instant::now() < deadline {
                match tokio::time::timeout(std::time::Duration::from_millis(50), ctx.audio_rx.recv()).await {
                    Ok(Some(_)) => {}
                    _ => break,
                }
            }
        }
        MonitorOutcome::Barged => {}
        MonitorOutcome::Cancelled => return false,
    }
    {
        let playback = ctx.engines.lock().unwrap().playback.clone();
        metrics.underruns = playback.underruns();
    }
    set_state(ctx, state, VoiceState::Listening);
    {
        let mut engines = ctx.engines.lock().unwrap();
        engines.vad.reset();
        engines.stt.reset();
    }
    true
}

async fn run_session(mut ctx: SessionCtx, state: Arc<Mutex<VoiceState>>, gen: Arc<AtomicU64>) {
    let mut metrics = VoiceMetrics::default();
    let mut chunk_index_base = 0usize;
    set_state(&ctx, &state, VoiceState::Listening);
    emit(&ctx, VoiceEvent::Started);
    loop {
        if ctx.session_cancel.load(Ordering::SeqCst) {
            break;
        }
        let keep_going = run_turn(&mut ctx, &state, &gen, &mut metrics, &mut chunk_index_base).await;
        if !keep_going {
            break;
        }
        if ctx.one_shot {
            break;
        }
    }
    set_state(&ctx, &state, VoiceState::Idle);
    emit(&ctx, VoiceEvent::Stopped { reason: "session ended".into() });
    log(
        Level::Info,
        "VOICE",
        "session metrics",
        Some(&serde_json::json!({
            "first_partial_ms": metrics.first_partial_ms,
            "final_ms": metrics.final_ms,
            "tts_first_audio_ms": metrics.tts_first_audio_ms,
            "underruns": metrics.underruns,
        })),
    );
}

/// Cloneable Tauri-facing controller. Engines load once, lazily.
#[derive(Clone)]
pub struct VoiceController {
    inner: Arc<ControllerInner>,
}

struct ControllerInner {
    state: Arc<Mutex<VoiceState>>,
    engines: Mutex<Option<SharedEngines>>,
    session: Mutex<Option<(tokio::task::JoinHandle<()>, tokio::sync::mpsc::Sender<ControlMsg>)>>,
    emit: Box<dyn Fn(VoiceEvent) + Send + Sync>,
    runner: Arc<dyn AgentRunner>,
    mic: Mutex<String>,
}

impl VoiceController {
    pub fn new(
        emit: impl Fn(VoiceEvent) + Send + Sync + 'static,
        runner: Arc<dyn AgentRunner>,
    ) -> Self {
        Self {
            inner: Arc::new(ControllerInner {
                state: Arc::new(Mutex::new(VoiceState::Idle)),
                engines: Mutex::new(None),
                session: Mutex::new(None),
                emit: Box::new(emit),
                runner,
                mic: Mutex::new(String::new()),
            }),
        }
    }

    pub fn state(&self) -> VoiceState {
        *self.inner.state.lock().unwrap()
    }

    fn emit(&self, event: VoiceEvent) {
        (self.inner.emit)(event);
    }

    /// Load models + build engines once (idempotent). Logs init latency.
    pub async fn ensure_engines(&self, build: EngineBuildConfig) -> anyhow::Result<SharedEngines> {
        ensure_shared_engines(&self.inner.engines, build).await
    }

    pub fn set_microphone(&self, name: &str) {
        *self.inner.mic.lock().unwrap() = name.to_string();
    }
}

/// Load models + build engines once per slot (idempotent). Logs init latency.
pub async fn ensure_shared_engines(
    slot: &Mutex<Option<SharedEngines>>,
    build: EngineBuildConfig,
) -> anyhow::Result<SharedEngines> {
    if let Some(engines) = slot.lock().unwrap().clone() {
        return Ok(engines);
    }
    let t0 = Instant::now();
    let stt_dir = build.models_base.join("stt");
    let vad_dir = build.models_base.join("vad");
    let tts_dir = build.models_base.join("tts");
    for (label, path) in [
        ("stt encoder", stt_dir.join("encoder-epoch-99-avg-1.int8.onnx")),
        ("stt decoder", stt_dir.join("decoder-epoch-99-avg-1.int8.onnx")),
        ("stt joiner", stt_dir.join("joiner-epoch-99-avg-1.int8.onnx")),
        ("stt tokens", stt_dir.join("tokens.txt")),
        ("vad model", vad_dir.join("silero_vad.onnx")),
    ] {
        if !path.is_file() {
            anyhow::bail!("model_missing: {label} not found at {}", path.display());
        }
    }
    let kokoro = crate::voice::tts::find_kokoro_files(&tts_dir)
        .map_err(|e| anyhow::anyhow!("model_missing: kokoro assets: {e}"))?;
    let sid: i32 = build.tts_voice.parse().unwrap_or(0);
    let vad_threshold = build.vad_threshold;
    let vad_silence_ms = build.vad_silence_ms;
    let vad_min_speech_ms = build.vad_min_speech_ms;
    let num_threads = build.num_threads;
    let tts_speed = build.tts_speed;
    // Heavy constructors off the async executor.
    let stt_dir_c = stt_dir.clone();
    let vad_path = vad_dir.join("silero_vad.onnx").to_string_lossy().to_string();
    let engines = tokio::task::spawn_blocking(move || -> anyhow::Result<Engines> {
        let vad = SherpaSileroVad::new(
            &vad_path,
            vad_threshold,
            vad_silence_ms as f32,
            vad_min_speech_ms as f32,
            1,
        )?;
        let stt = SherpaOnlineZipformer::new(
            &stt_dir_c.join("encoder-epoch-99-avg-1.int8.onnx").to_string_lossy(),
            &stt_dir_c.join("decoder-epoch-99-avg-1.int8.onnx").to_string_lossy(),
            &stt_dir_c.join("joiner-epoch-99-avg-1.int8.onnx").to_string_lossy(),
            &stt_dir_c.join("tokens.txt").to_string_lossy(),
            num_threads,
        )?;
        let tts = SherpaKokoro::new(&kokoro, sid, tts_speed, 1)?;
        let playback = Arc::new(CpalPlayback::open_default(24000)?) as Arc<dyn Playback>;
        Ok(Engines { vad: Box::new(vad), stt: Box::new(stt), tts: Box::new(tts), playback })
    })
    .await
    .map_err(|e| anyhow::anyhow!("engine init task failed: {e}"))??;
    let shared: SharedEngines = Arc::new(std::sync::Mutex::new(engines));
    *slot.lock().unwrap() = Some(shared.clone());
    let ms = t0.elapsed().as_millis() as u64;
    log(Level::Info, "VOICE", "engines ready", Some(&serde_json::json!({ "init_ms": ms })));
    Ok(shared)
}

impl VoiceController {
    /// Begin a voice session. `one_shot` ends after the first reply.
    pub async fn start_session(
        &self,
        engines: SharedEngines,
        cfg: VoiceRuntimeConfig,
        one_shot: bool,
    ) -> anyhow::Result<()> {
        if self.state() != VoiceState::Idle {
            anyhow::bail!("voice busy: already {:?}", self.state());
        }
        let mic = self.inner.mic.lock().unwrap().clone();
        let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(128);
        let capture = crate::voice::capture::MicCapture::start(&mic, audio_tx).map_err(|e| {
            anyhow::anyhow!("microphone_unavailable: {e}")
        })?;
        let (control_tx, control_rx) = tokio::sync::mpsc::channel::<ControlMsg>(8);
        let emit = {
            let inner = self.inner.clone();
            Arc::new(move |e: VoiceEvent| (inner.emit)(e)) as Arc<dyn Fn(VoiceEvent) + Send + Sync>
        };
        let ctx = SessionCtx {
            engines,
            audio_rx,
            control_rx,
            emit,
            runner: self.inner.runner.clone(),
            cfg,
            one_shot,
            session_cancel: Arc::new(AtomicBool::new(false)),
        };
        let state = self.inner.state.clone();
        let gen = Arc::new(AtomicU64::new(0));
        let reset_state = state.clone();
        let handle = tokio::spawn(async move {
            // Keep capture alive for the whole session.
            let _capture = capture;
            run_session(ctx, state, gen).await;
            *reset_state.lock().unwrap() = VoiceState::Idle;
        });
        *self.inner.session.lock().unwrap() = Some((handle, control_tx));
        Ok(())
    }

    pub async fn finish_turn(&self) {
        let sender = self.inner.session.lock().unwrap().as_ref().map(|(_, tx)| tx.clone());
        if let Some(tx) = sender {
            let _ = tx.send(ControlMsg::FinishTurn).await;
        }
    }

    pub async fn stop_session(&self, reason: &str) {
        let sender = self.inner.session.lock().unwrap().as_ref().map(|(_, tx)| tx.clone());
        if let Some(tx) = sender {
            let _ = tx.send(ControlMsg::Stop).await;
        }
        self.emit(VoiceEvent::Stopped { reason: reason.into() });
    }

    pub async fn cancel_session(&self) {
        let sender = self.inner.session.lock().unwrap().as_ref().map(|(_, tx)| tx.clone());
        if let Some(tx) = sender {
            let _ = tx.send(ControlMsg::Cancel).await;
        }
    }

    /// For tests: drive a scripted session without hardware.
    #[cfg(test)]
    pub async fn test_session(
        engines: SharedEngines,
        audio_rx: tokio::sync::mpsc::Receiver<Vec<f32>>,
        runner: Arc<dyn AgentRunner>,
        cfg: VoiceRuntimeConfig,
        one_shot: bool,
    ) {
        let (control_tx, control_rx) = tokio::sync::mpsc::channel::<ControlMsg>(8);
        // Held alive so the session isn't cancelled: dropping the last
        // sender would end every control poll immediately.
        let _keepalive = control_tx;
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_cb = events.clone();
        let ctx = SessionCtx {
            engines,
            audio_rx,
            control_rx,
            emit: Arc::new(move |e: VoiceEvent| {
                events_cb.lock().unwrap().push(format!("{e:?}"));
            }),
            runner,
            cfg,
            one_shot,
            session_cancel: Arc::new(AtomicBool::new(false)),
        };
        let state = Arc::new(Mutex::new(VoiceState::Idle));
        let gen = Arc::new(AtomicU64::new(0));
        run_session(ctx, state, gen).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::audio::FRAME_SAMPLES;
    use crate::voice::stt::FakeRecognizer;
    use crate::voice::tts::FakeSynth;
    use crate::voice::vad::FakeVad;

    struct FakePlayback {
        pushes: Arc<AtomicU64>,
        interrupts: Arc<AtomicU64>,
        drain_open: AtomicBool,
    }

    impl FakePlayback {
        fn new(drain_open: bool) -> (Self, Arc<AtomicU64>, Arc<AtomicU64>) {
            let pushes = Arc::new(AtomicU64::new(0));
            let interrupts = Arc::new(AtomicU64::new(0));
            (
                Self { pushes: pushes.clone(), interrupts: interrupts.clone(), drain_open: AtomicBool::new(drain_open) },
                pushes,
                interrupts,
            )
        }
    }

    impl Playback for FakePlayback {
        fn push_pcm(&self, samples: &[f32], _from_rate: u32) {
            self.pushes.fetch_add(samples.len() as u64, Ordering::SeqCst);
        }

        fn interrupt(&self) -> u64 {
            self.interrupts.fetch_add(1, Ordering::SeqCst) + 1
        }

        fn drained(&self, _gen: u64) -> bool {
            self.drain_open.load(Ordering::SeqCst)
        }

        fn underruns(&self) -> u64 {
            0
        }
    }

    struct ScriptRunner {
        tokens: Vec<String>,
        text: String,
    }

    impl AgentRunner for ScriptRunner {
        fn run<'a>(
            &'a self,
            _transcript: String,
            hooks: &'a VoiceHooks,
        ) -> Pin<Box<dyn Future<Output = AgentOutcome> + Send + 'a>> {
            Box::pin(async move {
                for tok in &self.tokens {
                    (hooks.on_token)(tok.clone());
                }
                AgentOutcome { text: self.text.clone(), ok: true }
            })
        }
    }

    fn test_engines(
        vad_pattern: Vec<bool>,
        partials: Vec<String>,
        drain_open: bool,
    ) -> (SharedEngines, Arc<AtomicU64>, Arc<AtomicU64>) {
        let (playback, pushes, interrupts) = FakePlayback::new(drain_open);
        let engines: SharedEngines = Arc::new(std::sync::Mutex::new(Engines {
            vad: Box::new(FakeVad::new(vad_pattern)),
            stt: Box::new(FakeRecognizer::new(partials)),
            tts: Box::new(FakeSynth::new()),
            playback: Arc::new(playback),
        }));
        (engines, pushes, interrupts)
    }

    fn silent_frame() -> Vec<f32> {
        vec![0.0f32; FRAME_SAMPLES]
    }

    fn test_cfg() -> VoiceRuntimeConfig {
        VoiceRuntimeConfig {
            silence_ms: 224, // 7 frames
            min_speech_ms: 96, // 3 frames
            max_utterance_ms: 30_000,
            decode_every_frames: 2,
            ..VoiceRuntimeConfig::default()
        }
    }

    #[tokio::test]
    async fn one_shot_session_transcribes_and_speaks() {
        // silence, speech long enough to endpoint, silence.
        let mut pattern = vec![false; 5];
        pattern.extend(vec![true; 12]);
        pattern.extend(vec![false; 20]);
        let (engines, pushes, _) = test_engines(
            pattern,
            vec!["open".into(), "open my project".into()],
            true,
        );
        let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(128);
        let runner = Arc::new(ScriptRunner {
            tokens: vec!["Hi ".into(), "there. ".into()],
            text: "Hi there.".into(),
        });
        let send = tokio::spawn(async move {
            for _ in 0..(5 + 12 + 20) {
                if audio_tx.send(silent_frame()).await.is_err() {
                    break;
                }
            }
        });
        VoiceController::test_session(engines, audio_rx, runner, test_cfg(), true).await;
        let _ = send.await;
        // TTS spoke the streamed reply through chunked synth + playback.
        assert!(pushes.load(Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn barge_in_interrupts_playback() {
        // A word, silence (endpoint), then sustained speech during TTS.
        let mut pattern = vec![false; 3];
        pattern.extend(vec![true; 6]);
        pattern.extend(vec![false; 10]);
        pattern.extend(vec![true; 60]);
        let (engines, pushes, interrupts) = test_engines(
            pattern,
            vec!["hi".into(), "tell me more".into()],
            false, // never drains: TTS stays Speaking until barged
        );
        let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(256);
        let runner = Arc::new(ScriptRunner {
            tokens: vec!["A long reply that keeps speaking. ".into()],
            text: "A long reply that keeps speaking.".into(),
        });
        let send = tokio::spawn(async move {
            for _ in 0..(3 + 6 + 10 + 60) {
                if audio_tx.send(silent_frame()).await.is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
        VoiceController::test_session(engines, audio_rx, runner, test_cfg(), true).await;
        let _ = send.await;
        assert!(interrupts.load(Ordering::SeqCst) >= 1, "barge-in must interrupt playback");
        assert!(pushes.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn state_machine_starts_idle() {
        let c = VoiceController::new(|_| {}, Arc::new(ScriptRunner { tokens: vec![], text: String::new() }));
        assert_eq!(c.state(), VoiceState::Idle);
    }
}
