//! JSON-RPC 2.0 over WebSocket client: request ids, progress streams, cancellation, typed errors.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::error::WorkerError;

/// Receives the `params` object of every `progress` notification of one request.
pub type ProgressTx = mpsc::UnboundedSender<Value>;

/// Cooperative cancellation flag shared between the caller and an in-flight request.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

#[derive(Default)]
struct CancelInner {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::SeqCst)
    }

    /// Completes once [`cancel`](Self::cancel) was called.
    pub async fn cancelled(&self) {
        loop {
            let waiting = self.inner.notify.notified();
            tokio::pin!(waiting);
            // Register interest before checking the flag so a concurrent cancel() is not missed.
            waiting.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            waiting.await;
        }
    }
}

struct Pending {
    resp: oneshot::Sender<Result<Value, WorkerError>>,
    progress: Option<ProgressTx>,
}

struct Inner {
    out: mpsc::UnboundedSender<Message>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_id: AtomicU64,
    closed: watch::Sender<bool>,
}

/// A connected JSON-RPC client. Cheap to clone; all clones share one connection.
#[derive(Clone)]
pub struct RpcClient {
    inner: Arc<Inner>,
}

/// How long to wait for the response of a cancelled request before giving up on it.
const CANCEL_GRACE: Duration = Duration::from_secs(15);

impl RpcClient {
    /// Connects to `ws://<host>:<port>/` authenticating with `Authorization: Bearer <token>`.
    pub async fn connect(host: &str, port: u16, token: &str) -> Result<Self, WorkerError> {
        let mut req = format!("ws://{host}:{port}/")
            .into_client_request()
            .map_err(|e| WorkerError::Protocol(e.to_string()))?;
        req.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|_| WorkerError::Protocol("invalid token".into()))?,
        );
        let (ws, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(req),
        )
        .await
        .map_err(|_| WorkerError::Timeout("websocket connect".into()))?
        .map_err(|e| WorkerError::Unavailable(format!("websocket connect failed: {e}")))?;
        Ok(Self::from_stream(ws))
    }

    fn from_stream<S>(ws: tokio_tungstenite::WebSocketStream<S>) -> Self
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let (closed_tx, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            out: out_tx,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            closed: closed_tx,
        });
        tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                if sink.send(m).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let reader = inner.clone();
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(Message::Text(t)) => reader.dispatch(t.as_str()),
                    Ok(Message::Binary(b)) => {
                        if let Ok(s) = std::str::from_utf8(&b) {
                            reader.dispatch(s);
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            reader.close();
        });
        Self { inner }
    }

    pub fn is_closed(&self) -> bool {
        *self.inner.closed.borrow()
    }

    /// Resolves when the connection is gone (peer exit, network error, [`close`](Self::close)).
    pub async fn closed(&self) {
        let mut rx = self.inner.closed.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Drops the connection; pending requests fail with [`WorkerError::Disconnected`].
    pub fn close(&self) {
        let _ = self.inner.out.send(Message::Close(None));
        self.inner.close();
    }

    /// Sends a request and waits for its response.
    ///
    /// `progress` receives the `params` of this request's `progress` notifications. When `cancel`
    /// fires, a `cancel` notification is sent and the call ends with [`WorkerError::Cancelled`].
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        progress: Option<ProgressTx>,
        cancel: Option<&CancelToken>,
    ) -> Result<Value, WorkerError> {
        if self.is_closed() {
            return Err(WorkerError::Disconnected);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .unwrap()
            .insert(id, Pending { resp: tx, progress });
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.inner.out.send(Message::text(frame.to_string())).is_err() {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(WorkerError::Disconnected);
        }
        // The connection may have died between the check and the registration.
        if self.is_closed() {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(WorkerError::Disconnected);
        }
        tokio::pin!(rx);
        let flatten = |r: Result<Result<Value, WorkerError>, oneshot::error::RecvError>| match r {
            Ok(v) => v,
            Err(_) => Err(WorkerError::Disconnected),
        };
        let Some(cancel) = cancel else {
            return flatten(rx.await);
        };
        tokio::select! {
            r = &mut rx => flatten(r),
            _ = cancel.cancelled() => {
                self.notify("cancel", json!({"req": id}));
                match tokio::time::timeout(CANCEL_GRACE, &mut rx).await {
                    Ok(r) => match flatten(r) {
                        Err(WorkerError::Rpc { code: -32800, .. }) | Err(WorkerError::Cancelled) => Err(WorkerError::Cancelled),
                        // finished just before the cancel arrived: the caller still asked to stop
                        Ok(_) => Err(WorkerError::Cancelled),
                        Err(e) => Err(e),
                    },
                    Err(_) => {
                        self.inner.pending.lock().unwrap().remove(&id);
                        Err(WorkerError::Cancelled)
                    }
                }
            }
        }
    }

    /// Fire-and-forget notification.
    pub fn notify(&self, method: &str, params: Value) {
        let frame = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let _ = self.inner.out.send(Message::text(frame.to_string()));
    }
}

impl Inner {
    fn close(&self) {
        if self.closed.send_replace(true) {
            return;
        }
        let drained: Vec<Pending> = self.pending.lock().unwrap().drain().map(|(_, p)| p).collect();
        for p in drained {
            let _ = p.resp.send(Err(WorkerError::Disconnected));
        }
    }

    fn dispatch(&self, text: &str) {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            tracing::warn!("worker sent invalid JSON");
            return;
        };
        match v {
            Value::Array(items) => items.iter().for_each(|i| self.dispatch_one(i)),
            other => self.dispatch_one(&other),
        }
    }

    fn dispatch_one(&self, v: &Value) {
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            let Some(p) = self.pending.lock().unwrap().remove(&id) else {
                return;
            };
            let res = match v.get("error") {
                Some(e) if !e.is_null() => Err(WorkerError::from_rpc_error(e)),
                _ => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = p.resp.send(res);
        } else if v.get("method").and_then(Value::as_str) == Some("progress") {
            let Some(params) = v.get("params") else {
                return;
            };
            let Some(req) = params.get("req").and_then(Value::as_u64) else {
                return;
            };
            if let Some(p) = self.pending.lock().unwrap().get(&req) {
                if let Some(tx) = &p.progress {
                    let _ = tx.send(params.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    /// Minimal in-process JSON-RPC server: `echo`, `slow` (progress + result), `hang`
    /// (answers -32800 once cancelled), `boom` (error) and `die` (drops the socket).
    async fn serve() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let cb = |req: &Request, resp: Response| {
                        let ok = req
                            .headers()
                            .get("authorization")
                            .map(|v| v == "Bearer tok")
                            .unwrap_or(false);
                        if ok {
                            Ok(resp)
                        } else {
                            Err(tokio_tungstenite::tungstenite::http::Response::builder()
                                .status(401)
                                .body(None)
                                .unwrap())
                        }
                    };
                    let Ok(ws) = tokio_tungstenite::accept_hdr_async(sock, cb).await else {
                        return;
                    };
                    let (mut sink, mut stream) = ws.split();
                    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                    tokio::spawn(async move {
                        while let Some(m) = rx.recv().await {
                            if sink.send(Message::text(m)).await.is_err() {
                                break;
                            }
                        }
                    });
                    let hanging: Arc<Mutex<HashMap<u64, ()>>> = Arc::default();
                    while let Some(Ok(Message::Text(t))) = stream.next().await {
                        let v: Value = serde_json::from_str(t.as_str()).unwrap();
                        let method = v["method"].as_str().unwrap().to_string();
                        let id = v.get("id").and_then(Value::as_u64);
                        match (method.as_str(), id) {
                            ("echo", Some(id)) => {
                                let _ = tx.send(json!({"jsonrpc":"2.0","id":id,"result":v["params"]}).to_string());
                            }
                            ("slow", Some(id)) => {
                                for d in 1..=3 {
                                    let _ = tx.send(json!({"jsonrpc":"2.0","method":"progress","params":{"req":id,"done":d,"total":3}}).to_string());
                                }
                                let _ = tx.send(json!({"jsonrpc":"2.0","id":id,"result":"done"}).to_string());
                            }
                            ("hang", Some(id)) => {
                                hanging.lock().unwrap().insert(id, ());
                            }
                            ("cancel", None) => {
                                let req = v["params"]["req"].as_u64().unwrap();
                                if hanging.lock().unwrap().remove(&req).is_some() {
                                    let _ = tx.send(json!({"jsonrpc":"2.0","id":req,"error":{"code":-32800,"message":"request cancelled","data":{"kind":"cancelled"}}}).to_string());
                                }
                            }
                            ("boom", Some(id)) => {
                                let _ = tx.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32010,"message":"models not installed","data":{"kind":"model_unavailable","detail":{"models":["a","b"]}}}}).to_string());
                            }
                            ("die", _) => break,
                            _ => {}
                        }
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn echo_progress_and_errors() {
        let port = serve().await;
        let c = RpcClient::connect("127.0.0.1", port, "tok").await.unwrap();
        let r = c.call("echo", json!({"x": 1}), None, None).await.unwrap();
        assert_eq!(r, json!({"x": 1}));

        let (tx, mut rx) = mpsc::unbounded_channel();
        let r = c.call("slow", json!({}), Some(tx), None).await.unwrap();
        assert_eq!(r, json!("done"));
        let mut seen = Vec::new();
        while let Ok(p) = rx.try_recv() {
            seen.push(p["done"].as_u64().unwrap());
        }
        assert_eq!(seen, vec![1, 2, 3]);

        match c.call("boom", json!({}), None, None).await {
            Err(e @ WorkerError::Rpc { .. }) => {
                assert_eq!(e.model_unavailable(), Some(vec!["a".to_string(), "b".to_string()]));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn concurrent_requests_are_matched_by_id() {
        let port = serve().await;
        let c = RpcClient::connect("127.0.0.1", port, "tok").await.unwrap();
        let futs: Vec<_> = (0..20)
            .map(|i| {
                let c = c.clone();
                tokio::spawn(async move { c.call("echo", json!(i), None, None).await.unwrap() })
            })
            .collect();
        for (i, f) in futs.into_iter().enumerate() {
            assert_eq!(f.await.unwrap(), json!(i));
        }
    }

    #[tokio::test]
    async fn cancel_sends_notification_and_returns_cancelled() {
        let port = serve().await;
        let c = RpcClient::connect("127.0.0.1", port, "tok").await.unwrap();
        let tok = CancelToken::new();
        let t2 = tok.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            t2.cancel();
        });
        let r = c.call("hang", json!({}), None, Some(&tok)).await;
        assert!(matches!(r, Err(WorkerError::Cancelled)), "{r:?}");
        // the connection is still usable
        assert_eq!(c.call("echo", json!(7), None, None).await.unwrap(), json!(7));
    }

    #[tokio::test]
    async fn disconnect_fails_pending_and_future_calls() {
        let port = serve().await;
        let c = RpcClient::connect("127.0.0.1", port, "tok").await.unwrap();
        let c2 = c.clone();
        let pending = tokio::spawn(async move { c2.call("hang", json!({}), None, None).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        c.notify("die", json!({}));
        let r = pending.await.unwrap();
        assert!(matches!(r, Err(WorkerError::Disconnected)), "{r:?}");
        tokio::time::timeout(Duration::from_secs(2), c.closed())
            .await
            .unwrap();
        assert!(matches!(
            c.call("echo", json!(1), None, None).await,
            Err(WorkerError::Disconnected)
        ));
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() {
        let port = serve().await;
        assert!(RpcClient::connect("127.0.0.1", port, "nope").await.is_err());
    }
}
