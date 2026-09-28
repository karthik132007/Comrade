/**
 * Sequential low-latency playback over cpal. Chunks queue with a generation
 * id; interruption bumps the generation and drains the queue instantly.
 * Underruns are counted for metrics.
 */
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

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
            PlaybackStats {
                underruns: 0,
                queued: 0,
                played: 0,
            }
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
    handle: PlaybackHandle,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    pub device_rate: u32,
    pub device_channels: u16,
}

impl Player {
    pub fn open_default(sample_rate: u32) -> anyhow::Result<Self> {
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
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let state_cb = state.clone();
        let shutdown_cb = handle.shutdown.clone();
        let thread = std::thread::Builder::new()
            .name("comrade-playback".into())
            .spawn(
                move || match open_output_stream(sample_rate, state_cb, shutdown_cb) {
                    Ok((stream, device_rate, device_channels)) => {
                        if ready_tx.send(Ok((device_rate, device_channels))).is_ok() {
                            let _ = stop_rx.recv();
                        }
                        drop(stream);
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                    }
                },
            )
            .map_err(|e| anyhow::anyhow!("playback thread failed: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok((device_rate, device_channels))) => Ok(Self {
                handle,
                stop: Some(stop_tx),
                thread: Some(thread),
                device_rate,
                device_channels,
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                anyhow::bail!(error)
            }
            Err(error) => {
                let _ = thread.join();
                anyhow::bail!("playback thread stopped during startup: {error}")
            }
        }
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

impl Drop for Player {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open_output_stream(
    sample_rate: u32,
    state: Arc<Mutex<QueueState>>,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<(cpal::Stream, u32, u16)> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("playback_unavailable: no default speaker"))?;
    let supported: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_output_configs()
        .map_err(|e| anyhow::anyhow!("no output configs: {e}"))?
        .collect();
    let exact = supported.iter().find(|config| {
        config.channels() == 1
            && config.sample_format() == cpal::SampleFormat::F32
            && config.min_sample_rate().0 <= sample_rate
            && sample_rate <= config.max_sample_rate().0
    });
    let config = match exact {
        Some(range) => range.with_sample_rate(cpal::SampleRate(sample_rate)),
        None => device
            .default_output_config()
            .map_err(|e| anyhow::anyhow!("no default output: {e}"))?,
    };
    let sample_format = config.sample_format();
    let device_rate = config.sample_rate().0;
    let device_channels = config.channels();
    let stream_config = config.into();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            build_output_stream::<f32>(&device, &stream_config, state, shutdown, device_channels)
        }
        cpal::SampleFormat::I16 => {
            build_output_stream::<i16>(&device, &stream_config, state, shutdown, device_channels)
        }
        cpal::SampleFormat::U16 => {
            build_output_stream::<u16>(&device, &stream_config, state, shutdown, device_channels)
        }
        format => anyhow::bail!("unsupported output sample format: {format:?}"),
    }?;
    stream
        .play()
        .map_err(|e| anyhow::anyhow!("output play failed: {e}"))?;
    Ok((stream, device_rate, device_channels))
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: Arc<Mutex<QueueState>>,
    shutdown: Arc<AtomicBool>,
    device_channels: u16,
) -> anyhow::Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let silence = T::from_sample(0.0);
                if shutdown.load(Ordering::SeqCst) {
                    data.fill(silence);
                    return;
                }
                let Ok(mut state) = state.lock() else {
                    data.fill(silence);
                    return;
                };
                if !state.active {
                    data.fill(silence);
                    return;
                }
                let channels = device_channels.max(1) as usize;
                let mut starved = false;
                for frame in data.chunks_exact_mut(channels) {
                    match state.queue.pop_front() {
                        Some(value) => {
                            state.played += 1;
                            let mut generations: Vec<u64> =
                                state.queued_by_gen.keys().copied().collect();
                            generations.sort_unstable();
                            for generation in generations {
                                let queued =
                                    state.queued_by_gen.get(&generation).copied().unwrap_or(0);
                                let played = state.played_by_gen.entry(generation).or_insert(0);
                                if *played < queued {
                                    *played += 1;
                                    break;
                                }
                            }
                            frame.fill(T::from_sample(value));
                        }
                        None => {
                            starved = true;
                            frame.fill(silence);
                        }
                    }
                }
                if starved {
                    let queued: u64 = state.queued_by_gen.values().sum();
                    let played: u64 = state.played_by_gen.values().sum();
                    if played < queued {
                        state.underruns += 1;
                    }
                }
            },
            |error| eprintln!("Comrade playback error: {error}"),
            None,
        )
        .map_err(|e| anyhow::anyhow!("output stream failed: {e}"))
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
