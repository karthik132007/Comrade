/**
 * Microphone capture over cpal. Requests 16 kHz mono f32; falls back to the
 * device default with conversion + linear resampling. Runs on its own thread,
 * never blocks async executors; non-blocking channel send (drops on overflow).
 */
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::voice::audio::{i16_to_f32, to_pipeline_frames, u16_to_f32, STT_SAMPLE_RATE};

#[derive(Clone, Debug, serde::Serialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

fn host() -> cpal::Host {
    cpal::default_host()
}

pub fn list_input_devices() -> Vec<AudioDeviceInfo> {
    let Ok(devices) = host().input_devices() else {
        return vec![];
    };
    let default_name = host()
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    devices
        .filter_map(|d| d.name().ok())
        .map(|name| AudioDeviceInfo { is_default: name == default_name, name })
        .collect()
}

pub fn list_output_devices() -> Vec<AudioDeviceInfo> {
    let Ok(devices) = host().output_devices() else {
        return vec![];
    };
    let default_name = host()
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    devices
        .filter_map(|d| d.name().ok())
        .map(|name| AudioDeviceInfo { is_default: name == default_name, name })
        .collect()
}

fn find_input_device(want: &str) -> anyhow::Result<cpal::Device> {
    if !want.trim().is_empty() {
        let found = host()
            .input_devices()
            .map_err(|e| anyhow::anyhow!("mic device list failed: {e}"))?
            .find(|d| d.name().as_deref() == Ok(want));
        if let Some(device) = found {
            return Ok(device);
        }
        anyhow::bail!("microphone \"{want}\" not found; using default");
    }
    host()
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default microphone (permission denied or none present)"))
}

pub struct MicCapture {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    dropped_frames: Arc<AtomicU64>,
}

impl MicCapture {
    /// Spawn capture; 16 kHz mono f32 frames flow into `tx` (512 samples each).
    pub fn start(
        device_name: &str,
        tx: tokio::sync::mpsc::Sender<Vec<f32>>,
    ) -> anyhow::Result<Self> {
        let device = find_input_device(device_name);
        let device = match device {
            Ok(d) => d,
            Err(first_err) => host().default_input_device().ok_or(first_err)?,
        };
        // Prefer native 16 kHz mono; else convert from the device default.
        let supported = device.supported_input_configs().map_err(|e| {
            anyhow::anyhow!("mic configs unavailable (permission?): {e}")
        })?;
        let mut use_config: Option<cpal::SupportedStreamConfig> = None;
        let mut fallback: Option<cpal::SupportedStreamConfig> = None;
        for range in supported {
            if range.channels() == 1
                && range.min_sample_rate().0 <= STT_SAMPLE_RATE
                && STT_SAMPLE_RATE <= range.max_sample_rate().0
            {
                let cfg = range.with_sample_rate(cpal::SampleRate(STT_SAMPLE_RATE));
                if range.sample_format() == cpal::SampleFormat::F32 {
                    use_config = Some(cfg);
                    break;
                }
                fallback.get_or_insert(cfg);
            }
            if fallback.is_none() && range.channels() <= 2 {
                fallback.get_or_insert(range.with_max_sample_rate());
            }
        }
        let chosen = use_config.or(fallback).ok_or_else(|| {
            anyhow::anyhow!("microphone offers no usable format")
        })?;
        let from_rate = chosen.sample_rate().0;
        let from_channels = chosen.channels();
        let from_format = chosen.sample_format();

        let stop = Arc::new(AtomicBool::new(false));
        let stop_cb = stop.clone();
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_cb = dropped.clone();

        let dev_name = device.name().unwrap_or_else(|_| "?".into());
        let build = move |config: &cpal::StreamConfig| -> anyhow::Result<cpal::Stream> {
            let mut leftover: Vec<f32> = Vec::new();
            let mut send_frames = move |mono_16k: &[f32]| {
                leftover.extend_from_slice(mono_16k);
                while leftover.len() >= crate::voice::audio::FRAME_SAMPLES {
                    let frame: Vec<f32> =
                        leftover.drain(..crate::voice::audio::FRAME_SAMPLES).collect();
                    if tx.try_send(frame).is_err() {
                        dropped_cb.fetch_add(1, Ordering::Relaxed);
                    }
                }
            };
            let err_fn = move |err| {
                eprintln!("Comrade mic stream error: {err}");
            };
            let stream = match from_format {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    config,
                    move |data: &[f32], _| {
                        send_frames(&to_pipeline_frames(data, from_rate, from_channels as usize));
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    config,
                    move |data: &[i16], _| {
                        let f = i16_to_f32(data);
                        send_frames(&to_pipeline_frames(&f, from_rate, from_channels as usize));
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::U16 => device.build_input_stream(
                    config,
                    move |data: &[u16], _| {
                        let f = u16_to_f32(data);
                        send_frames(&to_pipeline_frames(&f, from_rate, from_channels as usize));
                    },
                    err_fn,
                    None,
                ),
                fmt => anyhow::bail!("unsupported mic sample format: {fmt:?}"),
            }
            .map_err(|e| anyhow::anyhow!("mic stream failed: {e}"))?;
            Ok(stream)
        };

        let stream = build(&chosen.into())?;
        stream.play().map_err(|e| anyhow::anyhow!("mic stream play failed: {e}"))?;
        crate::logger::log(
            crate::logger::Level::Info,
            "VOICE",
            "mic capture started",
            Some(&serde_json::json!({
                "device": dev_name,
                "rate": from_rate,
                "channels": from_channels,
                "format": format!("{from_format:?}"),
            })),
        );
        // Keep the stream alive on a parked thread; cpal pumps callbacks.
        let thread = std::thread::Builder::new()
            .name("comrade-mic".into())
            .spawn(move || {
                let _stream = stream;
                while !stop_cb.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            })
            .map_err(|e| anyhow::anyhow!("mic thread failed: {e}"))?;
        Ok(Self { thread: Some(thread), stop, dropped_frames: dropped })
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames.load(Ordering::Relaxed)
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
