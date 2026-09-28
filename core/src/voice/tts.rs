/**
 * Kokoro-82M TTS through sherpa-onnx. Sentence-level synthesis; the manager
 * streams chunks so speech starts before the LLM finishes.
 */
pub struct TtsPcm {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

pub trait SpeechSynth: Send {
    fn synthesize(&mut self, text: &str) -> anyhow::Result<TtsPcm>;
    fn sample_rate(&self) -> u32;
}

pub struct KokoroFiles {
    pub model: String,
    pub voices: String,
    pub tokens: String,
    pub data_dir: String,
    pub dict_dir: String,
    pub lexicon: String,
}

/// Locate Kokoro assets inside an extracted model dir (names vary by release).
pub fn find_kokoro_files(dir: &std::path::Path) -> anyhow::Result<KokoroFiles> {
    let pick = |pred: &dyn Fn(&str) -> bool| -> Option<String> {
        std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .find(|n| pred(&n.to_lowercase()))
            .map(|n| dir.join(n).to_string_lossy().to_string())
    };
    // Prefer the normalized model.onnx; fall back to any .onnx present.
    let model = pick(&|n| n == "model.onnx")
        .or_else(|| pick(&|n| n.ends_with(".onnx")))
        .ok_or_else(|| anyhow::anyhow!("kokoro model.onnx missing"))?;
    let voices = pick(&|n| n == "voices.bin" || (n.ends_with(".bin") && n.contains("voice")))
        .ok_or_else(|| anyhow::anyhow!("kokoro voices.bin missing"))?;
    let tokens = pick(&|n| n == "tokens.txt" || (n.ends_with(".txt") && n.contains("token")))
        .unwrap_or_default();
    let data_dir = pick(&|n| n == "espeak-ng-data")
        .or_else(|| {
            std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
                let p = e.path().join("espeak-ng-data");
                p.is_dir().then(|| p.to_string_lossy().to_string())
            })
        })
        .unwrap_or_default();
    let dict_dir = pick(&|n| n == "dict").unwrap_or_default();
    let lexicon = pick(&|n| n.starts_with("lexicon") && n.ends_with(".txt")).unwrap_or_default();
    Ok(KokoroFiles { model, voices, tokens, data_dir, dict_dir, lexicon })
}

pub struct SherpaKokoro {
    tts: sherpa_rs::tts::KokoroTts,
    sid: i32,
    speed: f32,
    sample_rate: u32,
}

impl SherpaKokoro {
    pub fn new(files: &KokoroFiles, sid: i32, speed: f32, num_threads: i32) -> anyhow::Result<Self> {
        let mut tts = sherpa_rs::tts::KokoroTts::new(sherpa_rs::tts::KokoroTtsConfig {
            model: files.model.clone(),
            voices: files.voices.clone(),
            tokens: files.tokens.clone(),
            data_dir: files.data_dir.clone(),
            dict_dir: files.dict_dir.clone(),
            lexicon: files.lexicon.clone(),
            length_scale: 1.0,
            lang: "en".to_string(),
            ..Default::default()
        });
        // Probe once so a bad model dir fails fast with audio params known.
        let probe = tts
            .create("ok", sid, speed)
            .map_err(|e| anyhow::anyhow!("Kokoro probe failed: {e}"))?;
        let sample_rate = probe.sample_rate;
        let _ = num_threads;
        Ok(Self { tts, sid, speed, sample_rate })
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.25, 4.0);
    }
}

impl SpeechSynth for SherpaKokoro {
    fn synthesize(&mut self, text: &str) -> anyhow::Result<TtsPcm> {
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("nothing to synthesize");
        }
        let audio = self
            .tts
            .create(text, self.sid, self.speed)
            .map_err(|e| anyhow::anyhow!("Kokoro synth failed: {e}"))?;
        if audio.samples.is_empty() {
            anyhow::bail!("Kokoro returned no samples");
        }
        Ok(TtsPcm { samples: audio.samples, sample_rate: audio.sample_rate })
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Scripted synth for tests: 0.1 s of tone per character (capped).
pub struct FakeSynth {
    pub calls: Vec<String>,
    pub sample_rate: u32,
}

impl FakeSynth {
    pub fn new() -> Self {
        Self { calls: vec![], sample_rate: 24000 }
    }
}

impl Default for FakeSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeechSynth for FakeSynth {
    fn synthesize(&mut self, text: &str) -> anyhow::Result<TtsPcm> {
        self.calls.push(text.to_string());
        let n = (text.chars().count().min(50) as f32 * 0.1 * self.sample_rate as f32) as usize;
        let samples: Vec<f32> = (0..n).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        Ok(TtsPcm { samples, sample_rate: self.sample_rate })
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn finder_maps_release_layout() {
        let dir = std::env::temp_dir().join("comrade-kokoro-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kokoro-v1_1.onnx"), "x").unwrap();
        std::fs::write(dir.join("voices.bin"), "x").unwrap();
        std::fs::write(dir.join("tokens.txt"), "x").unwrap();
        let files = find_kokoro_files(&dir).unwrap();
        assert!(files.model.ends_with("kokoro-v1_1.onnx"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = PathBuf::from("x");
    }

    #[test]
    fn fake_synth_produces_audio() {
        let mut s = FakeSynth::new();
        let pcm = s.synthesize("Hello world").unwrap();
        assert_eq!(pcm.sample_rate, 24000);
        assert!(!pcm.samples.is_empty());
        assert_eq!(s.calls, vec!["Hello world".to_string()]);
    }
}
