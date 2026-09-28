/**
 * Sequential low-latency playback over cpal. Chunks queue with a generation
 * id; interruption bumps the generation and drains the queue instantly.
 * Underruns are counted for metrics.
 */
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use cpal::traits::{DeviceTrait, StreamTrait};

struct QueueState {
    queue: VecDeque<f32>,
    /// Total samples consumed by the callback (all generations).
    played: u64,
    /// Samples queued per generation (for drain detection).
    queued_by_gen: std::collections::HashMap<u64, u64>,
    played_by_gen: std::collections::HashMap<u64, u64>,
    underruns: u64,
    active: bool,
}

pub struct PlaybackStats {
    pub underruns: u64,
    pub queued: u64,
    pub played: u64,
}

#[derive(Clone)]
pub struct PlaybackHandle {
    state: Arc<Mutex<QueueState>>,
    generation: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
}

impl PlaybackHandle {
    /// Queue samples under the current generation. Returns queued count.
    pub fn push(&self, samples: &[f32]) -> usize {
        let gen = self.generation.load(Ordering::SeqCst);
        if let Ok(mut s) = self.state.lock() {
            if !s.active {
                return 0;
            }
            s.queue.extend(samples.iter().copied());
            *s.queued_by_gen.entry(gen).or_insert(0) += samples.len() as u64;
            samples.len()
        } else {
            0
        }
    }

    /// Generation of subsequently pushed audio.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Stop everything now: drops queued audio, invalidates in-flight chunks.
    /// Returns the new generation id.
    pub fn interrupt(&self) -> u64 {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut s) = self.state.lock() {
            s.queue.clear();
            s.queued_by_gen.retain(|&g, _| g >= gen);
            s.played_by_gen.retain(|&g, _| g >= gen);
        }
        gen
    }

    /// True when every sample queued under `gen` has played out.
    pub fn drained(&self, gen: u64) -> bool {
        if let Ok(s) = self.state.lock() {
            let queued = s.queued_by_gen.get(&gen).copied().unwrap_or(0);
            let played = s.played_by_gen.get(&gen).copied().unwrap_or(0);
            queued > 0 && played >= queued
        } else {
            true
        }
    }

    pub fn stats(&self) -> PlaybackStats {
        if let Ok(s) = self.state.lock() {
            PlaybackStats {
                underruns: s.underruns,
                queued: s.queued_by_gen.values().sum(),
                played: s.played,
            }
        } else {
            PlaybackStats { underruns: 0, queued: 0, played: 0 }
        }
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Ok(mut s) = self.state.lock() {
            s.active = false;
            s.queue.clear();
        }
    }
}

/// Output device opened at `sample_rate` mono f32 when possible, else the
/// device default with resampling in the callback.
pub struct Player {
    _stream: cpal::Stream,
    handle: PlaybackHandle,
    pub device_rate: u32,
    pub device_channels: u16,
}

impl Player {
    pub fn open(device: &cpal::Device, sample_rate: u32) -> anyhow::Result<Self> {
        let mut supported: Vec<cpal::SupportedStreamConfigRange> = device
            .supported_output_configs()
            .map_err(|e| anyhow::anyhow!("no output configs: {e}"))?
            .collect();
        // Prefer exact match, else default (resampled in callback).
        let exact = supported.iter().find(|c| {
            c.channels() == 1 && c.sample_format() == cpal::SampleFormat::F32
                && c.min_sample_rate().0 <= sample_rate
                && sample_rate <= c.max_sample_rate().0
        });
        let (config, _) = match exact {
            Some(range) => (
                range.with_sample_rate(cpal::SampleRate(sample_rate)),
                false,
            ),
            None => {
                let def = device
                    .default_output_config()
                    .map_err(|e| anyhow::anyhow!("no default output: {e}"))?;
                supported.sort_by_key(|c| {
                    (c.channels() != def.channels()) as u8 * 100
                        + (c.sample_format() != cpal::SampleFormat::F32) as u8
                });
                (def, true)
            }
        };
        let device_rate = config.sample_rate().0;
        let device_channels = config.channels();

        let state = Arc::new(Mutex::new(QueueState {
            queue: VecDeque::new(),
            played: 0,
            queued_by_gen: Default::default(),
            played_by_gen: Default::default(),
            underruns: 0,
            active: true,
        }));
        let handle = PlaybackHandle {
            state: state.clone(),
            generation: Arc::new(AtomicU64::new(1)),
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        let state_cb = state.clone();
        let shutdown_cb = handle.shutdown.clone();
        let stream = device
            .build_output_stream(
                &config.into(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if shutdown_cb.load(Ordering::SeqCst) {
                        data.fill(0.0);
                        return;
                    }
                    let Ok(mut s) = state_cb.lock() else {
                        data.fill(0.0);
                        return;
                    };
                    if !s.active {
                        data.fill(0.0);
                        return;
                    }
                    // Queue content is device-rate mono (see push_resampled).
                    let ch = device_channels.max(1) as usize;
                    let mut starved = false;
                    for frame in data.chunks_exact_mut(ch) {
                        match s.queue.pop_front() {
                            Some(v) => {
                                s.played += 1;
                                // Attribute to the oldest unfinished generation.
                                let mut gens: Vec<u64> =
                                    s.queued_by_gen.keys().copied().collect();
                                gens.sort_unstable();
                                for g in gens {
                                    let q = s.queued_by_gen.get(&g).copied().unwrap_or(0);
                                    let p = s.played_by_gen.entry(g).or_insert(0);
                                    if *p < q {
                                        *p += 1;
                                        break;
                                    }
                                }
                                for slot in frame.iter_mut() {
                                    *slot = v;
                                }
                            }
                            None => {
                                starved = true;
                                for slot in frame.iter_mut() {
                                    *slot = 0.0;
                                }
                            }
                        }
                    }
                    if starved {
                        let queued: u64 = s.queued_by_gen.values().sum();
                        let played: u64 = s.played_by_gen.values().sum();
                        if played < queued {
                            s.underruns += 1;
                        }
                    }
                },
                |err| eprintln!("Comrade playback error: {err}"),
                None,
            )
            .map_err(|e| anyhow::anyhow!("output stream failed: {e}"))?;
        stream.play().map_err(|e| anyhow::anyhow!("output play failed: {e}"))?;
        // NOTE: queue content is expected at device_rate already; the manager
        // NOTE: queue content is expected at device_rate already; the manager
        // resamples TTS audio (24 kHz) to device_rate on push. See push_resampled.
        Ok(Self { _stream: stream, handle, device_rate, device_channels })
    }

    pub fn handle(&self) -> PlaybackHandle {
        self.handle.clone()
    }

    /// Push model-rate mono audio, resampling to the device rate first.
    pub fn push_resampled(&self, samples: &[f32], from_rate: u32) -> usize {
        let mono_16k_device = crate::voice::audio::resample(samples, from_rate, self.device_rate);
        self.handle.push(&mono_16k_device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_handle() -> PlaybackHandle {
        PlaybackHandle {
            state: Arc::new(Mutex::new(QueueState {
                queue: VecDeque::new(),
                played: 0,
                queued_by_gen: Default::default(),
                played_by_gen: Default::default(),
                underruns: 0,
                active: true,
            })),
            generation: Arc::new(AtomicU64::new(1)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn interrupt_invalidates_queued_audio() {
        let h = test_handle();
        assert_eq!(h.push(&[0.1, 0.2, 0.3]), 3);
        assert!(!h.drained(1));
        let gen2 = h.interrupt();
        assert_eq!(gen2, 2);
        // Old generation can never "drain" (dropped); new pushes work.
        assert_eq!(h.push(&[0.4]), 1);
        assert_eq!(h.generation(), 2);
    }

    #[test]
    fn push_after_shutdown_drops() {
        let h = test_handle();
        h.shutdown();
        assert_eq!(h.push(&[0.1]), 0);
    }
}
