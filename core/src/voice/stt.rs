/**
 * Genuinely streaming STT: one persistent online Zipformer/Transducer stream,
 * frames in, partials out. Built on sherpa-rs-sys (the safe `sherpa-rs` crate
 * only wraps the OFFLINE recognizer, which would be buffer-loop transcription).
 */
use std::ffi::CString;
use std::mem;

/// Streaming recognizer interface. Implementations must be Send; a single
/// owner drives them (no concurrent use).
pub trait StreamRecognizer: Send {
    /// Feed 16 kHz mono samples (any chunk size).
    fn accept(&mut self, samples: &[f32]);
    /// Run the decoder on buffered audio.
    fn decode(&mut self);
    /// Current best hypothesis (may revise as audio grows).
    fn partial_text(&mut self) -> String;
    /// Decode once more and return the final text for this utterance.
    fn finalize(&mut self) -> String;
    /// Discard utterance state for the next turn.
    fn reset(&mut self);
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn read_text(ptr: *const std::os::raw::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(ptr).to_string_lossy().trim().to_string() }
}

/// Online Zipformer transducer over the sherpa-onnx C API.
pub struct SherpaOnlineZipformer {
    recognizer: *const sherpa_rs_sys::SherpaOnnxOnlineRecognizer,
    stream: *const sherpa_rs_sys::SherpaOnnxOnlineStream,
    // Own the C strings the config points at.
    _keepalive: Vec<CString>,
}

impl SherpaOnlineZipformer {
    pub fn new(
        encoder: &str,
        decoder: &str,
        joiner: &str,
        tokens: &str,
        num_threads: i32,
    ) -> anyhow::Result<Self> {
        let keep = vec![
            cstr(encoder),
            cstr(decoder),
            cstr(joiner),
            cstr(tokens),
            cstr("cpu"),
            cstr("greedy_search"),
            cstr(""),
        ];
        // keep: [encoder, decoder, joiner, tokens, provider, decoding, empty]
        let empty = keep[6].as_ptr();
        let transducer = sherpa_rs_sys::SherpaOnnxOnlineTransducerModelConfig {
            encoder: keep[0].as_ptr(),
            decoder: keep[1].as_ptr(),
            joiner: keep[2].as_ptr(),
        };
        let model_config = unsafe {
            sherpa_rs_sys::SherpaOnnxOnlineModelConfig {
                transducer,
                paraformer: mem::zeroed(),
                zipformer2_ctc: mem::zeroed(),
                tokens: keep[3].as_ptr(),
                num_threads,
                provider: keep[4].as_ptr(),
                debug: 0,
                model_type: empty,
                modeling_unit: empty,
                bpe_vocab: empty,
                tokens_buf: empty,
                tokens_buf_size: 0,
                nemo_ctc: mem::zeroed(),
            }
        };
        let recognizer_config = sherpa_rs_sys::SherpaOnnxOnlineRecognizerConfig {
            feat_config: sherpa_rs_sys::SherpaOnnxFeatureConfig {
                sample_rate: 16000,
                feature_dim: 80,
            },
            model_config,
            decoding_method: keep[5].as_ptr(),
            max_active_paths: 4,
            // Endpointing belongs to the VAD layer, not the decoder.
            enable_endpoint: 0,
            rule1_min_trailing_silence: 0.0,
            rule2_min_trailing_silence: 0.0,
            rule3_min_utterance_length: 0.0,
            hotwords_file: empty,
            hotwords_score: 1.5,
            ctc_fst_decoder_config: unsafe { mem::zeroed() },
            rule_fsts: empty,
            rule_fars: empty,
            blank_penalty: 0.0,
            hotwords_buf: empty,
            hotwords_buf_size: 0,
            hr: unsafe { mem::zeroed() },
        };
        let recognizer =
            unsafe { sherpa_rs_sys::SherpaOnnxCreateOnlineRecognizer(&recognizer_config) };
        if recognizer.is_null() {
            anyhow::bail!("SherpaOnnxCreateOnlineRecognizer failed (check model files).");
        }
        let stream = unsafe { sherpa_rs_sys::SherpaOnnxCreateOnlineStream(recognizer) };
        if stream.is_null() {
            unsafe { sherpa_rs_sys::SherpaOnnxDestroyOnlineRecognizer(recognizer) };
            anyhow::bail!("SherpaOnnxCreateOnlineStream failed.");
        }
        Ok(Self { recognizer, stream, _keepalive: keep })
    }

    fn result_text(&mut self) -> String {
        unsafe {
            let result =
                sherpa_rs_sys::SherpaOnnxGetOnlineStreamResult(self.recognizer, self.stream);
            if result.is_null() {
                return String::new();
            }
            let text = read_text((*result).text);
            sherpa_rs_sys::SherpaOnnxDestroyOnlineRecognizerResult(result);
            text
        }
    }

    fn decode_ready(&mut self) {
        unsafe {
            while sherpa_rs_sys::SherpaOnnxIsOnlineStreamReady(self.recognizer, self.stream) == 1 {
                sherpa_rs_sys::SherpaOnnxDecodeOnlineStream(self.recognizer, self.stream);
            }
        }
    }
}

impl StreamRecognizer for SherpaOnlineZipformer {
    fn accept(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        unsafe {
            sherpa_rs_sys::SherpaOnnxOnlineStreamAcceptWaveform(
                self.stream,
                16000,
                samples.as_ptr(),
                samples.len() as i32,
            );
        }
    }

    fn decode(&mut self) {
        self.decode_ready();
    }

    fn partial_text(&mut self) -> String {
        self.result_text()
    }

    fn finalize(&mut self) -> String {
        unsafe {
            sherpa_rs_sys::SherpaOnnxOnlineStreamInputFinished(self.stream);
        }
        self.decode();
        let text = self.result_text();
        self.reset();
        text
    }

    fn reset(&mut self) {
        unsafe {
            sherpa_rs_sys::SherpaOnnxDestroyOnlineStream(self.stream);
            self.stream = sherpa_rs_sys::SherpaOnnxCreateOnlineStream(self.recognizer);
        }
    }
}

// The C objects are exclusively owned here; never shared across threads.
unsafe impl Send for SherpaOnlineZipformer {}

impl Drop for SherpaOnlineZipformer {
    fn drop(&mut self) {
        unsafe {
            if !self.stream.is_null() {
                sherpa_rs_sys::SherpaOnnxDestroyOnlineStream(self.stream);
                self.stream = std::ptr::null();
            }
            if !self.recognizer.is_null() {
                sherpa_rs_sys::SherpaOnnxDestroyOnlineRecognizer(self.recognizer);
                self.recognizer = std::ptr::null();
            }
        }
    }
}

/// Scripted recognizer for manager/state tests (no models).
pub struct FakeRecognizer {
    partials: Vec<String>,
    pos: usize,
    pub accepts: usize,
    pub decodes: usize,
    pub resets: usize,
}

impl FakeRecognizer {
    pub fn new(partials: Vec<String>) -> Self {
        Self { partials, pos: 0, accepts: 0, decodes: 0, resets: 0 }
    }
}

impl StreamRecognizer for FakeRecognizer {
    fn accept(&mut self, samples: &[f32]) {
        self.accepts += samples.len();
    }

    fn decode(&mut self) {
        self.decodes += 1;
        if self.pos < self.partials.len().saturating_sub(1) {
            self.pos += 1;
        }
    }

    fn partial_text(&mut self) -> String {
        self.partials.get(self.pos).cloned().unwrap_or_default()
    }

    fn finalize(&mut self) -> String {
        let text = self.partials.last().cloned().unwrap_or_default();
        self.reset();
        text
    }

    fn reset(&mut self) {
        self.resets += 1;
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_recognizer_replays_partials() {
        let mut r = FakeRecognizer::new(vec!["open".into(), "open my".into(), "open my project".into()]);
        r.accept(&[0.0; 512]);
        r.decode();
        assert_eq!(r.partial_text(), "open my");
        r.decode();
        assert_eq!(r.partial_text(), "open my project");
        assert_eq!(r.finalize(), "open my project");
        assert_eq!(r.resets, 1);
    }
}
