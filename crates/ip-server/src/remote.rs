//! Remote AI (docs/api-contract-m8.md section C).
//!
//! Host side: pairing (`/api/remote/pair/*`), device management, and the device-only
//! `/api/remote/rpc`, `/api/remote/files/{id}`, `/api/remote/ping` that expose the host's own AI
//! worker (through `Core::worker`, so timeouts, model management and the 409 / 503 semantics are
//! exactly those of local calls).
//!
//! Phone side: `/api/remote/connect`, `/api/remote/status` (owner only).
//!
//! Safety rules of `/api/remote/rpc`:
//! * the method must be in [`ip_worker_client::REMOTE_METHODS`];
//! * every image path in `params` must be a `file:<field>` reference to an uploaded file; the
//!   host never reads any other path on behalf of a device;
//! * uploads are written below a per-request directory, `out_dir` is forced into it and
//!   `allow_download` forced off (a phone cannot make the host download models);
//! * output paths below that directory become `remote:<id>` entries of a short-lived store bound
//!   to the device that made the request; everything is deleted at expiry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use ip_core::auth::random_token;
use ip_core::Core;
use ip_worker_client::remote::{
    safe_name, FILE_NAME_HEADER, FILE_PREFIX, REMOTE_METHODS, REMOTE_PREFIX,
};
use ip_worker_client::{
    AiWorker, AnalyzeRequest, BeautyPrepareRequest, BestTakeComposeRequest, CancelToken,
    EnhanceRequest, FacesEmbedRequest, InpaintRequest, MaskRequest, PairError, WorkerError,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::auth::{DeviceCtx, MaybeIdentity, Peer, Role};
use crate::error::{ApiError, ApiJson, ApiPath, ApiResult};
use crate::routes::AppState;

/// Limits of the host side (defaults per the contract; tests shrink them).
#[derive(Debug, Clone)]
pub struct RemoteOptions {
    /// A pairing code is valid this long (contract: 5 minutes).
    pub pairing_ttl: Duration,
    /// Result files are kept this long (contract: 10 minutes).
    pub file_ttl: Duration,
    /// Request body cap (contract: 64 MB).
    pub max_upload_bytes: usize,
    /// One uploaded file.
    pub max_file_bytes: usize,
    /// Uploaded files per request.
    pub max_files: usize,
    /// Result bytes one request may produce.
    pub max_result_bytes: u64,
    /// Result bytes held for all devices together.
    pub store_cap_bytes: u64,
    /// Simultaneous RPCs per device.
    pub per_device_concurrency: usize,
    /// Reading the upload may take this long.
    pub upload_timeout: Duration,
    /// Wall-clock cap of `analyze.batch`.
    pub batch_timeout: Duration,
    /// Wall-clock cap of every other method (the worker's own timeouts usually fire first).
    pub call_timeout: Duration,
    pub sweep_interval: Duration,
}

impl Default for RemoteOptions {
    fn default() -> Self {
        Self {
            pairing_ttl: ip_core::auth::PAIRING_TTL,
            file_ttl: Duration::from_secs(600),
            max_upload_bytes: 64 * 1024 * 1024,
            max_file_bytes: 32 * 1024 * 1024,
            max_files: 64,
            max_result_bytes: 256 * 1024 * 1024,
            store_cap_bytes: 1024 * 1024 * 1024,
            per_device_concurrency: 2,
            upload_timeout: Duration::from_secs(300),
            batch_timeout: Duration::from_secs(30 * 60),
            call_timeout: Duration::from_secs(15 * 60),
            sweep_interval: Duration::from_secs(30),
        }
    }
}

struct StoredFile {
    device_id: String,
    path: PathBuf,
    name: String,
    size: u64,
    expires: Instant,
    /// The request directory holding this file (removed once every file of it expired).
    dir: PathBuf,
}

/// Temp dirs, result store and per-device limits of the host.
pub struct RemoteHost {
    opts: RemoteOptions,
    root: PathBuf,
    files: Mutex<HashMap<String, StoredFile>>,
    limits: Mutex<HashMap<String, Arc<Semaphore>>>,
    sweeper: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl RemoteHost {
    pub fn new(core: &Core, opts: RemoteOptions) -> Arc<Self> {
        let root = core.dirs.root.join("cache").join("remote");
        // leftovers of a previous run are never served again
        let _ = std::fs::remove_dir_all(&root);
        Arc::new(Self {
            opts,
            root,
            files: Mutex::new(HashMap::new()),
            limits: Mutex::new(HashMap::new()),
            sweeper: AtomicBool::new(false),
        })
    }

    /// Starts the background cleanup task once (needs a running tokio runtime).
    fn ensure_sweeper(self: &Arc<Self>) {
        if self.sweeper.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let every = self.opts.sweep_interval;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                let Some(h) = weak.upgrade() else { break };
                h.sweep();
            }
        });
    }

    /// Drops expired result files (and their request directories).
    pub fn sweep(&self) {
        let now = Instant::now();
        let mut dirs: Vec<PathBuf> = Vec::new();
        {
            let mut f = lock(&self.files);
            f.retain(|_, e| {
                let keep = e.expires > now;
                if !keep {
                    dirs.push(e.dir.clone());
                }
                keep
            });
        }
        for d in dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    fn purge_device(&self, device_id: &str) {
        let mut dirs: Vec<PathBuf> = Vec::new();
        lock(&self.files).retain(|_, e| {
            let drop = e.device_id == device_id;
            if drop {
                dirs.push(e.dir.clone());
            }
            !drop
        });
        for d in dirs {
            let _ = std::fs::remove_dir_all(d);
        }
        lock(&self.limits).remove(device_id);
    }

    /// Number of live result files (tests).
    pub fn stored_files(&self) -> usize {
        lock(&self.files).len()
    }

    fn permit(&self, device_id: &str) -> ApiResult<OwnedSemaphorePermit> {
        let sem = lock(&self.limits)
            .entry(device_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.opts.per_device_concurrency.max(1))))
            .clone();
        sem.try_acquire_owned().map_err(|_| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "device_busy",
                "too many simultaneous AI requests from this device; retry shortly",
            )
        })
    }

    fn total_bytes(&self) -> u64 {
        lock(&self.files).values().map(|e| e.size).sum()
    }
}

// ------------------------------------------------------------------- routes

pub fn routes(opts: &RemoteOptions) -> Router<AppState> {
    Router::new()
        .route("/remote/pair/start", post(pair_start))
        .route("/remote/pair/complete", post(pair_complete))
        .route("/remote/devices", get(devices))
        .route("/remote/devices/{id}", delete(revoke_device))
        .route(
            "/remote/rpc",
            post(rpc).layer(DefaultBodyLimit::max(opts.max_upload_bytes + 64 * 1024)),
        )
        .route("/remote/files/{id}", get(file))
        .route("/remote/ping", get(ping))
        .route("/remote/connect", post(connect).delete(disconnect))
        .route("/remote/status", get(status))
}

fn require_owner(id: Option<crate::auth::Identity>) -> ApiResult<()> {
    match id {
        Some(i) if i.role == Role::Owner => Ok(()),
        Some(_) => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only the owner may do this",
        )),
        None => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "authentication required",
        )),
    }
}

fn host_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

// ---------------------------------------------------------- pairing (host)

async fn pair_start(
    State(st): State<AppState>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    require_owner(id)?;
    let a = &st.auth;
    if !a.lan_enabled() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "lan_disabled",
            "remote AI needs LAN access: enable it in the settings first",
        ));
    }
    let pc = a.pairing().start();
    let port = match a.bound_port() {
        0 => host_header(&headers)
            .and_then(|h| h.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()))
            .unwrap_or(80),
        p => p,
    };
    let mut urls: Vec<String> = crate::auth::lan_addresses()
        .into_iter()
        .map(|ip| format!("http://{ip}:{port}"))
        .collect();
    if urls.is_empty() {
        if let Some(h) = host_header(&headers) {
            urls.push(format!("http://{h}"));
        }
    }
    let base = urls.first().cloned().unwrap_or_default();
    let link = format!(
        "imagepicker://pair?url={}&code={}",
        urlencode(&base),
        pc.code
    );
    let qr = crate::auth::qr_svg(&link);
    let mut resp = Json(json!({
        "code": pc.code,
        // unix seconds
        "expires_at": pc.expires_at / 1000,
        "qr_svg": qr,
        "url": link,
        "urls": urls,
    }))
    .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

fn urlencode(s: &str) -> String {
    serde_urlencoded::to_string([("u", s)])
        .map(|q| q.trim_start_matches("u=").to_string())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
    device_name: String,
}

async fn pair_complete(
    State(st): State<AppState>,
    Extension(peer): Extension<Peer>,
    ApiJson(b): ApiJson<PairBody>,
) -> ApiResult<Response> {
    let a = &st.auth;
    if let Some(wait) = a.pair_retry_after(peer.ip) {
        let mut e = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "too many failed pairing attempts; try again later",
        );
        e.extra = Some(json!({ "retry_after": wait }));
        let mut resp = e.into_response();
        if let Ok(v) = HeaderValue::from_str(&wait.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return Ok(resp);
    }
    let name = b.device_name.trim();
    if name.is_empty() || name.chars().count() > ip_core::auth::MAX_DEVICE_NAME {
        return Err(ApiError::bad_request(
            "device_name must be 1..64 characters",
        ));
    }
    if let Err(why) = a.pairing().complete(&b.code) {
        a.pair_record_failure(peer.ip);
        return Err(match why {
            ip_core::auth::PairingInvalid::Expired => ApiError::new(
                StatusCode::UNAUTHORIZED,
                "pair_expired",
                "the pairing code has expired; start pairing again on the host",
            ),
            ip_core::auth::PairingInvalid::Invalid => ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_code",
                "wrong or already used pairing code",
            ),
        });
    }
    a.pair_clear_failures(peer.ip);
    let (info, token) = a.store().add_device(name)?;
    let mut resp = Json(json!({ "device_id": info.device_id, "token": token })).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

async fn devices(
    State(st): State<AppState>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
) -> ApiResult<Json<Value>> {
    require_owner(id)?;
    Ok(Json(json!({ "devices": st.auth.store().list_devices() })))
}

async fn revoke_device(
    State(st): State<AppState>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
    ApiPath(device_id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    require_owner(id)?;
    if !st.auth.store().revoke_device(&device_id)? {
        return Err(ApiError::not_found("no such device"));
    }
    st.remote.purge_device(&device_id);
    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------------------- ping

async fn ping(State(st): State<AppState>) -> Json<Value> {
    let s = st.core.worker.status();
    let tier = s
        .tier
        .clone()
        .or_else(|| s.info.as_ref().and_then(|i| i.tier.clone()));
    Json(json!({
        "ok": true,
        "tier": tier,
        "host_name": default_device_name_or("imagePicker host"),
        "worker_state": s.state.as_str(),
    }))
}

// --------------------------------------------------------------------- rpc

fn worker_error(e: WorkerError, method: &str) -> ApiError {
    if let Some(models) = e.model_unavailable() {
        let mut err = ApiError::new(
            StatusCode::CONFLICT,
            "models_missing",
            format!("models missing on the host: {}", models.join(", ")),
        );
        err.extra = Some(json!({ "models": models }));
        return err;
    }
    match e {
        WorkerError::Unavailable(m) => {
            let msg = m.replace(ip_worker_client::RUNTIME_MISSING_TAG, "");
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "worker_unavailable", msg)
        }
        WorkerError::Disconnected => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "worker_unavailable",
            "the connection to the AI worker was lost",
        ),
        WorkerError::CallTimeout { method, secs } => {
            let mut err = ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "worker_timeout",
                format!("the AI worker did not answer {method} within {secs} s"),
            );
            err.extra = Some(json!({ "method": method, "secs": secs }));
            err
        }
        WorkerError::Timeout(m) => {
            let mut err = ApiError::new(StatusCode::GATEWAY_TIMEOUT, "worker_timeout", m);
            err.extra = Some(json!({ "method": method, "secs": 0 }));
            err
        }
        WorkerError::Cancelled => ApiError::new(
            StatusCode::from_u16(499).unwrap_or(StatusCode::BAD_REQUEST),
            "cancelled",
            "request cancelled",
        ),
        WorkerError::Rpc {
            code,
            message,
            kind,
            detail,
        } => {
            let mut err = ApiError::new(StatusCode::BAD_GATEWAY, "worker_error", message.clone());
            err.extra = Some(json!({
                "worker": {"code": code, "message": message, "kind": kind, "detail": detail}
            }));
            err
        }
        other => ApiError::new(StatusCode::BAD_GATEWAY, "worker_error", other.to_string()),
    }
}

/// JSON pointers of the image paths of each method's params.
fn path_pointers(method: &str, params: &Value) -> Vec<String> {
    match method {
        "analyze.batch" => {
            let n = params
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            (0..n).map(|i| format!("/items/{i}/path")).collect()
        }
        "mask.generate" | "beauty.prepare" | "enhance.run" => vec!["/photo/path".into()],
        "faces.embed" => vec!["/path".into()],
        "besttake.compose" => vec!["/base/path".into(), "/source/path".into()],
        "inpaint.run" => vec!["/photo/path".into(), "/mask".into()],
        _ => Vec::new(),
    }
}

fn bind_files(
    method: &str,
    params: &mut Value,
    files: &HashMap<String, PathBuf>,
    out_dir: &Path,
) -> ApiResult<()> {
    if !params.is_object() {
        return Err(ApiError::bad_request("params must be a JSON object"));
    }
    for ptr in path_pointers(method, params) {
        let slot = params
            .pointer_mut(&ptr)
            .ok_or_else(|| ApiError::bad_request(format!("params{ptr} is missing")))?;
        let field = slot
            .as_str()
            .and_then(|s| s.strip_prefix(FILE_PREFIX))
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "params{ptr} must be a `file:<field>` reference to an uploaded file"
                ))
            })?;
        let path = files.get(field).ok_or_else(|| {
            ApiError::bad_request(format!(
                "params{ptr} refers to the missing upload `{field}`"
            ))
        })?;
        *slot = Value::String(path.to_string_lossy().into_owned());
    }
    // nothing but the references above may name a file
    fn leftover(v: &Value) -> bool {
        match v {
            Value::String(s) => s.starts_with(FILE_PREFIX),
            Value::Array(a) => a.iter().any(leftover),
            Value::Object(o) => o.values().any(leftover),
            _ => false,
        }
    }
    if leftover(params) {
        return Err(ApiError::bad_request(
            "`file:` references are only allowed in image path fields",
        ));
    }
    let obj = params.as_object_mut().expect("checked above");
    // the worker writes below the request directory; models are never downloaded for a device
    if method != "system.info" && method != "models.list" {
        obj.insert(
            "out_dir".into(),
            Value::String(out_dir.to_string_lossy().into_owned()),
        );
    }
    if obj.contains_key("allow_download") {
        obj.insert("allow_download".into(), Value::Bool(false));
    }
    Ok(())
}

fn bad_params(method: &str, e: serde_json::Error) -> ApiError {
    ApiError::bad_request(format!("invalid params for {method}: {e}"))
}

async fn dispatch(
    w: &Arc<dyn AiWorker>,
    method: &str,
    params: Value,
    cancel: &CancelToken,
) -> ApiResult<Value> {
    fn out<T: serde::Serialize>(r: Result<T, WorkerError>, method: &str) -> ApiResult<Value> {
        let v = r.map_err(|e| worker_error(e, method))?;
        serde_json::to_value(v).map_err(|e| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
        })
    }
    match method {
        "system.info" => out(w.system_info().await, method),
        "models.list" => out(w.models_list().await, method),
        "analyze.batch" => {
            let r: AnalyzeRequest =
                serde_json::from_value(params).map_err(|e| bad_params(method, e))?;
            out(w.analyze_batch(&r, None, cancel).await, method)
        }
        "mask.generate" => {
            let r: MaskRequest =
                serde_json::from_value(params).map_err(|e| bad_params(method, e))?;
            out(w.mask_generate(&r).await, method)
        }
        "beauty.prepare" => {
            let r: BeautyPrepareRequest =
                serde_json::from_value(params).map_err(|e| bad_params(method, e))?;
            out(w.beauty_prepare(&r).await, method)
        }
        "besttake.compose" => {
            let r: BestTakeComposeRequest =
                serde_json::from_value(params).map_err(|e| bad_params(method, e))?;
            out(w.besttake_compose(&r).await, method)
        }
        "inpaint.run" => {
            let r: InpaintRequest =
                serde_json::from_value(params).map_err(|e| bad_params(method, e))?;
            out(w.inpaint_run(&r).await, method)
        }
        "enhance.run" => {
            let r: EnhanceRequest =
                serde_json::from_value(params).map_err(|e| bad_params(method, e))?;
            out(w.enhance_run(&r).await, method)
        }
        "faces.embed" => {
            let r: FacesEmbedRequest =
                serde_json::from_value(params).map_err(|e| bad_params(method, e))?;
            out(w.faces_embed(&r).await, method)
        }
        _ => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "method_not_allowed",
            format!("{method} is not available to remote devices"),
        )),
    }
}

struct CancelOnDrop(CancelToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Removes the request directory unless it was handed over to the result store.
struct ReqDir {
    dir: PathBuf,
    keep: bool,
}
impl Drop for ReqDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

fn multipart_error(e: axum::extract::multipart::MultipartError) -> ApiError {
    let status = e.status();
    let code = if status == StatusCode::PAYLOAD_TOO_LARGE {
        "payload_too_large"
    } else {
        "bad_request"
    };
    ApiError::new(status, code, e.body_text())
}

struct Upload {
    method: String,
    params: Value,
    files: HashMap<String, PathBuf>,
}

async fn read_upload(host: &RemoteHost, mp: &mut Multipart, in_dir: &Path) -> ApiResult<Upload> {
    let o = &host.opts;
    let mut method: Option<String> = None;
    let mut params: Option<Value> = None;
    let mut files: HashMap<String, PathBuf> = HashMap::new();
    while let Some(mut field) = mp.next_field().await.map_err(multipart_error)? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "method" => {
                let t = field.text().await.map_err(multipart_error)?;
                if !REMOTE_METHODS.contains(&t.as_str()) {
                    return Err(ApiError::new(
                        StatusCode::FORBIDDEN,
                        "method_not_allowed",
                        format!("{t} is not available to remote devices"),
                    ));
                }
                method = Some(t);
            }
            "params" => {
                let t = field.text().await.map_err(multipart_error)?;
                if t.len() > 1024 * 1024 {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "payload_too_large",
                        "params are too large",
                    ));
                }
                params = Some(
                    serde_json::from_str(&t)
                        .map_err(|e| ApiError::bad_request(format!("params is not JSON: {e}")))?,
                );
            }
            f => {
                if method.is_none() || params.is_none() {
                    return Err(ApiError::bad_request(
                        "send `method` and `params` before the files",
                    ));
                }
                let valid = !f.is_empty()
                    && f.len() <= 32
                    && f.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if !valid {
                    return Err(ApiError::bad_request("invalid upload field name"));
                }
                if files.contains_key(f) {
                    return Err(ApiError::bad_request("duplicate upload field"));
                }
                if files.len() >= o.max_files {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "payload_too_large",
                        format!("at most {} files per request", o.max_files),
                    ));
                }
                let fname = safe_name(field.file_name().unwrap_or("upload.bin"));
                let dir = in_dir.join(f);
                tokio::fs::create_dir_all(&dir).await.map_err(io_err)?;
                let path = dir.join(fname);
                let mut file = tokio::fs::File::create(&path).await.map_err(io_err)?;
                let mut size = 0usize;
                while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                    size += chunk.len();
                    if size > o.max_file_bytes {
                        return Err(ApiError::new(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "payload_too_large",
                            format!("a file exceeds {} bytes", o.max_file_bytes),
                        ));
                    }
                    file.write_all(&chunk).await.map_err(io_err)?;
                }
                file.flush().await.map_err(io_err)?;
                files.insert(f.to_string(), path);
            }
        }
    }
    match (method, params) {
        (Some(method), Some(params)) => Ok(Upload {
            method,
            params,
            files,
        }),
        _ => Err(ApiError::bad_request("`method` and `params` are required")),
    }
}

fn io_err(e: std::io::Error) -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        format!("host file error: {e}"),
    )
}

fn walk_strings<'a>(v: &'a mut Value, out: &mut Vec<&'a mut String>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter_mut().for_each(|x| walk_strings(x, out)),
        Value::Object(o) => o.values_mut().for_each(|x| walk_strings(x, out)),
        _ => {}
    }
}

fn remove_unlisted(dir: &Path, keep: &std::collections::HashSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_unlisted(&p, keep);
        } else if !std::fs::canonicalize(&p).is_ok_and(|c| keep.contains(&c)) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

struct Artifact {
    id: String,
    path: PathBuf,
    name: String,
    size: u64,
}

/// Replaces every string naming a file below `out_dir` by `remote:<id>`.
fn extract_artifacts(
    result: &mut Value,
    out_dir: &Path,
    max_file: u64,
    max_total: u64,
) -> ApiResult<Vec<Artifact>> {
    let Ok(canon_out) = std::fs::canonicalize(out_dir) else {
        return Ok(Vec::new()); // the worker wrote nothing
    };
    let mut strings = Vec::new();
    walk_strings(result, &mut strings);
    let mut seen: HashMap<PathBuf, String> = HashMap::new();
    let mut arts: Vec<Artifact> = Vec::new();
    let mut total = 0u64;
    for s in strings {
        if s.len() > 1024 || !(s.contains('/') || s.contains('\\')) {
            continue;
        }
        let Ok(canon) = std::fs::canonicalize(&*s) else {
            continue;
        };
        if !canon.starts_with(&canon_out) {
            continue;
        }
        let Ok(md) = std::fs::metadata(&canon) else {
            continue;
        };
        if !md.is_file() {
            continue;
        }
        if let Some(id) = seen.get(&canon) {
            *s = format!("{REMOTE_PREFIX}{id}");
            continue;
        }
        if md.len() > max_file {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "result_too_large",
                "a result file exceeds the size limit",
            ));
        }
        total += md.len();
        if total > max_total {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "result_too_large",
                "the results exceed the size limit",
            ));
        }
        let rel = canon.strip_prefix(&canon_out).unwrap_or(&canon);
        let name = rel
            .components()
            .map(|c| safe_name(&c.as_os_str().to_string_lossy()))
            .collect::<Vec<_>>()
            .join("/");
        let id = random_token()[..32].to_string();
        seen.insert(canon.clone(), id.clone());
        *s = format!("{REMOTE_PREFIX}{id}");
        arts.push(Artifact {
            id,
            path: canon,
            name,
            size: md.len(),
        });
    }
    Ok(arts)
}

async fn rpc(
    State(st): State<AppState>,
    Extension(DeviceCtx(dev)): Extension<DeviceCtx>,
    mut mp: Multipart,
) -> ApiResult<Response> {
    let host = st.remote.clone();
    host.ensure_sweeper();
    host.sweep();
    let _permit = host.permit(&dev.device_id)?;

    let req_dir = host.root.join(format!("req-{}", &random_token()[..24]));
    let in_dir = req_dir.join("in");
    let out_dir = req_dir.join("out");
    tokio::fs::create_dir_all(&in_dir).await.map_err(io_err)?;
    tokio::fs::create_dir_all(&out_dir).await.map_err(io_err)?;
    let mut guard = ReqDir {
        dir: req_dir.clone(),
        keep: false,
    };

    let up = tokio::time::timeout(
        host.opts.upload_timeout,
        read_upload(&host, &mut mp, &in_dir),
    )
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::REQUEST_TIMEOUT,
            "upload_timeout",
            "the upload took too long",
        )
    })??;
    let Upload {
        method,
        mut params,
        files,
    } = up;
    bind_files(&method, &mut params, &files, &out_dir)?;

    let cancel = CancelToken::new();
    let _on_drop = CancelOnDrop(cancel.clone());
    let limit = if method == "analyze.batch" {
        host.opts.batch_timeout
    } else {
        host.opts.call_timeout
    };
    let mut result = match tokio::time::timeout(
        limit,
        dispatch(&st.core.worker, &method, params, &cancel),
    )
    .await
    {
        Ok(r) => r?,
        Err(_) => {
            cancel.cancel();
            let mut e = ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "worker_timeout",
                format!("{method} exceeded the host's {} s limit", limit.as_secs()),
            );
            e.extra = Some(json!({ "method": method, "secs": limit.as_secs() }));
            return Err(e);
        }
    };

    // the inputs are no longer needed
    let _ = tokio::fs::remove_dir_all(&in_dir).await;
    let (max_file, max_total) = (host.opts.max_result_bytes, host.opts.max_result_bytes);
    let (result, arts) = tokio::task::spawn_blocking({
        let out_dir = out_dir.clone();
        move || {
            let arts = extract_artifacts(&mut result, &out_dir, max_file, max_total)?;
            let keep: std::collections::HashSet<PathBuf> =
                arts.iter().map(|a| a.path.clone()).collect();
            remove_unlisted(&out_dir, &keep);
            Ok::<_, ApiError>((result, arts))
        }
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))??;

    if arts.is_empty() {
        return Ok(Json(result).into_response());
    }
    // make room: the store is capped, oldest entries (they expire within minutes anyway) go first
    let new_bytes: u64 = arts.iter().map(|a| a.size).sum();
    if new_bytes > host.opts.store_cap_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "result_too_large",
            "the results exceed the host's result store",
        ));
    }
    while host.total_bytes() + new_bytes > host.opts.store_cap_bytes {
        let oldest = lock(&host.files)
            .iter()
            .min_by_key(|(_, e)| e.expires)
            .map(|(k, _)| k.clone());
        let Some(k) = oldest else { break };
        if let Some(e) = lock(&host.files).remove(&k) {
            let _ = std::fs::remove_file(&e.path);
        }
    }
    let expires = Instant::now() + host.opts.file_ttl;
    {
        let mut f = lock(&host.files);
        for a in arts {
            f.insert(
                a.id,
                StoredFile {
                    device_id: dev.device_id.clone(),
                    path: a.path,
                    name: a.name,
                    size: a.size,
                    expires,
                    dir: req_dir.clone(),
                },
            );
        }
    }
    guard.keep = true;
    Ok(Json(result).into_response())
}

// ------------------------------------------------------------------- files

async fn file(
    State(st): State<AppState>,
    Extension(DeviceCtx(dev)): Extension<DeviceCtx>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Response> {
    let host = st.remote.clone();
    host.sweep();
    let (path, name) = {
        let f = lock(&host.files);
        match f.get(&id) {
            // files are private to the device that produced them
            Some(e) if e.device_id == dev.device_id && e.expires > Instant::now() => {
                (e.path.clone(), e.name.clone())
            }
            _ => return Err(ApiError::not_found("no such file (it may have expired)")),
        }
    };
    let mut resp = ServeFile::new(&path)
        .oneshot(Request::new(Body::empty()))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))?
        .into_response();
    if resp.status() == StatusCode::NOT_FOUND {
        return Err(ApiError::not_found("no such file (it may have expired)"));
    }
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&name) {
        h.insert(FILE_NAME_HEADER, v);
    }
    Ok(resp)
}

// ------------------------------------------------------- phone side (owner)

#[derive(Deserialize)]
struct ConnectBody {
    #[serde(alias = "url")]
    host_url: String,
    code: String,
    #[serde(default)]
    device_name: String,
}

fn default_device_name_or(fallback: &str) -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| fallback.into())
}

fn default_device_name() -> String {
    default_device_name_or("imagePicker device")
}

async fn connect(
    State(st): State<AppState>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
    ApiJson(b): ApiJson<ConnectBody>,
) -> ApiResult<Response> {
    require_owner(id)?;
    let host = ip_worker_client::normalize_host_url(&b.host_url).map_err(ApiError::bad_request)?;
    let name = if b.device_name.trim().is_empty() {
        default_device_name()
    } else {
        b.device_name.clone()
    };
    let outcome = ip_worker_client::pair(&host, &b.code, &name)
        .await
        .map_err(|e| match e {
            PairError::InvalidCode(m) => ApiError::new(StatusCode::UNAUTHORIZED, "invalid_code", m),
            PairError::Expired(m) => ApiError::new(StatusCode::UNAUTHORIZED, "pair_expired", m),
            PairError::RateLimited(w) => {
                let mut err = ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too_many_requests",
                    "the host refused further pairing attempts for now",
                );
                err.extra = w.map(|w| json!({ "retry_after": w }));
                err
            }
            PairError::Unreachable(m) => ApiError::new(
                StatusCode::BAD_GATEWAY,
                "host_unreachable",
                format!("cannot reach the host: {m}"),
            ),
            PairError::Other(m) => ApiError::new(StatusCode::BAD_GATEWAY, "pair_failed", m),
        })?;
    st.core.remote_save_pairing(&host, &outcome)?;
    if let Ok(full) = crate::m6::full_settings(&st) {
        st.core.emit_settings_updated(full);
    }
    Ok(Json(st.core.remote_status().await).into_response())
}

async fn disconnect(
    State(st): State<AppState>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
) -> ApiResult<StatusCode> {
    require_owner(id)?;
    st.core.remote_disconnect()?;
    if let Ok(full) = crate::m6::full_settings(&st) {
        st.core.emit_settings_updated(full);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn status(
    State(st): State<AppState>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
) -> ApiResult<Json<Value>> {
    require_owner(id)?;
    Ok(Json(
        serde_json::to_value(st.core.remote_status().await).map_err(|e| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
        })?,
    ))
}
