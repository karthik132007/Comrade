# Comrade Local Voice Pipeline

Fully offline realtime voice: mic → Silero VAD → streaming Zipformer STT →
Comrade agent → sentence-chunked Kokoro TTS → speakers, with barge-in
(interrupt Comrade mid-sentence by talking). No cloud STT/TTS anywhere.

## 1. Architecture

```text
Tauri commands/events
  start/stop/cancel_voice_input, mic select, model status/download
  voice-event: started, partial/final transcript, states, errors, model status
        │
        ▼
VoiceController (core/src/voice/manager.rs, Clone + Send)
  state machine: Idle → Listening → Processing → Speaking ⇄ Interrupted
        │
        ▼
session task (one per voice session)
  mic frames ──► VAD ──► UtteranceTracker ──► streaming STT ──► partials
       endpoint │                                              │ final
                ▼                                              ▼
        barge-in watcher ◄── monitor consumes mic DURING speech
                │
  final ──► AgentRunner (shell trait; LLM stays independent)
                │ tokens
                ▼
  SentenceBuffer ──► Kokoro (spawn_blocking) ──► cpal playback queue
```

Engines load once and are shared across sessions (`SharedEngines`). Blocking
inference (Kokoro synth, engine construction) runs in `spawn_blocking`; the
Tauri thread never blocks. STT decode (~100 ms) runs inline in the session.

Key invariant: mic audio is consumed during Listening (VAD/STT), dropped
during Processing, and consumed by the barge monitor during Speaking. Stale
buffered audio is drained at phase transitions so old speech can never trigger
a false utterance, and a 200 ms deaf window swallows the TTS echo tail.

## 2. Required models

All Apache-2.0-licensed weights unless noted. Stored under
`<comrade-agent>/models/` (see `core/src/voice/models.rs`):

| Pack | Files | Size | Source |
|------|-------|------|--------|
| `stt` | `encoder-epoch-99-avg-1.int8.onnx` (42.8 MB), `decoder-*.int8.onnx`, `joiner-*.int8.onnx`, `tokens.txt` | ~43 MB | `csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17` (HuggingFace). 20M-param English streaming transducer, INT8 for CPU. |
| `vad` | `silero_vad.onnx` (644 KB) | <1 MB | `k2-fsa/sherpa-onnx` release `asr-models` (Silero originals: MIT). |
| `tts` | `model.onnx` + `voices.bin` + `tokens.txt` + `espeak-ng-data/` + lexicons | ~207 MB extracted (140 MB archive) | `k2-fsa/sherpa-onnx` release `tts-models`, `kokoro-int8-multi-lang-v1_1.tar.bz2` (Kokoro-82M INT8). |
| `stt/test` | `0.wav`, `trans.txt` | <1 MB | Same HF repo; used by the offline integration test. |

Why these: the 20M Zipformer is the smallest English *streaming* transducer
sherpa publishes (genuine frame-in/partial-out, not buffer-loop offline);
INT8 keeps it laptop-quiet. Kokoro-82M-int8 is the only spec-compliant local
TTS at reasonable quality per MB. Silero is a 2 MB frame classifier, ideal for
gating.

## 3. Model download/setup

First run (or Settings → Download) fetches each missing file with progress
events (`voice-download`), verifies Content-Length, records SHA-256 + size in
`models/manifest.json`, extracts the Kokoro tarball, and deletes the archive.
Retries 3× with backoff; partial files are never kept. Re-running skips
anything whose size matches the manifest.

```bash
cargo run -p comrade-core --example fetch-models   # CLI download with progress
```

## 4. Running completely offline

After `manifest.json` is complete, unplug the network: VAD, STT, and TTS run
entirely on CPU via the vendored sherpa-onnx runtime. The app only needs the
network for the LLM call itself (DeepSeek) — voice I/O never phones home.
Mic audio is never written to disk and never leaves the process.

## 5. Supported platforms

Linux (primary, tested), macOS and Windows via the same crates
(`sherpa-rs-sys` ships prebuilt libs per target; cpal covers CoreAudio/WASAPI).
`sherpa-rs` upstream was archived Jun 2026 — pinned at 0.6.8; all sherpa usage
sits behind `core/src/voice` traits (`VadEngine`, `StreamRecognizer`,
`SpeechSynth`, `Playback`), so a runtime swap is contained.

## 6. CPU/RAM expectations (measured, x86-64 desktop)

- Download size ≈ 185 MB; on disk ≈ 250 MB.
- STT init ≈ 1.4 s; VAD negligible; Kokoro init ≈ 4.8 s (once per app run).
- Kokoro int8: ~2.3 s of audio in ~4.8 s cold (≈0.5× realtime); warm chunks
  are faster. First spoken audio typically lands <1.5 s after the first
  sentence completes.
- Streaming decode ≈ tens of ms per 0.5 s tick on 2 threads; partials feel
  instant. RAM ≈ a few hundred MB with all engines loaded.
- Battery: engines idle at zero cost between sessions (streams closed, no
  polling); capture thread sleeps when no session is active.

## 7. Troubleshooting microphone issues

- "microphone_unavailable": no default input or permission denied. On Linux,
  check PipeWire/PulseAudio (`pavucontrol`), and that the sandbox (Flatpak)
  exposes the mic. Select an explicit device in Settings → Voice.
- Empty transcripts: mic muted at hardware/OS level, or wrong device picked.
  Watch for `dropped_frames` in logs (channel overflow = consumer stall).
- Non-16 kHz / stereo mics are converted automatically (logged once per
  session); exotic sample formats error clearly instead of misbehaving.
- WebKitGTK webviews cannot capture audio — capture is native (cpal) in the
  backend, which is why push-to-talk needs no browser permission.

## 8. Troubleshooting model loading

- `model_missing: <file> not found at …` → Settings → Download, or run the
  fetch example. Progress + retries are logged.
- `Kokoro probe failed` → re-download the TTS pack (delete
  `models/tts/.extracted` to force re-extract).
- `playback_unavailable` → no output device (headless/RDP). STT-only use still
  needs a playback device object today — run with a dummy sink if headless.
- Version skew: `manifest.json` records sizes; tampering/corruption is caught
  on the next ensure pass (missing → re-download).

## 9. How STT streaming works

One persistent `OnlineRecognizer` + `OnlineStream` per engine lifetime
(`core/src/voice/stt.rs`, direct `sherpa-rs-sys` bindings — the safe
`sherpa-rs` crate only wraps the *offline* recognizer). Each 32 ms mic frame
goes to `AcceptWaveform`; every ~0.5 s the session calls `DecodeOnlineStream`
in a ready-loop and reads the hypothesis. Text is emitted as partial events
only when it changes; on VAD endpoint the stream is finalized
(`InputFinished` + decode + result) and **recreated** for the next utterance.
Proven by the ignored integration test feeding a WAV in 0.5 s chunks and
asserting partials grow (`THE YE` → full sentence).

## 10. How TTS streaming works

LLM tokens flow into `SentenceBuffer` (`core/src/voice/chunk.rs`):
boundaries on `. ! ? …` + CJK punctuation (abbreviation-aware: `Dr.`, `p.m.`),
newline splits, 220-char hard cap, and short fragments (`<12` chars) held back
and fused forward so `Hey!` never speaks alone. Each ready sentence is
synthesized in `spawn_blocking` (never blocking the session) and appended to
the cpal playback queue with a generation id. Playback starts on the first
chunk while the LLM is still generating — no waiting for the full response.
Cancellation bumps the generation: in-flight synth results are dropped before
push, and the queue drains instantly (`interrupt()`).

Barge-in: while Speaking, the monitor consumes mic frames through VAD; ~0.5 s
of sustained speech stops playback, resets VAD/STT, and returns to Listening
with the echo tail swallowed by a 200 ms deaf window. Caveat: loudspeakers
leak into the mic, so sustained-menu-music-level background speech can false
trigger — headphones recommended, threshold tunable in `comrade.conf`.

## Licenses / redistribution

- sherpa-onnx runtime + Zipformer weights + Kokoro weights: **Apache-2.0**.
- Silero VAD weights: **MIT** (via sherpa-onnx release assets).
- `espeak-ng-data` inside the Kokoro bundle: **GPL-3.0** (G2P data). Review
  before distributing binaries commercially.
- `sherpa-rs`/`sherpa-rs-sys` bindings: MIT (upstream archived — pinned).

## Testing voice independently

```bash
# unit (no hardware, no network, no models)
cargo test -p comrade-core --lib voice::

# offline integration (needs models, still no mic/network for inference)
cargo test -p comrade-core --test voice_local -- --ignored --nocapture

# full download (one time, ~185 MB)
cargo run -p comrade-core --example fetch-models
```

The live loop itself is covered by scripted session tests
(`one_shot_session_transcribes_and_speaks`, `barge_in_interrupts_playback`)
with fake VAD/STT/TTS/playback — including a regression test for the
speak-phase audio-consumption hang.
