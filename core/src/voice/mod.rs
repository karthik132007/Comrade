/**
 * Fully local realtime voice pipeline: mic → VAD → streaming STT → agent →
 * chunked TTS → speakers. No cloud APIs; models live under the comrade-agent
 * home and run offline after first install.
 */
pub mod audio;
pub mod capture;
pub mod chunk;
pub mod manager;
pub mod models;
pub mod playback;
pub mod stt;
pub mod tts;
pub mod vad;
