/** Sample math: conversion, mono mixing, linear resampling. Pure + tested. */
pub const STT_SAMPLE_RATE: u32 = 16000;
/// 32 ms frames at 16 kHz — matches the Silero VAD window.
pub const FRAME_SAMPLES: usize = 512;

pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|s| *s as f32 / 32768.0).collect()
}

pub fn u16_to_f32(samples: &[u16]) -> Vec<f32> {
    samples.iter().map(|s| (*s as f32 - 32768.0) / 32768.0).collect()
}

/// Interleaved multi-channel → mono by averaging.
pub fn mix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Linear resampler. Transparent for speech STT/playback fallback paths;
/// exact when rates match.
pub fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == to_rate || from_rate == 0 {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len.max(1));
    for i in 0..out_len {
        let pos = i as f64 / ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = input[idx.min(input.len() - 1)];
        let b = input[(idx + 1).min(input.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

/// Microphone chunk (any rate/channels) → mono 16 kHz frames for the pipeline.
pub fn to_pipeline_frames(interleaved: &[f32], from_rate: u32, channels: usize) -> Vec<f32> {
    let mono = mix_to_mono(interleaved, channels.max(1));
    resample(&mono, from_rate, STT_SAMPLE_RATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_identity_and_lengths() {
        let input: Vec<f32> = (0..1600).map(|i| (i as f32 / 1600.0).sin()).collect();
        assert_eq!(resample(&input, 16000, 16000).len(), 1600);
        assert_eq!(resample(&input, 8000, 16000).len(), 3200);
        assert_eq!(resample(&input, 48000, 16000).len(), 533);
        assert!(resample(&[], 8000, 16000).is_empty());
    }

    #[test]
    fn resample_preserves_dc_and_peak() {
        let dc = vec![0.5f32; 100];
        let up = resample(&dc, 8000, 16000);
        assert!(up.iter().all(|v| (v - 0.5).abs() < 1e-5));
        let peak = vec![0.0f32, 1.0, 0.0, -1.0];
        let up = resample(&peak, 2, 4);
        assert_eq!(up.len(), 8);
        assert!(up.iter().all(|v| v.abs() <= 1.0 + 1e-5));
    }

    #[test]
    fn mono_mix_averages_channels() {
        assert_eq!(mix_to_mono(&[1.0, 3.0, 2.0, 4.0], 2), vec![2.0, 3.0]);
        assert_eq!(mix_to_mono(&[1.0, 2.0], 1), vec![1.0, 2.0]);
    }

    #[test]
    fn i16_scales_to_unit_range() {
        let out = i16_to_f32(&[0, 32767, -32768]);
        assert!((out[0]).abs() < 1e-9);
        assert!((out[1] - 0.99997).abs() < 1e-4);
        assert!((out[2] + 1.0).abs() < 1e-9);
    }
}
