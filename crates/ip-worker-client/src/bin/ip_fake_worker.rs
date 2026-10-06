//! A tiny stand-in for `imagepicker-ai serve`, used by this crate's tests (never shipped to users).
//!
//! Speaks just enough of the protocol: ready line, bearer-token auth, `system.info`,
//! `system.ping`, `system.shutdown`, `models.list`, `analyze.batch` (progress + empty items),
//! `hang` (waits for `cancel`), `crash` (exits) and `pid`.
//! `--sleeper` runs a process that only sleeps; with `--grandchild` the worker spawns one
//! and reports its pid in `system.info` (to test killing the whole process tree).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--sleeper") {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        return;
    }
    let token = args
        .windows(2)
        .find(|w| w[0] == "--token")
        .map(|w| w[1].clone())
        .expect("--token required");
    let mut grandchild: Option<u32> = None;
    if args.iter().any(|a| a == "--grandchild") {
        let exe = std::env::current_exe().unwrap();
        let child = std::process::Command::new(exe)
            .arg("--sleeper")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        grandchild = Some(child.id());
        std::mem::forget(child);
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    println!("{}", json!({"event": "ready", "port": port}));
    use std::io::Write;
    std::io::stdout().flush().unwrap();

    while let Ok((sock, _)) = listener.accept().await {
        let token = token.clone();
        tokio::spawn(async move {
            let cb = |req: &Request, resp: Response| {
                let ok = req
                    .headers()
                    .get("authorization")
                    .map(|v| v.to_str().unwrap_or("") == format!("Bearer {token}"))
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
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(t) = msg else { continue };
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                let method = v["method"].as_str().unwrap_or("");
                let id = v.get("id").and_then(Value::as_u64);
                let reply = |result: Value| {
                    if let Some(id) = id {
                        let _ = tx.send(json!({"jsonrpc":"2.0","id":id,"result":result}).to_string());
                    }
                };
                match method {
                    "system.info" => reply(json!({
                        "tier": "T9", "providers": ["CPUExecutionProvider"],
                        "hardware": {"device": "cpu", "gpus": [{"name": "Fake GPU", "vram_mb": 1234}]},
                        "pid": std::process::id(), "grandchild_pid": grandchild,
                    })),
                    "system.ping" => reply(json!({"pong": true})),
                    "pid" => reply(json!(std::process::id())),
                    "models.list" => reply(json!({"models": []})),
                    "system.shutdown" => {
                        reply(json!({"shutting_down": true}));
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        std::process::exit(0);
                    }
                    "crash" => std::process::exit(1),
                    "hang" => {
                        if let Some(id) = id {
                            hanging.lock().unwrap().insert(id, ());
                        }
                    }
                    "cancel" => {
                        let req = v["params"]["req"].as_u64().unwrap_or(0);
                        if hanging.lock().unwrap().remove(&req).is_some() {
                            let _ = tx.send(json!({"jsonrpc":"2.0","id":req,"error":{"code":-32800,"message":"request cancelled","data":{"kind":"cancelled"}}}).to_string());
                        }
                    }
                    "analyze.batch" => {
                        let n = v["params"]["items"].as_array().map(|a| a.len()).unwrap_or(0);
                        if let Some(id) = id {
                            let _ = tx.send(json!({"jsonrpc":"2.0","method":"progress","params":{"req":id,"kind":"analyze","done":n,"total":n}}).to_string());
                        }
                        reply(json!({"items": [], "steps": [], "skipped_steps": [], "warnings": []}));
                    }
                    _ => {}
                }
            }
        });
    }
}
