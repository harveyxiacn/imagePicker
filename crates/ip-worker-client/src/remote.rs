//! [`RemoteWorker`]: an [`AiWorker`] that runs the whitelisted RPCs on the home PC's worker over
//! HTTP (`docs/api-contract-m8.md` section C).
//!
//! Every call is one `multipart/form-data` POST to `/api/remote/rpc`:
//! * `method` and `params` (the worker's JSON params, every image path rewritten to `file:<field>`),
//! * one file field per input image, downscaled to the documented long edge, **upright pixels**
//!   (EXIF orientation applied, so the params say `orientation: 1`).
//!
//! The answer is the worker's result with every artifact path rewritten to `remote:<file_id>`;
//! those files are downloaded into the request's `out_dir` and the paths restored, so callers see
//! exactly the shapes a local worker returns.
//!
//! TLS is rustls (ring) only; no OpenSSL anywhere, so the crate cross-compiles for Android.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::client::{CancelToken, ProgressTx};
use crate::error::{Result, WorkerError, CODE_MODEL_UNAVAILABLE};
use crate::*;

/// Worker RPCs a paired device may call (`POST /api/remote/rpc`).
pub const REMOTE_METHODS: [&str; 9] = [
    "system.info",
    "models.list",
    "analyze.batch",
    "mask.generate",
    "beauty.prepare",
    "besttake.compose",
    "inpaint.run",
    "enhance.run",
    "faces.embed",
];

pub const RPC_PATH: &str = "/api/remote/rpc";
pub const PAIR_COMPLETE_PATH: &str = "/api/remote/pair/complete";
pub const FILES_PATH: &str = "/api/remote/files";
pub const PING_PATH: &str = "/api/remote/ping";
/// Prefix of an input reference in `params` (the multipart field name follows).
pub const FILE_PREFIX: &str = "file:";
/// Prefix of an output artifact in a result (the file id follows).
pub const REMOTE_PREFIX: &str = "remote:";
/// Response header of `GET /api/remote/files/{id}`: the artifact's relative file name.
pub const FILE_NAME_HEADER: &str = "x-file-name";

/// Upload budget of one request (the host accepts 64 MB; leave room for the envelope).
pub const MAX_UPLOAD_BYTES: usize = 48 * 1024 * 1024;
/// Largest artifact the client accepts.
pub const MAX_DOWNLOAD_BYTES: u64 = 128 * 1024 * 1024;

/// Long edge of the copies uploaded for analysis (contract: default 1536).
pub const EDGE_ANALYSIS: u32 = 1536;
/// Long edge for generative / retouching requests (inpaint, best take, enhance, beauty, ...).
pub const EDGE_GENERATE: u32 = 2048;

/// A file-name component safe on every OS: `[A-Za-z0-9._-]`, at most 80 chars, never hidden.
pub fn safe_name(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    while out.contains("..") {
        out = out.replace("..", "_");
    }
    if out.starts_with('.') {
        out.replace_range(0..1, "_");
    }
    if out.is_empty() {
        out.push('f');
    }
    out
}

/// Normalises a user-typed host address to `scheme://host[:port]` (no path, credentials, query).
pub fn normalize_host_url(raw: &str) -> std::result::Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("host address is empty".into());
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let u = reqwest::Url::parse(&with_scheme).map_err(|e| format!("invalid host address: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("host address must start with http:// or https://".into());
    }
    if u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() {
        return Err("host address must be host[:port] without credentials".into());
    }
    if u.query().is_some() || u.fragment().is_some() || !matches!(u.path(), "" | "/") {
        return Err("host address must not contain a path or query".into());
    }
    Ok(u.origin().ascii_serialization())
}

/// Installs the rustls `ring` provider process-wide (idempotent). Needed before building any
/// `reqwest` client because the crate is built without a default provider.
pub fn ensure_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Ignore "already installed" (another component may have chosen a provider).
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Normalised `scheme://host[:port]`.
    pub host_url: String,
    /// The device token (never logged).
    pub token: String,
    pub connect_timeout: Duration,
    /// Reconnect attempts after a failed connect (backoff `backoff_base * 2^n`).
    pub retries: u32,
    pub backoff_base: Duration,
    /// Cap of a whole `analyze.batch` request.
    pub batch_timeout: Duration,
    /// Cap of every other request.
    pub call_timeout: Duration,
    pub analysis_edge: u32,
    pub generate_edge: u32,
}

impl RemoteConfig {
    pub fn new(host_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            host_url: host_url.into(),
            token: token.into(),
            connect_timeout: Duration::from_secs(5),
            retries: 3,
            backoff_base: Duration::from_millis(500),
            batch_timeout: Duration::from_secs(45 * 60),
            call_timeout: Duration::from_secs(12 * 60),
            analysis_edge: EDGE_ANALYSIS,
            generate_edge: EDGE_GENERATE,
        }
    }
}

struct Upload {
    field: String,
    filename: String,
    bytes: Vec<u8>,
}

/// Answer of `GET /api/remote/ping`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPing {
    pub tier: Option<String>,
    pub worker_state: Option<String>,
    pub host_name: Option<String>,
}

/// Result of a successful pairing.
#[derive(Debug, Clone)]
pub struct PairOutcome {
    pub device_id: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub enum PairError {
    /// Wrong, expired or already used code (401).
    InvalidCode(String),
    /// The code outlived its 5 minutes (401 `pair_expired`).
    Expired(String),
    /// Too many attempts (429); seconds to wait when the host said so.
    RateLimited(Option<u64>),
    /// The host cannot be reached.
    Unreachable(String),
    Other(String),
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCode(m) | Self::Expired(m) => write!(f, "{m}"),
            Self::RateLimited(Some(s)) => write!(f, "too many attempts; retry in {s} s"),
            Self::RateLimited(None) => write!(f, "too many attempts; try again later"),
            Self::Unreachable(m) => write!(f, "cannot reach the host: {m}"),
            Self::Other(m) => write!(f, "{m}"),
        }
    }
}

fn http_client(connect_timeout: Duration) -> Result<reqwest::Client> {
    ensure_crypto_provider();
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        // a redirect must never carry the token to another place
        .redirect(reqwest::redirect::Policy::none())
        // the host is on the LAN: environment proxies do not apply
        .no_proxy()
        .build()
        .map_err(|e| WorkerError::Unavailable(format!("cannot create the HTTP client: {e}")))
}

/// `POST /api/remote/pair/complete`: exchanges the code shown on the host for a device token.
pub async fn pair(
    host_url: &str,
    code: &str,
    device_name: &str,
) -> std::result::Result<PairOutcome, PairError> {
    let host = normalize_host_url(host_url).map_err(PairError::Other)?;
    let client =
        http_client(Duration::from_secs(5)).map_err(|e| PairError::Other(e.to_string()))?;
    let resp = client
        .post(format!("{host}{PAIR_COMPLETE_PATH}"))
        .timeout(Duration::from_secs(20))
        .header("content-type", "application/json")
        .body(json!({"code": code.trim(), "device_name": device_name}).to_string())
        .send()
        .await
        .map_err(|e| PairError::Unreachable(describe_transport(&e)))?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let body: Value = resp
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let msg = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match status {
        200 => {
            let id = body.get("device_id").and_then(Value::as_str);
            let tok = body.get("token").and_then(Value::as_str);
            match (id, tok) {
                (Some(id), Some(tok)) => Ok(PairOutcome {
                    device_id: id.to_string(),
                    token: tok.to_string(),
                }),
                _ => Err(PairError::Other("the host sent an invalid answer".into())),
            }
        }
        401 if body.pointer("/error/code").and_then(Value::as_str) == Some("pair_expired") => {
            Err(PairError::Expired(msg))
        }
        401 => Err(PairError::InvalidCode(if msg.is_empty() {
            "wrong or expired pairing code".into()
        } else {
            msg
        })),
        429 => Err(PairError::RateLimited(retry_after)),
        s => Err(PairError::Other(format!(
            "the host answered {s}{}",
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        ))),
    }
}

fn describe_transport(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

pub struct RemoteWorker {
    cfg: RemoteConfig,
    http: reqwest::Client,
    tx: watch::Sender<WorkerStatus>,
    last_error: Mutex<Option<String>>,
    last_ok: std::sync::atomic::AtomicI64,
    host_name: Mutex<Option<String>>,
}

impl RemoteWorker {
    pub fn new(cfg: RemoteConfig) -> Result<Self> {
        let http = http_client(cfg.connect_timeout)?;
        let (tx, _) = watch::channel(WorkerStatus {
            state: WorkerState::Stopped,
            tier: None,
            error: None,
            info: None,
        });
        Ok(Self {
            cfg,
            http,
            tx,
            last_error: Mutex::new(None),
            last_ok: std::sync::atomic::AtomicI64::new(0),
            host_name: Mutex::new(None),
        })
    }

    pub fn host_url(&self) -> &str {
        &self.cfg.host_url
    }

    /// Tier of the host's worker from the last successful `system.info`.
    pub fn host_tier(&self) -> Option<String> {
        self.tx.borrow().tier.clone()
    }

    /// Unix seconds of the last successful contact (0 = never).
    pub fn last_ok(&self) -> i64 {
        self.last_ok.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn host_name(&self) -> Option<String> {
        self.host_name.lock().unwrap().clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    fn record_ok(&self) {
        self.last_ok.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            std::sync::atomic::Ordering::Relaxed,
        );
        *self.last_error.lock().unwrap() = None;
        self.tx.send_modify(|s| {
            s.state = WorkerState::Ready;
            s.error = None;
        });
    }

    fn record_err(&self, e: &WorkerError) {
        if matches!(e, WorkerError::Cancelled) {
            return;
        }
        // Per-request failures (a model is missing, a call timed out) do not mean the host is down.
        let down = matches!(
            e,
            WorkerError::Unavailable(_) | WorkerError::Unpaired(_) | WorkerError::Disconnected
        );
        let msg = e.to_string();
        *self.last_error.lock().unwrap() = Some(msg.clone());
        self.tx.send_modify(|s| {
            if down {
                s.state = WorkerState::Unavailable;
            }
            s.error = Some(msg);
        });
    }

    /// `GET /api/remote/ping`: cheap authenticated liveness check (does not start the host's
    /// worker). Updates [`RemoteWorker::host_tier`] and the status.
    pub async fn ping(&self) -> Result<HostPing> {
        let r = self.ping_inner().await;
        match &r {
            Ok(p) => {
                self.record_ok();
                if p.host_name.is_some() {
                    *self.host_name.lock().unwrap() = p.host_name.clone();
                }
                if p.tier.is_some() {
                    self.tx.send_modify(|s| s.tier = p.tier.clone());
                }
            }
            Err(e) => self.record_err(e),
        }
        r
    }

    async fn ping_inner(&self) -> Result<HostPing> {
        let resp = self
            .http
            .get(format!("{}{PING_PATH}", self.cfg.host_url))
            .bearer_auth(&self.cfg.token)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| {
                WorkerError::Unavailable(format!(
                    "cannot reach the AI host {}: {}",
                    self.cfg.host_url,
                    describe_transport(&e)
                ))
            })?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.unwrap_or_default();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status != 200 {
            return Err(map_http_error(status, &body, "ping"));
        }
        Ok(HostPing {
            tier: body.get("tier").and_then(Value::as_str).map(str::to_string),
            host_name: body
                .get("host_name")
                .and_then(Value::as_str)
                .map(str::to_string),
            worker_state: body
                .get("worker_state")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    // ------------------------------------------------------------ images

    async fn image_upload(
        &self,
        field: &str,
        path: &str,
        orientation: u8,
        edge: u32,
    ) -> std::result::Result<Upload, String> {
        let p = PathBuf::from(path);
        let fmt = ip_imaging::ImageFormat::from_path(&p)
            .ok_or_else(|| format!("unsupported image type: {path}"))?;
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let enc = tokio::task::spawn_blocking(move || {
            ip_imaging::generate_thumbnail(&p, fmt, orientation, edge, 90)
        })
        .await
        .map_err(|e| format!("image task failed: {e}"))?
        .map_err(|e| format!("cannot decode {path}: {e}"))?;
        Ok(Upload {
            field: field.to_string(),
            filename: format!("{}.jpg", safe_name(&stem)),
            bytes: enc.bytes,
        })
    }

    async fn raw_upload(&self, field: &str, path: &str) -> std::result::Result<Upload, String> {
        let p = PathBuf::from(path);
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = tokio::fs::read(&p)
            .await
            .map_err(|e| format!("cannot read {path}: {e}"))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(format!("{path} is too large to upload"));
        }
        Ok(Upload {
            field: field.to_string(),
            filename: safe_name(&name),
            bytes,
        })
    }

    async fn photo(&self, field: &str, path: &str, o: u8, edge: u32) -> Result<Upload> {
        self.image_upload(field, path, o, edge)
            .await
            .map_err(WorkerError::Protocol)
    }

    // --------------------------------------------------------------- RPC

    fn timeout_of(&self, method: &str) -> Duration {
        if method == "analyze.batch" {
            self.cfg.batch_timeout
        } else if matches!(method, "system.info" | "models.list") {
            Duration::from_secs(30)
        } else {
            self.cfg.call_timeout
        }
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        uploads: &[Upload],
        out_dir: Option<&Path>,
        cancel: Option<&CancelToken>,
    ) -> Result<Value> {
        let fut = self.rpc_inner(method, params, uploads, out_dir);
        let r = match cancel {
            Some(c) => tokio::select! {
                r = fut => r,
                _ = c.cancelled() => Err(WorkerError::Cancelled),
            },
            None => fut.await,
        };
        match &r {
            Ok(_) => self.record_ok(),
            Err(e) => self.record_err(e),
        }
        r
    }

    async fn rpc_inner(
        &self,
        method: &str,
        params: Value,
        uploads: &[Upload],
        out_dir: Option<&Path>,
    ) -> Result<Value> {
        let url = format!("{}{RPC_PATH}", self.cfg.host_url);
        let params_text = params.to_string();
        let timeout = self.timeout_of(method);
        let mut attempt: u32 = 0;
        let resp = loop {
            let mut form = reqwest::multipart::Form::new()
                .text("method", method.to_string())
                .text("params", params_text.clone());
            for u in uploads {
                let part = reqwest::multipart::Part::bytes(u.bytes.clone())
                    .file_name(u.filename.clone())
                    .mime_str("application/octet-stream")
                    .map_err(|e| WorkerError::Protocol(e.to_string()))?;
                form = form.part(u.field.clone(), part);
            }
            let sent = self
                .http
                .post(&url)
                .bearer_auth(&self.cfg.token)
                .timeout(timeout)
                .multipart(form)
                .send()
                .await;
            match sent {
                Ok(r) => break r,
                // Nothing reached the host: safe to retry after a backoff.
                Err(e) if e.is_connect() && attempt < self.cfg.retries => {
                    let wait = self.cfg.backoff_base * 2u32.pow(attempt);
                    tracing::debug!(error = %describe_transport(&e), ?wait, "remote host unreachable; retrying");
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                }
                Err(e) if e.is_timeout() => {
                    return Err(WorkerError::Timeout(format!(
                        "the AI host did not answer {method} in time"
                    )));
                }
                Err(e) => {
                    return Err(WorkerError::Unavailable(format!(
                        "cannot reach the AI host {}: {}",
                        self.cfg.host_url,
                        describe_transport(&e)
                    )));
                }
            }
        };
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| WorkerError::Unavailable(format!("the AI host connection broke: {e}")))?;
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            return Err(map_http_error(status, &body, method));
        }
        match out_dir {
            Some(dir) => self.fetch_artifacts(body, dir).await,
            None => Ok(body),
        }
    }

    /// Downloads every `remote:<id>` of `v` into `out_dir` and replaces it by the local path.
    async fn fetch_artifacts(&self, mut v: Value, out_dir: &Path) -> Result<Value> {
        let mut ids: Vec<String> = Vec::new();
        collect_remote(&v, &mut ids);
        if ids.is_empty() {
            return Ok(v);
        }
        tokio::fs::create_dir_all(out_dir).await.map_err(|e| {
            WorkerError::Protocol(format!("cannot create {}: {e}", out_dir.display()))
        })?;
        let mut map: std::collections::HashMap<String, String> = Default::default();
        for chunk in ids.chunks(6) {
            let fetches = chunk.iter().map(|id| self.download(id, out_dir));
            for (id, r) in chunk
                .iter()
                .zip(futures_util::future::join_all(fetches).await)
            {
                map.insert(id.clone(), r?.to_string_lossy().into_owned());
            }
        }
        rewrite_remote(&mut v, &map);
        Ok(v)
    }

    async fn download(&self, id: &str, out_dir: &Path) -> Result<PathBuf> {
        let url = format!("{}{FILES_PATH}/{id}", self.cfg.host_url);
        let mut attempt = 0u32;
        let mut resp = loop {
            match self
                .http
                .get(&url)
                .bearer_auth(&self.cfg.token)
                .timeout(Duration::from_secs(120))
                .send()
                .await
            {
                Ok(r) => break r,
                Err(e) if e.is_connect() && attempt < self.cfg.retries => {
                    tokio::time::sleep(self.cfg.backoff_base * 2u32.pow(attempt)).await;
                    attempt += 1;
                }
                Err(e) => {
                    return Err(WorkerError::Unavailable(format!(
                        "downloading a result failed: {}",
                        describe_transport(&e)
                    )))
                }
            }
        };
        let status = resp.status().as_u16();
        if status != 200 {
            let b = resp.bytes().await.unwrap_or_default();
            let body: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
            return Err(map_http_error(status, &body, "files"));
        }
        let rel = resp
            .headers()
            .get(FILE_NAME_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("artifact.bin")
            .to_string();
        let mut dest = out_dir.to_path_buf();
        for comp in Path::new(&rel).components() {
            if let Component::Normal(c) = comp {
                dest.push(safe_name(&c.to_string_lossy()));
            }
        }
        if dest == out_dir {
            dest.push("artifact.bin");
        }
        if dest.exists() {
            let name = dest.file_name().unwrap().to_string_lossy().into_owned();
            dest.set_file_name(format!("{}-{name}", &id[..id.len().min(8)]));
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| WorkerError::Protocol(e.to_string()))?;
        }
        let tmp = dest.with_extension("part");
        let mut file = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| WorkerError::Protocol(format!("cannot write {}: {e}", tmp.display())))?;
        let mut total = 0u64;
        use tokio::io::AsyncWriteExt;
        loop {
            match resp.chunk().await {
                Ok(Some(c)) => {
                    total += c.len() as u64;
                    if total > MAX_DOWNLOAD_BYTES {
                        let _ = tokio::fs::remove_file(&tmp).await;
                        return Err(WorkerError::Protocol("a result file is too large".into()));
                    }
                    file.write_all(&c)
                        .await
                        .map_err(|e| WorkerError::Protocol(e.to_string()))?;
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    return Err(WorkerError::Unavailable(format!(
                        "downloading a result failed: {e}"
                    )));
                }
            }
        }
        file.flush()
            .await
            .map_err(|e| WorkerError::Protocol(e.to_string()))?;
        drop(file);
        tokio::fs::rename(&tmp, &dest)
            .await
            .map_err(|e| WorkerError::Protocol(e.to_string()))?;
        Ok(dest)
    }

    async fn typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        uploads: &[Upload],
        out_dir: Option<&Path>,
    ) -> Result<T> {
        let v = self.rpc(method, params, uploads, out_dir, None).await?;
        serde_json::from_value(v).map_err(|e| {
            WorkerError::Protocol(format!("invalid {method} result from the AI host: {e}"))
        })
    }
}

fn collect_remote(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if let Some(id) = s.strip_prefix(REMOTE_PREFIX) {
                if !out.iter().any(|x| x == id) {
                    out.push(id.to_string());
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_remote(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_remote(x, out)),
        _ => {}
    }
}

fn rewrite_remote(v: &mut Value, map: &std::collections::HashMap<String, String>) {
    match v {
        Value::String(s) => {
            if let Some(local) = s.strip_prefix(REMOTE_PREFIX).and_then(|id| map.get(id)) {
                *s = local.clone();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rewrite_remote(x, map)),
        Value::Object(o) => o.values_mut().for_each(|x| rewrite_remote(x, map)),
        _ => {}
    }
}

/// HTTP status + error body of the host -> the error a local worker would have raised.
pub fn map_http_error(status: u16, body: &Value, method: &str) -> WorkerError {
    let code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let msg = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match status {
        401 => WorkerError::Unpaired(if msg.is_empty() {
            "the host rejected the device token".into()
        } else {
            msg
        }),
        409 if code == "models_missing" => WorkerError::Rpc {
            code: CODE_MODEL_UNAVAILABLE,
            message: if msg.is_empty() {
                "a required model is not installed on the host".into()
            } else {
                msg
            },
            kind: Some("model_unavailable".into()),
            detail: Some(json!({ "models": body.get("models").cloned().unwrap_or(json!([])) })),
        },
        503 => WorkerError::Unavailable(if msg.is_empty() {
            "the AI worker on the host is unavailable".into()
        } else {
            msg
        }),
        504 => WorkerError::CallTimeout {
            method: body
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or(method)
                .to_string(),
            secs: body.get("secs").and_then(Value::as_u64).unwrap_or(0),
        },
        502 if code == "worker_error" => {
            let w = body.get("worker").cloned().unwrap_or(Value::Null);
            WorkerError::Rpc {
                code: w.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                message: w
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or(&msg)
                    .to_string(),
                kind: w.get("kind").and_then(Value::as_str).map(str::to_string),
                detail: w.get("detail").cloned().filter(|d| !d.is_null()),
            }
        }
        429 => WorkerError::Unavailable(if msg.is_empty() {
            "the AI host is busy; try again shortly".into()
        } else {
            msg
        }),
        _ => WorkerError::Protocol(format!(
            "the AI host answered {status}{}",
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        )),
    }
}

fn jset(v: &mut Value, ptr: &str, nv: Value) {
    if let Some(t) = v.pointer_mut(ptr) {
        *t = nv;
    }
}

fn to_params<T: serde::Serialize>(req: &T) -> Result<Value> {
    serde_json::to_value(req).map_err(|e| WorkerError::Protocol(e.to_string()))
}

fn not_remote(method: &str) -> WorkerError {
    WorkerError::Unavailable(format!(
        "{method} is not available through the remote AI host"
    ))
}

#[async_trait::async_trait]
impl AiWorker for RemoteWorker {
    fn status(&self) -> WorkerStatus {
        self.tx.borrow().clone()
    }

    fn subscribe(&self) -> watch::Receiver<WorkerStatus> {
        self.tx.subscribe()
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        let info: SystemInfo = self.typed("system.info", json!({}), &[], None).await?;
        self.tx.send_modify(|s| {
            s.tier = info.tier.clone();
            s.info = Some(info.clone());
        });
        Ok(info)
    }

    async fn models_list(&self) -> Result<ModelsListing> {
        self.typed("models.list", json!({}), &[], None).await
    }

    /// Models are installed on the host only; downloads are never started from a phone.
    async fn models_ensure(
        &self,
        _ids: &[String],
        _progress: Option<ProgressTx>,
        _cancel: &CancelToken,
    ) -> Result<()> {
        Err(WorkerError::Unavailable(
            "models are installed on the AI host; install them there".into(),
        ))
    }

    async fn analyze_batch(
        &self,
        req: &AnalyzeRequest,
        progress: Option<ProgressTx>,
        cancel: &CancelToken,
    ) -> Result<AnalyzeResponse> {
        let out_dir = PathBuf::from(&req.out_dir);
        let edge = self.cfg.analysis_edge;
        // prepare the uploads (decode + downscale); an unreadable photo is an item error
        let prepared = futures_util::future::join_all(req.items.iter().enumerate().map(
            |(i, it)| async move {
                self.image_upload(&format!("f{i}"), &it.path, it.orientation, edge)
                    .await
            },
        ))
        .await;
        let mut merged = AnalyzeResponse::default();
        let mut groups: Vec<Vec<(usize, Upload)>> = vec![Vec::new()];
        let mut group_bytes = 0usize;
        for (i, p) in prepared.into_iter().enumerate() {
            match p {
                Ok(up) => {
                    if group_bytes + up.bytes.len() > MAX_UPLOAD_BYTES
                        && !groups.last().unwrap().is_empty()
                    {
                        groups.push(Vec::new());
                        group_bytes = 0;
                    }
                    group_bytes += up.bytes.len();
                    groups.last_mut().unwrap().push((i, up));
                }
                Err(message) => merged.items.push(AnalyzeItem {
                    photo_id: req.items[i].photo_id,
                    error: Some(ItemError {
                        code: -32020,
                        message,
                        kind: Some("decode_failed".into()),
                    }),
                    ..Default::default()
                }),
            }
        }
        let mut done = merged.items.len();
        for group in groups.into_iter().filter(|g| !g.is_empty()) {
            if cancel.is_cancelled() {
                return Err(WorkerError::Cancelled);
            }
            let mut params = to_params(req)?;
            let mut items = Vec::new();
            let mut uploads = Vec::new();
            for (n, (i, up)) in group.into_iter().enumerate() {
                let mut item = to_params(&req.items[i])?;
                // re-number the fields so they are dense within the request
                let field = format!("f{n}");
                jset(&mut item, "/path", json!(format!("{FILE_PREFIX}{field}")));
                jset(&mut item, "/orientation", json!(1));
                items.push(item);
                uploads.push(Upload { field, ..up });
            }
            let count = items.len();
            jset(&mut params, "/items", Value::Array(items));
            jset(&mut params, "/out_dir", json!("out"));
            let v = self
                .rpc(
                    "analyze.batch",
                    params,
                    &uploads,
                    Some(&out_dir),
                    Some(cancel),
                )
                .await?;
            let part: AnalyzeResponse = serde_json::from_value(v).map_err(|e| {
                WorkerError::Protocol(format!("invalid analyze.batch result from the host: {e}"))
            })?;
            merged.items.extend(part.items);
            for s in part.steps {
                if !merged.steps.contains(&s) {
                    merged.steps.push(s);
                }
            }
            for s in part.skipped_steps {
                if !merged.skipped_steps.contains(&s) {
                    merged.skipped_steps.push(s);
                }
            }
            merged.warnings.extend(part.warnings);
            if merged.models.is_null() || merged.models == Value::Null {
                merged.models = part.models;
                merged.timings = part.timings;
            }
            done += count;
            if let Some(tx) = &progress {
                let _ = tx.send(json!({"kind": "analyze", "done": done, "total": req.items.len()}));
            }
        }
        Ok(merged)
    }

    async fn mask_generate(&self, req: &MaskRequest) -> Result<MaskResponse> {
        let up = self
            .photo(
                "f0",
                &req.photo.path,
                req.photo.orientation,
                self.cfg.analysis_edge,
            )
            .await?;
        let mut p = to_params(req)?;
        jset(&mut p, "/photo/path", json!("file:f0"));
        jset(&mut p, "/photo/orientation", json!(1));
        jset(&mut p, "/out_dir", json!("out"));
        self.typed("mask.generate", p, &[up], Some(Path::new(&req.out_dir)))
            .await
    }

    async fn beauty_prepare(&self, req: &BeautyPrepareRequest) -> Result<BeautyPrepareResponse> {
        let up = self
            .photo(
                "f0",
                &req.photo.path,
                req.photo.orientation,
                self.cfg.generate_edge,
            )
            .await?;
        let mut p = to_params(req)?;
        jset(&mut p, "/photo/path", json!("file:f0"));
        jset(&mut p, "/photo/orientation", json!(1));
        jset(&mut p, "/out_dir", json!("out"));
        self.typed("beauty.prepare", p, &[up], Some(Path::new(&req.out_dir)))
            .await
    }

    async fn faces_embed(&self, req: &FacesEmbedRequest) -> Result<FacesEmbedResponse> {
        let up = self
            .photo("f0", &req.path, req.orientation, self.cfg.generate_edge)
            .await?;
        let mut p = to_params(req)?;
        jset(&mut p, "/path", json!("file:f0"));
        jset(&mut p, "/orientation", json!(1));
        self.typed("faces.embed", p, &[up], None).await
    }

    async fn besttake_compose(
        &self,
        req: &BestTakeComposeRequest,
    ) -> Result<BestTakeComposeResponse> {
        let e = self.cfg.generate_edge;
        let base = self
            .photo("f0", &req.base.path, req.base.orientation, e)
            .await?;
        let source = self
            .photo("f1", &req.source.path, req.source.orientation, e)
            .await?;
        let mut p = to_params(req)?;
        jset(&mut p, "/base/path", json!("file:f0"));
        jset(&mut p, "/base/orientation", json!(1));
        jset(&mut p, "/source/path", json!("file:f1"));
        jset(&mut p, "/source/orientation", json!(1));
        jset(&mut p, "/out_dir", json!("out"));
        self.typed(
            "besttake.compose",
            p,
            &[base, source],
            Some(Path::new(&req.out_dir)),
        )
        .await
    }

    async fn inpaint_run(&self, req: &InpaintRequest) -> Result<InpaintResponse> {
        let photo = self
            .photo(
                "f0",
                &req.photo.path,
                req.photo.orientation,
                self.cfg.generate_edge,
            )
            .await?;
        let mask = self
            .raw_upload("f1", &req.mask)
            .await
            .map_err(WorkerError::Protocol)?;
        let mut p = to_params(req)?;
        jset(&mut p, "/photo/path", json!("file:f0"));
        jset(&mut p, "/photo/orientation", json!(1));
        jset(&mut p, "/mask", json!("file:f1"));
        jset(&mut p, "/out_dir", json!("out"));
        self.typed(
            "inpaint.run",
            p,
            &[photo, mask],
            Some(Path::new(&req.out_dir)),
        )
        .await
    }

    async fn enhance_run(&self, req: &EnhanceRequest) -> Result<EnhanceResponse> {
        let up = self
            .photo(
                "f0",
                &req.photo.path,
                req.photo.orientation,
                self.cfg.generate_edge,
            )
            .await?;
        let mut p = to_params(req)?;
        jset(&mut p, "/photo/path", json!("file:f0"));
        jset(&mut p, "/photo/orientation", json!(1));
        jset(&mut p, "/out_dir", json!("out"));
        self.typed("enhance.run", p, &[up], Some(Path::new(&req.out_dir)))
            .await
    }

    async fn llm_plan(&self, _req: &LlmPlanRequest) -> Result<LlmPlanResponse> {
        Err(not_remote("llm.plan"))
    }

    async fn vlm_suggest(&self, _req: &VlmSuggestRequest) -> Result<VlmSuggestResponse> {
        Err(not_remote("vlm.suggest"))
    }

    async fn vlm_describe(&self, _req: &VlmDescribeRequest) -> Result<VlmDescribeResponse> {
        Err(not_remote("vlm.describe"))
    }

    async fn models_delete(&self, _id: &str) -> Result<()> {
        Err(not_remote("models.delete"))
    }

    async fn shutdown(&self) {}

    /// The host owns its worker; a client-side timeout must not (and cannot) kill it.
    async fn kill(&self) {}
}

#[allow(dead_code)]
fn _assert_object_safe(_: Arc<dyn AiWorker>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_urls_are_normalised() {
        assert_eq!(
            normalize_host_url("192.168.1.5:7878").unwrap(),
            "http://192.168.1.5:7878"
        );
        assert_eq!(
            normalize_host_url(" http://pc.local:7878/ ").unwrap(),
            "http://pc.local:7878"
        );
        assert_eq!(
            normalize_host_url("https://pc.example").unwrap(),
            "https://pc.example"
        );
        for bad in [
            "",
            "ftp://x",
            "http://u:p@h",
            "http://h/path",
            "http://h/?x=1",
            "javascript:alert(1)",
        ] {
            assert!(normalize_host_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_are_sanitised() {
        assert_eq!(safe_name("a b/../c.npy"), "a_b___c.npy");
        assert_eq!(safe_name("..\\x"), "__x");
        assert_eq!(safe_name(".hidden"), "_hidden");
        assert_eq!(safe_name(""), "f");
        assert_eq!(safe_name("照片.jpg"), "__.jpg");
    }

    #[test]
    fn http_errors_map_to_worker_errors() {
        let e = map_http_error(
            409,
            &json!({"error":{"code":"models_missing","message":"m"},"models":["a","b"]}),
            "analyze.batch",
        );
        assert_eq!(e.model_unavailable().unwrap(), ["a", "b"]);
        assert!(matches!(
            map_http_error(401, &json!({}), "x"),
            WorkerError::Unpaired(_)
        ));
        assert!(matches!(
            map_http_error(503, &json!({"error":{"code":"worker_unavailable","message":"down"}}), "x"),
            WorkerError::Unavailable(m) if m == "down"
        ));
        assert!(matches!(
            map_http_error(504, &json!({"method":"inpaint.run","secs":300}), "x"),
            WorkerError::CallTimeout { secs: 300, .. }
        ));
        assert!(matches!(
            map_http_error(
                502,
                &json!({"error":{"code":"worker_error","message":"m"},"worker":{"code":-32020,"message":"bad","kind":"decode_failed"}}),
                "x"
            ),
            WorkerError::Rpc { code: -32020, .. }
        ));
    }

    #[test]
    fn remote_refs_are_collected_and_rewritten() {
        let mut v = json!({"items":[{"embedding_file":"remote:abc","n":1},{"embedding_file":"remote:abc"}],"masks":{"sky":"remote:def"},"x":"remote:unknown"});
        let mut ids = Vec::new();
        collect_remote(&v, &mut ids);
        assert_eq!(ids, ["abc", "def", "unknown"]);
        let map = [("abc".to_string(), "/o/a.npy".to_string())]
            .into_iter()
            .collect();
        rewrite_remote(&mut v, &map);
        assert_eq!(v["items"][1]["embedding_file"], "/o/a.npy");
        assert_eq!(v["masks"]["sky"], "remote:def");
    }
}
