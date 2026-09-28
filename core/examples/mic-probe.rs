/**
 * Mic probe: hear what Comrade hears.
 *
 *   mic-probe list                  input devices
 *   mic-probe check [device]        3 s level check (RMS/max/verdict)
 *   mic-probe record [device] [s] out.wav   save 16 kHz mono WAV for playback
 *
 * Uses the real capture path (core::voice::capture), so results transfer 1:1.
 */
use comrade_core::voice::capture::{list_input_devices, MicCapture};

fn usage() -> ! {
    eprintln!("usage: mic-probe list | check [device] | record [device] [secs] out.wav");
    std::process::exit(2);
}

fn verdict(max_rms: f32) -> &'static str {
    if max_rms < 0.005 {
        "SILENT (mic muted, wrong device, or no signal)"
    } else if max_rms < 0.03 {
        "QUIET (works but low gain — raise mic volume)"
    } else if max_rms > 0.9 {
        "LOUD (clipping risk — lower mic volume)"
    } else {
        "OK"
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("list") => {
            for d in list_input_devices() {
                println!("{}{}", d.name, if d.is_default { "  [default]" } else { "" });
            }
        }
        Some("check") => {
            let device = args.next().unwrap_or_default();
            run_check(&device, None).await;
        }
        Some("record") => {
            // record [device] [secs] out.wav — device/secs optional.
            let rest: Vec<String> = args.collect();
            let (device, secs, out) = match rest.len() {
                1 => (String::new(), 3u64, rest[0].clone()),
                2 => {
                    if let Ok(s) = rest[0].parse::<u64>() {
                        (String::new(), s, rest[1].clone())
                    } else {
                        (rest[0].clone(), 3, rest[1].clone())
                    }
                }
                3 => (rest[0].clone(), rest[1].parse().unwrap_or(3), rest[2].clone()),
                _ => usage(),
            };
            run_check(&device, Some((secs, out))).await;
        }
        _ => usage(),
    }
}

async fn run_check(device: &str, save: Option<(u64, String)>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<f32>>(256);
    let label = if device.is_empty() { "(default)" } else { device };
    let cap = match MicCapture::start(device, tx) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("capture failed on {label}: {e}");
            std::process::exit(1);
        }
    };
    let secs = save.as_ref().map(|(s, _)| *s).unwrap_or(3);
    println!("recording {secs}s from {label} — speak now...");
    let t0 = std::time::Instant::now();
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut n = 0u64;
    let mut hot_frames = 0u64;
    let mut frames = 0u64;
    let mut saved: Vec<f32> = Vec::new();
    while t0.elapsed().as_secs() < secs {
        match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await {
            Ok(Some(frame)) => {
                frames += 1;
                for v in &frame {
                    let a = v.abs();
                    if a > peak {
                        peak = a;
                    }
                    sum_sq += (a * a) as f64;
                    n += 1;
                }
                let rms = (frame.iter().map(|v| v * v).sum::<f32>() / frame.len() as f32).sqrt();
                if rms > 0.02 {
                    hot_frames += 1;
                }
                if save.is_some() {
                    saved.extend_from_slice(&frame);
                }
            }
            _ => break,
        }
    }
    drop(cap);
    let rms_all = (sum_sq / n.max(1) as f64).sqrt();
    println!("frames: {frames}, peak RMS(sample): {peak:.4}, overall RMS: {rms_all:.4}");
    println!(
        "frames above -34 dB: {hot_frames}/{frames} ({:.0}%)",
        100.0 * hot_frames as f32 / frames.max(1) as f32
    );
    println!("verdict: {}", verdict(peak));
    if let Some((_, out)) = save {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&out, spec).expect("wav create");
        for v in saved {
            let _ = w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        w.finalize().expect("wav finalize");
        println!("saved {out} — play it back to hear what Comrade hears");
    }
}
