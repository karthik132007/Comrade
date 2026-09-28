/**
 * Voice activity detection. Frame classification is pluggable (Silero via
 * sherpa-onnx, or a fake in tests); utterance endpointing is pure logic here
 * so silence timeouts and short pauses behave identically everywhere.
 */
use crate::voice::audio::FRAME_SAMPLES;

/// Per-frame classifier over 16 kHz mono audio. Frames of FRAME_SAMPLES.
pub trait VadEngine: Send {
    /// Classify one frame; true = speech.
    fn is_speech(&mut self, frame_16k_mono: &[f32]) -> bool;
    fn reset(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackerEvent {
    /// Sustained speech began (caller should start/continue STT).
    Started,
    /// Endpoint: silence timeout after speech (finalize STT).
    EndpointSilence,
    /// Endpoint: utterance too short to be real (discard).
    EndpointTooShort,
    /// Endpoint: hit the length cap (finalize what we have).
    EndpointTooLong,
}

pub struct UtteranceTracker {
    silence_frames_needed: usize,
    min_speech_frames: usize,
    max_frames: usize,
    speech_frames: usize,
    silence_frames: usize,
    in_utterance: bool,
    total_frames: usize,
}

impl UtteranceTracker {
    pub fn new(silence_ms: u32, min_speech_ms: u32, max_ms: u32) -> Self {
        // 32 ms per frame.
        let f = |ms: u32| ((ms / 32).max(1)) as usize;
        Self {
            silence_frames_needed: f(silence_ms),
            min_speech_frames: f(min_speech_ms),
            max_frames: f(max_ms),
            speech_frames: 0,
            silence_frames: 0,
            in_utterance: false,
            total_frames: 0,
        }
    }

    pub fn reset(&mut self) {
        self.speech_frames = 0;
        self.silence_frames = 0;
        self.in_utterance = false;
        self.total_frames = 0;
    }

    pub fn in_utterance(&self) -> bool {
        self.in_utterance
    }

    /// Feed one classified frame. Short pauses ride through: only sustained
    /// silence ends the utterance. An endpoint is terminal — the tracker
    /// auto-resets so continued audio starts a fresh utterance.
    pub fn push(&mut self, is_speech: bool) -> Option<TrackerEvent> {
        self.total_frames += 1;
        if is_speech {
            self.silence_frames = 0;
            self.speech_frames += 1;
            if !self.in_utterance && self.speech_frames >= self.min_speech_frames {
                self.in_utterance = true;
                return Some(TrackerEvent::Started);
            }
        } else if self.in_utterance {
            self.silence_frames += 1;
            if self.silence_frames >= self.silence_frames_needed {
                self.reset();
                return Some(TrackerEvent::EndpointSilence);
            }
        }
        if self.in_utterance && self.total_frames >= self.max_frames {
            self.reset();
            return Some(TrackerEvent::EndpointTooLong);
        }
        if !self.in_utterance && self.total_frames >= self.max_frames {
            let had_speech = self.speech_frames > 0;
            self.reset();
            if had_speech {
                return Some(TrackerEvent::EndpointTooShort);
            }
            return None; // pure silence: quiet housekeeping, no event
        }
        None
    }
}

/// Silero VAD through sherpa-onnx. `accept` buffers into FRAME_SAMPLES windows.
pub struct SherpaSileroVad {
    vad: sherpa_rs::silero_vad::SileroVad,
    pending: Vec<f32>,
    threshold: f32,
}

impl SherpaSileroVad {
    pub fn new(
        model_path: &str,
        threshold: f32,
        silence_ms: f32,
        min_speech_ms: f32,
        num_threads: i32,
    ) -> anyhow::Result<Self> {
        let vad = sherpa_rs::silero_vad::SileroVad::new(
            sherpa_rs::silero_vad::SileroVadConfig {
                model: model_path.to_string(),
                threshold,
                min_silence_duration: silence_ms / 1000.0,
                min_speech_duration: min_speech_ms / 1000.0,
                max_speech_duration: 30.0,
                sample_rate: 16000,
                window_size: FRAME_SAMPLES as i32,
                provider: None,
                num_threads: Some(num_threads),
                debug: false,
            },
            60.0,
        )
        .map_err(|e| anyhow::anyhow!("VAD init failed: {e}"))?;
        Ok(Self { vad, pending: Vec::with_capacity(FRAME_SAMPLES * 2), threshold })
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }
}

impl VadEngine for SherpaSileroVad {
    fn is_speech(&mut self, frame_16k_mono: &[f32]) -> bool {
        self.pending.extend_from_slice(frame_16k_mono);
        let mut speech = false;
        while self.pending.len() >= FRAME_SAMPLES {
            let window: Vec<f32> = self.pending.drain(..FRAME_SAMPLES).collect();
            // Sherpa's segmenter tracks endpoints; we only need the live flag.
            self.vad.accept_waveform(window);
            if self.vad.is_speech() {
                speech = true;
            }
            // Drain endpointed segments so the internal buffer can't grow
            // unboundedly while we do our own endpointing.
            while !self.vad.is_empty() {
                self.vad.pop();
            }
        }
        speech
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.vad.clear();
    }
}

/// Deterministic script for tests: yields the pattern, then silence.
pub struct FakeVad {
    pattern: Vec<bool>,
    pos: usize,
}

impl FakeVad {
    pub fn new(pattern: Vec<bool>) -> Self {
        Self { pattern, pos: 0 }
    }
}

impl VadEngine for FakeVad {
    fn is_speech(&mut self, _frame: &[f32]) -> bool {
        let v = *self.pattern.get(self.pos).unwrap_or(&false);
        self.pos += 1;
        v
    }

    fn reset(&mut self) {
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(n: usize, speech: bool) -> Vec<bool> {
        vec![speech; n]
    }

    #[test]
    fn endpoint_after_sustained_silence() {
        // 250 ms min speech (8 frames), 700 ms silence (~22 frames).
        let mut t = UtteranceTracker::new(700, 250, 30_000);
        let mut events = Vec::new();
        for s in frames(10, true).into_iter().chain(frames(30, false)) {
            if let Some(e) = t.push(s) {
                events.push(e);
            }
        }
        assert_eq!(events, vec![TrackerEvent::Started, TrackerEvent::EndpointSilence]);
    }

    #[test]
    fn short_pause_does_not_end_utterance() {
        let mut t = UtteranceTracker::new(700, 250, 30_000);
        let mut events = Vec::new();
        // speech, 300 ms pause, speech again, then long silence.
        let seq: Vec<bool> = frames(10, true)
            .into_iter()
            .chain(frames(9, false))
            .chain(frames(10, true))
            .chain(frames(30, false))
            .collect();
        for s in seq {
            if let Some(e) = t.push(s) {
                events.push(e);
            }
        }
        assert_eq!(events, vec![TrackerEvent::Started, TrackerEvent::EndpointSilence]);
    }

    #[test]
    fn blips_below_min_speech_are_ignored() {
        let mut t = UtteranceTracker::new(700, 250, 30_000);
        let mut events = Vec::new();
        for s in frames(3, true).into_iter().chain(frames(5, false)) {
            if let Some(e) = t.push(s) {
                events.push(e);
            }
        }
        assert!(events.is_empty());
        assert!(!t.in_utterance());
    }

    #[test]
    fn overly_long_utterance_force_ends() {
        let mut t = UtteranceTracker::new(700, 250, 1000);
        let mut events = Vec::new();
        for s in frames(200, true) {
            if let Some(e) = t.push(s) {
                events.push(e);
            }
        }
        assert!(events.contains(&TrackerEvent::EndpointTooLong));
    }

    #[test]
    fn fake_vad_replays_pattern() {
        let mut vad = FakeVad::new(vec![true, true, false]);
        let frame = vec![0.0f32; FRAME_SAMPLES];
        assert!(vad.is_speech(&frame));
        assert!(vad.is_speech(&frame));
        assert!(!vad.is_speech(&frame));
        assert!(!vad.is_speech(&frame)); // exhausted → silence
    }
}
