//! Low-latency browser frames use a private Tauri channel, not global events.
//! Chromium's ack limits unpainted frames; hiding the pane stops its producer.
use comrade_core::tools::browser_driver;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};
use tauri::ipc::Channel;
use tokio::sync::Mutex;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Frame {
        stream_id: u64,
        #[serde(flatten)]
        frame: browser_driver::StreamFrame,
    },
    Error {
        stream_id: u64,
        message: String,
    },
}
struct ActiveStream {
    id: u64,
    ws_url: String,
    worker: tokio::task::JoinHandle<()>,
}
static STREAM: OnceLock<Mutex<Option<ActiveStream>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn stream_slot() -> &'static Mutex<Option<ActiveStream>> {
    STREAM.get_or_init(|| Mutex::new(None))
}

async fn stop_active(stream: ActiveStream) {
    stream.worker.abort();
    // Serialize stop/start so an old worker can never stop a newly opened stream.
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        browser_driver::stop_screencast(&stream.ws_url),
    )
    .await;
}

#[tauri::command]
pub async fn browser_stream_start(
    width: u32,
    height: u32,
    on_frame: Channel<StreamEvent>,
) -> Result<u64, String> {
    let mut slot = stream_slot().lock().await;
    if let Some(previous) = slot.take() {
        stop_active(previous).await;
    }
    let mut cast = browser_driver::start_screencast(width, height).await?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let ws_url = cast.ws_url.clone();
    let worker = tokio::spawn(async move {
        loop {
            match cast.next_frame().await {
                Ok(frame) => {
                    if on_frame
                        .send(StreamEvent::Frame {
                            stream_id: id,
                            frame,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(message) => {
                    let _ = on_frame.send(StreamEvent::Error {
                        stream_id: id,
                        message,
                    });
                    break;
                }
            }
        }
        // Also stop if the frontend channel vanished (closed window / reload).
        let _ = browser_driver::stop_screencast(&cast.ws_url).await;
    });
    *slot = Some(ActiveStream { id, ws_url, worker });
    Ok(id)
}

#[tauri::command]
pub async fn browser_stream_stop(stream_id: u64) -> Result<(), String> {
    let mut slot = stream_slot().lock().await;
    if slot.as_ref().is_some_and(|s| s.id == stream_id) {
        if let Some(stream) = slot.take() {
            stop_active(stream).await;
        }
    }
    Ok(())
}
async fn stream_url(id: u64) -> Option<String> {
    stream_slot()
        .lock()
        .await
        .as_ref()
        .filter(|s| s.id == id)
        .map(|s| s.ws_url.clone())
}
#[tauri::command]
pub async fn browser_stream_ack(stream_id: u64, session_id: i64) -> Result<(), String> {
    if let Some(url) = stream_url(stream_id).await {
        browser_driver::acknowledge_frame(&url, session_id).await?;
    }
    Ok(())
}
#[tauri::command]
pub async fn browser_stream_resize(stream_id: u64, width: u32, height: u32) -> Result<(), String> {
    if let Some(url) = stream_url(stream_id).await {
        browser_driver::resize_viewport(&url, width, height).await?;
    }
    Ok(())
}
