/**
 * Offline voice integration: real local models, prerecorded WAV, no mic.
 * Needs models first: Settings → Download (or ensure_models).
 * Run: `cargo test -p comrade-core --test voice_local -- --ignored --nocapture`
 */
use std::path::PathBuf;

fn models_base() -> PathBuf {
    comrade_core::voice::models::models_dir()
}

fn require_models() -> PathBuf {
    let base = models_base();
    assert!(
        comrade_core::voice::models::all_ready(&base),
        "voice models missing at {} — download first (Settings → Download)",
        base.display()
    );
    base
}

fn read_test_wav(base: &std::path::Path) -> (Vec<f32>, u32) {
    let path = base.join("stt/test/0.wav");
    let mut reader = hound::WavReader::open(&path)
        .unwrap_or_else(|_| panic!("test wav missing: {}", path.display()));
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            reader
                .samples::<i32>()
                .map(|s| {
                    let v = s.unwrap_or(0) as f32;
                    v / (1u64 << (bits - 1)) as f32
                })
                .collect()
        }
    };
    // Mix down if needed.
    let mono = if spec.channels > 1 {
        comrade_core::voice::audio::mix_to_mono(&samples, spec.channels as usize)
    } else {
        samples
    };
    let pcm = comrade_core::voice::audio::resample(&mono, spec.sample_rate, 16000);
    (pcm, 16000)
}

#[tokio::test]
#[ignore]
async fn local_streaming_stt_transcribes_test_wav() {
    use comrade_core::voice::stt::{SherpaOnlineZipformer, StreamRecognizer};
    let base = require_models();
    let stt_dir = base.join("stt");
    let j = |n: &str| stt_dir.join(n).to_string_lossy().to_string();
    let t0 = std::time::Instant::now();
    let mut rec = SherpaOnlineZipformer::new(
        &j("encoder-epoch-99-avg-1.int8.onnx"),
        &j("decoder-epoch-99-avg-1.int8.onnx"),
        &j("joiner-epoch-99-avg-1.int8.onnx"),
        &j("tokens.txt"),
        2,
    )
    .expect("stt init failed");
    println!("stt init: {}ms", t0.elapsed().as_millis());
    let (pcm, _) = read_test_wav(&base);
    // Stream in ~0.5 s chunks with periodic decodes, like the live pipeline.
    let mut partials = Vec::new();
    for chunk in pcm.chunks(8000) {
        rec.accept(chunk);
        rec.decode();
        let p = rec.partial_text();
        if !p.is_empty() && partials.last() != Some(&p) {
            println!("partial: {p}");
            partials.push(p);
        }
    }
    assert!(!partials.is_empty(), "no partials streamed (not genuinely streaming)");
    let final_text = rec.finalize();
    println!("final: {final_text}");
    assert!(
        final_text.to_lowercase().contains("lamps"),
        "unexpected transcript: {final_text}"
    );
}

#[tokio::test]
#[ignore]
async fn local_vad_flags_speech_in_test_wav() {
    use comrade_core::voice::vad::{SherpaSileroVad, VadEngine};
    let base = require_models();
    let vad_path = base.join("vad/silero_vad.onnx").to_string_lossy().to_string();
    let mut vad = SherpaSileroVad::new(&vad_path, 0.5, 700.0, 250.0, 1).expect("vad init failed");
    let (pcm, _) = read_test_wav(&base);
    let mut speech_frames = 0usize;
    let mut total = 0usize;
    let frame_samples = comrade_core::voice::audio::FRAME_SAMPLES;
    for frame in pcm.chunks_exact(frame_samples) {
        total += 1;
        if vad.is_speech(frame) {
            speech_frames += 1;
        }
    }
    println!("speech frames: {speech_frames}/{total}");
    assert!(speech_frames > total / 10, "VAD heard almost nothing");
}

#[tokio::test]
#[ignore]
async fn local_kokoro_synthesizes() {
    use comrade_core::voice::tts::{find_kokoro_files, SherpaKokoro, SpeechSynth};
    let base = require_models();
    let files = find_kokoro_files(&base.join("tts")).expect("kokoro assets missing");
    println!("kokoro model: {}", files.model);
    let t0 = std::time::Instant::now();
    let mut tts = SherpaKokoro::new(&files, 0, 1.0, 1).expect("tts init failed");
    println!("tts init: {}ms", t0.elapsed().as_millis());
    let t1 = std::time::Instant::now();
    let pcm = tts.synthesize("Hello Comrade. Voice check complete.").expect("synth failed");
    let secs = pcm.samples.len() as f32 / pcm.sample_rate as f32;
    println!("synth: {:.2}s audio in {}ms (rate {})", secs, t1.elapsed().as_millis(), pcm.sample_rate);
    assert_eq!(pcm.sample_rate, 24000);
    assert!(secs > 1.0, "suspiciously short audio");
    assert!(pcm.samples.iter().any(|&s| s.abs() > 0.01), "all silence");
}
