//! One persistent, multiplexed CDP socket per target. A reader task routes
//! replies by id and broadcasts page events without blocking user input.
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;

type Reply = oneshot::Sender<Result<Value, String>>;
struct Request {
    method: String,
    params: Value,
    reply: Reply,
}
pub(super) struct Session {
    requests: mpsc::Sender<Request>,
    events: broadcast::Sender<Arc<Value>>,
    alive: AtomicBool,
}
static SESSIONS: OnceLock<Mutex<HashMap<String, Arc<Session>>>> = OnceLock::new();

pub(super) async fn session(url: &str) -> Result<Arc<Session>, String> {
    let mut sessions = SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .await;
    if let Some(session) = sessions
        .get(url)
        .filter(|s| s.alive.load(Ordering::Acquire))
    {
        return Ok(session.clone());
    }
    sessions.retain(|_, s| s.alive.load(Ordering::Acquire));
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(5),
        // Small input and frame-ack packets must leave immediately. Nagle
        // buffering adds latency even though Chromium is on loopback.
        tokio_tungstenite::connect_async_with_config(url, None, true),
    )
    .await
    .map_err(|_| "CDP_TIMEOUT: connecting to browser".to_string())?
    .map_err(|e| format!("CDP_WS: connect failed: {e}"))?;
    let (requests, mut rx) = mpsc::channel::<Request>(64);
    let (events, _) = broadcast::channel(8);
    let session = Arc::new(Session {
        requests,
        events,
        alive: AtomicBool::new(true),
    });
    sessions.insert(url.to_string(), session.clone());
    let running = session.clone();
    tokio::spawn(async move {
        let (mut sink, mut source) = ws.split();
        let mut pending: HashMap<u64, Reply> = HashMap::new();
        let mut id = 0u64;
        loop {
            tokio::select! {
                request = rx.recv() => {
                    let Some(request) = request else { break };
                    pending.retain(|_, reply| !reply.is_closed());
                    if request.reply.is_closed() { continue; }
                    id += 1;
                    let message = serde_json::json!({"id": id, "method": request.method, "params": request.params});
                    pending.insert(id, request.reply);
                    if sink.send(Message::Text(message.to_string().into())).await.is_err() { break; }
                }
                incoming = source.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
                            if let Some(id) = value["id"].as_u64() {
                                if let Some(reply) = pending.remove(&id) {
                                    let result = match value.get("error") {
                                        Some(error) => Err(format!("CDP_ERROR: {error}")),
                                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                                    };
                                    let _ = reply.send(result);
                                }
                            } else if value.get("method").is_some() {
                                let _ = running.events.send(Arc::new(value));
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            if sink.send(Message::Pong(data)).await.is_err() { break; }
                        }
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {}
                    }
                }
            }
        }
        running.alive.store(false, Ordering::Release);
        for (_, reply) in pending {
            let _ = reply.send(Err("CDP_CLOSED: browser connection ended".into()));
        }
        // Wake stream subscribers even though the cached Session still owns its sender.
        let _ = running.events.send(Arc::new(
            serde_json::json!({"method": "Comrade.disconnected"}),
        ));
    });
    Ok(session)
}
impl Session {
    pub(super) fn subscribe(&self) -> broadcast::Receiver<Arc<Value>> {
        self.events.subscribe()
    }
    pub(super) async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        if !self.alive.load(Ordering::Acquire) {
            return Err("CDP_CLOSED: browser connection ended".into());
        }
        let (reply, response) = oneshot::channel();
        // Bound both queueing and waiting. Never replay a timed-out mutation:
        // its reply may be missing even though Chromium already performed it.
        tokio::time::timeout(Duration::from_secs(20), async {
            self.requests
                .send(Request {
                    method: method.into(),
                    params,
                    reply,
                })
                .await
                .map_err(|_| "CDP_CLOSED: browser connection ended".to_string())?;
            response
                .await
                .map_err(|_| "CDP_CLOSED: browser connection ended".to_string())?
        })
        .await
        .map_err(|_| "CDP_TIMEOUT: no reply from browser within 20s".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn multiplexes_out_of_order_replies_and_events_on_one_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let mut requests = Vec::new();
            for _ in 0..2 {
                let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
                    panic!("expected request")
                };
                requests.push(serde_json::from_str::<Value>(&text).unwrap());
            }
            ws.send(Message::Text(
                serde_json::json!({"method": "Page.frameNavigated", "params": {"url": "test"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            for request in requests.into_iter().rev() {
                ws.send(Message::Text(serde_json::json!({"id": request["id"], "result": {"method": request["method"]}}).to_string().into())).await.unwrap();
            }
            // An unexpected second connection would break multiplexing.
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
            ws.close(None).await.unwrap();
        });
        let session = session(&url).await.unwrap();
        let mut events = session.subscribe();
        let (a, b) = tokio::join!(
            session.call("first", Value::Null),
            session.call("second", Value::Null)
        );
        assert_eq!(a.unwrap()["method"], "first");
        assert_eq!(b.unwrap()["method"], "second");
        assert_eq!(
            events.recv().await.unwrap()["method"],
            "Page.frameNavigated"
        );
        server.await.unwrap();
        assert!(session.call("after-close", Value::Null).await.is_err());
    }
}
