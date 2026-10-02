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
    protected: Mutex<bool>,
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
        tokio_tungstenite::connect_async(url),
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
        protected: Mutex::new(false),
    });
    sessions.insert(url.to_string(), session.clone());
    let running = session.clone();
    tokio::spawn(async move {
        let (mut sink, mut source) = ws.split();
        let mut pending: HashMap<u64, Reply> = HashMap::new();
        let mut id = 0u64;
        let mut frames: HashMap<String, String> = HashMap::new();
        let mut parents: HashMap<String, String> = HashMap::new();
        let mut main_frame = String::new();
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
                            if let Some(tree) = value["result"].get("frameTree") {
                                record_frame_tree(tree, &mut frames, &mut parents, &mut main_frame);
                            }
                            if let Some(id) = value["id"].as_u64() {
                                if let Some(reply) = pending.remove(&id) {
                                    let result = match value.get("error") {
                                        Some(error) => Err(format!("CDP_ERROR: {error}")),
                                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                                    };
                                    let _ = reply.send(result);
                                }
                            } else if value.get("method").is_some() {
                                match value["method"].as_str().unwrap_or("") {
                                    "Page.frameAttached" => {
                                        if let (Some(frame), Some(parent)) = (value["params"]["frameId"].as_str(), value["params"]["parentFrameId"].as_str()) {
                                            parents.insert(frame.into(), parent.into());
                                        }
                                    }
                                    "Page.frameNavigated" => {
                                        let frame = &value["params"]["frame"];
                                        if let (Some(frame_id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str()) {
                                            frames.insert(frame_id.into(), url.into());
                                            if let Some(parent) = frame["parentId"].as_str() { parents.insert(frame_id.into(), parent.into()); }
                                            else { main_frame = frame_id.into(); }
                                        }
                                    }
                                    "Page.frameDetached" => {
                                        if let Some(frame) = value["params"]["frameId"].as_str() { frames.remove(frame); parents.remove(frame); }
                                    }
                                    "Fetch.requestPaused" => {
                                        // Paused requests are handled here, never through the lossy
                                        // screencast broadcast queue. Every request is resolved once.
                                        let params = &value["params"];
                                        let frame = params["frameId"].as_str().unwrap_or("");
                                        let url = params["request"]["url"].as_str().unwrap_or("");
                                        let resource = params["resourceType"].as_str().unwrap_or("Other");
                                        let parent = parents.get(frame);
                                        let source = if resource == "Document" {
                                            parent.and_then(|p| frames.get(p))
                                        } else { frames.get(frame) }.or_else(|| frames.get(&main_frame)).map(String::as_str).unwrap_or("");
                                        let kind = request_kind(resource, parent.is_some());
                                        let blocked = super::adblock::current().is_some_and(|shield| shield.blocks(
                                            url, source, kind, params["request"]["method"].as_str().unwrap_or("GET")
                                        ));
                                        if resource == "Document" && !blocked { frames.insert(frame.into(), url.into()); }
                                        id += 1;
                                        let (method, reply_params) = if blocked {
                                            ("Fetch.failRequest", serde_json::json!({"requestId": params["requestId"], "errorReason": "BlockedByClient"}))
                                        } else {
                                            ("Fetch.continueRequest", serde_json::json!({"requestId": params["requestId"]}))
                                        };
                                        let message = serde_json::json!({"id": id, "method": method, "params": reply_params});
                                        if sink.send(Message::Text(message.to_string().into())).await.is_err() { break; }
                                        continue;
                                    }
                                    "Runtime.bindingCalled" if value["params"]["name"] == "__comradeAdblock" => {
                                        if let Some(expression) = cosmetic_expression(&value["params"]) {
                                            id += 1;
                                            let message = serde_json::json!({"id": id, "method": "Runtime.evaluate", "params": {
                                                "expression": expression, "contextId": value["params"]["executionContextId"]
                                            }});
                                            if sink.send(Message::Text(message.to_string().into())).await.is_err() { break; }
                                        }
                                        continue;
                                    }
                                    _ => {}
                                }
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
fn record_frame_tree(
    tree: &Value,
    frames: &mut HashMap<String, String>,
    parents: &mut HashMap<String, String>,
    main: &mut String,
) {
    let frame = &tree["frame"];
    if let (Some(id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str()) {
        frames.insert(id.into(), url.into());
        if let Some(parent) = frame["parentId"].as_str() {
            parents.insert(id.into(), parent.into());
        } else {
            *main = id.into();
        }
    }
    if let Some(children) = tree["childFrames"].as_array() {
        for child in children {
            record_frame_tree(child, frames, parents, main);
        }
    }
}
fn request_kind(resource: &str, child_frame: bool) -> &str {
    match resource {
        "Document" if child_frame => "subdocument",
        "Document" => "document",
        "Stylesheet" => "stylesheet",
        "Image" => "image",
        "Media" => "media",
        "Font" => "font",
        "Script" => "script",
        "XHR" | "Fetch" => "xmlhttprequest",
        "Ping" => "ping",
        "WebSocket" => "websocket",
        _ => "other",
    }
}
fn cosmetic_expression(params: &Value) -> Option<String> {
    let payload = params["payload"].as_str()?;
    if payload.len() > 256 * 1024 {
        return None;
    }
    let data: Value = serde_json::from_str(payload).ok()?;
    let collect = |key: &str| -> Vec<String> {
        data[key]
            .as_array()
            .into_iter()
            .flatten()
            .take(2000)
            .filter_map(|v| v.as_str().filter(|s| s.len() <= 256).map(str::to_owned))
            .collect()
    };
    let shield = super::adblock::current()?;
    let selectors = shield.selectors(data["url"].as_str()?, &collect("classes"), &collect("ids"));
    let encoded = serde_json::to_string(&selectors).ok()?;
    Some(format!(
        r#"(() => {{
        let style = document.getElementById('__comradeAdblockStyle');
        const rules = {encoded};
        if (!rules.length) {{ if (style) style.remove(); return; }}
        if (!document.documentElement) return;
        if (!style) {{ style = document.createElement('style'); style.id = '__comradeAdblockStyle'; document.documentElement.appendChild(style); }}
        const sheet = style.sheet;
        if (!sheet) return;
        while (sheet.cssRules.length) sheet.deleteRule(0);
        for (const selector of rules) {{ try {{ sheet.insertRule(selector + '{{display:none!important}}', sheet.cssRules.length); }} catch (_) {{}} }}
    }})()"#
    ))
}
impl Session {
    pub(super) async fn protect(&self) -> Result<(), String> {
        let mut protected = self.protected.lock().await;
        if *protected {
            return Ok(());
        }
        super::adblock::ensure().await?;
        self.call("Page.enable", serde_json::json!({})).await?;
        self.call("Runtime.enable", serde_json::json!({})).await?;
        self.call("Runtime.addBinding", serde_json::json!({"name": "__comradeAdblock", "executionContextName": "comrade-adblock"})).await?;
        let source = include_str!("../../assets/adblock/cosmetic.js");
        self.call(
            "Page.addScriptToEvaluateOnNewDocument",
            serde_json::json!({"source": source, "worldName": "comrade-adblock"}),
        )
        .await?;
        // Bypass service workers so their cached responses cannot bypass filtering.
        self.call(
            "Network.setBypassServiceWorker",
            serde_json::json!({"bypass": true}),
        )
        .await?;
        self.call(
            "Fetch.enable",
            serde_json::json!({"patterns": [{"urlPattern": "*", "requestStage": "Request"}]}),
        )
        .await?;
        let tree = self
            .call("Page.getFrameTree", serde_json::json!({}))
            .await?;
        if let Some(frame) = tree["frameTree"]["frame"]["id"].as_str() {
            let world = self
                .call(
                    "Page.createIsolatedWorld",
                    serde_json::json!({"frameId": frame, "worldName": "comrade-adblock"}),
                )
                .await?;
            self.call(
                "Runtime.evaluate",
                serde_json::json!({"expression": source, "contextId": world["executionContextId"]}),
            )
            .await?;
        }
        *protected = true;
        Ok(())
    }

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
