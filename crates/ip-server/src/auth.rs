//! Authentication, roles, LAN login sessions, the path whitelist and `GET /api/system/lan`
//! (docs/api-contract-m6.md §B). Everything is a middleware (`middleware`) plus a few routes
//! (`routes`), so it merges trivially with other route additions.
//!
//! Credentials accepted on protected `/api/*` routes:
//! * the desktop session token (`Authorization: Bearer`, or `?token=`), owner role;
//! * the session cookie `ip_session_<port>` (HttpOnly, SameSite=Strict) obtained by exchanging
//!   `/?token=...` (desktop) or by `POST /api/auth/login` (LAN: owner or read-only guest);
//! * nothing, when no desktop token is configured and the peer is a (non-proxied) loopback client
//!   that addressed the server by a loopback name (development / plain `imagepicker serve`).

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use ip_core::auth::{ct_eq, random_token, SecurityStore};
use ip_core::roots::{default_roots, RootSet};
use ip_core::Core;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiJson, ApiResult};
use crate::routes::AppState;

pub const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
pub const LOGIN_MAX_FAILURES: usize = 5;
pub const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const COOKIE_PREFIX: &str = "ip_session";
const MAX_SESSIONS: usize = 10_000;
/// Origins of the Vite dev server (allowed only with `dev_cors`).
pub const DEV_ORIGINS: [&str; 2] = ["http://localhost:5173", "http://127.0.0.1:5173"];

#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Per-launch desktop token. `None` = development mode (loopback needs no credentials).
    pub session_token: Option<String>,
    /// LAN mode: password login for remote clients.
    pub lan: bool,
    /// Allow the Vite dev origins through CORS (default: env `IMAGEPICKER_DEV_CORS`).
    pub dev_cors: bool,
    /// Extra allowed filesystem roots on top of the built-in and persisted ones.
    pub extra_roots: Vec<PathBuf>,
    pub session_ttl: Duration,
    pub login_max_failures: usize,
    pub login_window: Duration,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            session_token: None,
            lan: false,
            dev_cors: env_flag("IMAGEPICKER_DEV_CORS"),
            extra_roots: Vec::new(),
            session_ttl: SESSION_TTL,
            login_max_failures: LOGIN_MAX_FAILURES,
            login_window: LOGIN_WINDOW,
        }
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Owner,
    Guest,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Guest => "guest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Bearer / `?token=`.
    Token,
    /// Session cookie; `desktop` sessions come from the token exchange.
    Session { desktop: bool },
    /// No token configured and a trusted loopback peer.
    LocalOpen,
}

#[derive(Debug, Clone, Copy)]
pub struct Identity {
    pub role: Role,
    source: Source,
}

/// Request extension: the resolved identity (if any), also on the exempt `/api/auth/*` routes.
#[derive(Debug, Clone, Copy)]
pub struct MaybeIdentity(pub Option<Identity>);

/// Request extension: the TCP peer.
#[derive(Debug, Clone, Copy)]
pub struct Peer {
    pub ip: IpAddr,
    /// Loopback and no proxy headers: a genuinely local client.
    pub trusted_local: bool,
}

struct SessionRec {
    role: Role,
    /// `None` for desktop sessions (survive password changes).
    epoch: Option<u64>,
    last_seen: Instant,
}

pub struct AuthState {
    opts: ServerOptions,
    store: SecurityStore,
    sessions: Mutex<HashMap<[u8; 32], SessionRec>>,
    failures: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    port: AtomicU16,
}

fn key_of(id: &str) -> [u8; 32] {
    *blake3::hash(id.as_bytes()).as_bytes()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AuthState {
    pub fn new(core: &Core, opts: ServerOptions) -> Arc<Self> {
        Arc::new(Self {
            store: SecurityStore::open(core.data_dir()),
            opts,
            sessions: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            port: AtomicU16::new(0),
        })
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    pub fn store(&self) -> &SecurityStore {
        &self.store
    }

    pub fn dev_cors(&self) -> bool {
        self.opts.dev_cors
    }

    pub fn lan_enabled(&self) -> bool {
        self.opts.lan
    }

    // ------------------------------------------------------------ sessions

    fn create_session(&self, role: Role, desktop: bool) -> String {
        let id = random_token();
        let epoch = (!desktop).then(|| self.store.auth_epoch());
        let now = Instant::now();
        let mut s = lock(&self.sessions);
        if s.len() >= MAX_SESSIONS {
            let ttl = self.opts.session_ttl;
            s.retain(|_, r| now.duration_since(r.last_seen) < ttl);
        }
        if s.len() >= MAX_SESSIONS {
            // Still full of live sessions: drop the least recently used one.
            if let Some(k) = s.iter().min_by_key(|(_, r)| r.last_seen).map(|(k, _)| *k) {
                s.remove(&k);
            }
        }
        s.insert(
            key_of(&id),
            SessionRec {
                role,
                epoch,
                last_seen: now,
            },
        );
        id
    }

    /// Looks a session up, enforcing expiry (sliding) and password-change invalidation.
    fn lookup_session(&self, id: &str) -> Option<(Role, bool)> {
        let key = key_of(id);
        let epoch = self.store.auth_epoch();
        let now = Instant::now();
        let mut s = lock(&self.sessions);
        let rec = s.get_mut(&key)?;
        let expired = now.duration_since(rec.last_seen) >= self.opts.session_ttl;
        let stale = rec.epoch.is_some_and(|e| e != epoch);
        if expired || stale {
            s.remove(&key);
            return None;
        }
        rec.last_seen = now;
        Some((rec.role, rec.epoch.is_none()))
    }

    fn drop_session(&self, id: &str) {
        lock(&self.sessions).remove(&key_of(id));
    }

    // ---------------------------------------------------------- identities

    fn identify(&self, headers: &HeaderMap, uri: &Uri, peer: Peer) -> Option<Identity> {
        if let Some(tok) = &self.opts.session_token {
            let bearer = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    let (scheme, rest) = v.split_once(' ')?;
                    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
                });
            let query = query_token(uri);
            if bearer.is_some_and(|b| ct_eq(b, tok)) || query.is_some_and(|q| ct_eq(&q, tok)) {
                return Some(Identity {
                    role: Role::Owner,
                    source: Source::Token,
                });
            }
        }
        if let Some(id) = cookie_value(headers, &cookie_name(headers)) {
            if let Some((role, desktop)) = self.lookup_session(&id) {
                return Some(Identity {
                    role,
                    source: Source::Session { desktop },
                });
            }
        }
        if self.opts.session_token.is_none() && peer.trusted_local && host_is_local(headers) {
            return Some(Identity {
                role: Role::Owner,
                source: Source::LocalOpen,
            });
        }
        None
    }

    // ---------------------------------------------------------- rate limit

    fn retry_after(&self, ip: IpAddr) -> Option<u64> {
        let now = Instant::now();
        let mut f = lock(&self.failures);
        let q = f.get_mut(&ip)?;
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.opts.login_window)
        {
            q.pop_front();
        }
        if q.len() >= self.opts.login_max_failures {
            let oldest = *q.front()?;
            let wait = self
                .opts
                .login_window
                .saturating_sub(now.duration_since(oldest));
            return Some(wait.as_secs().max(1));
        }
        if q.is_empty() {
            f.remove(&ip);
        }
        None
    }

    fn record_failure(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut f = lock(&self.failures);
        if f.len() > 4096 {
            let w = self.opts.login_window;
            f.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < w));
        }
        f.entry(ip).or_default().push_back(now);
    }

    fn clear_failures(&self, ip: IpAddr) {
        lock(&self.failures).remove(&ip);
    }

    // ------------------------------------------------------ path whitelist

    /// Allowed roots: built-in (home, Pictures, removable drives), configured (`settings.roots`),
    /// options' extra roots and the roots of imported sessions.
    pub async fn root_set(&self, core: &Core) -> RootSet {
        let mut explicit: Vec<PathBuf> =
            self.store.roots().into_iter().map(PathBuf::from).collect();
        explicit.extend(self.opts.extra_roots.iter().cloned());
        if let Ok(sessions) = core.sessions().await {
            explicit.extend(sessions.into_iter().map(|s| PathBuf::from(s.root_path)));
        }
        tokio::task::spawn_blocking(move || RootSet::new(&default_roots(), &explicit))
            .await
            .unwrap_or_default()
    }

    /// Resolves a client-supplied path inside the whitelist (403 `forbidden_path` otherwise).
    /// The returned canonical path is what must be used for the actual operation.
    pub async fn check_path(&self, core: &Core, raw: &str) -> ApiResult<PathBuf> {
        let roots = self.root_set(core).await;
        let raw = raw.to_string();
        tokio::task::spawn_blocking(move || roots.check(&raw))
            .await
            .map_err(|e| {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
            })?
            .map_err(|e| ApiError::new(StatusCode::FORBIDDEN, "forbidden_path", e.0))
    }
}

// ------------------------------------------------------------------ helpers

fn query_token(uri: &Uri) -> Option<String> {
    let q = uri.query()?;
    serde_urlencoded::from_str::<Vec<(String, String)>>(q)
        .ok()?
        .into_iter()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v)
}

fn host_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::HOST).and_then(|v| v.to_str().ok())
}

fn host_port(host: &str) -> u16 {
    if host.ends_with(']') {
        return 80;
    }
    host.rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(80)
}

fn cookie_name(headers: &HeaderMap) -> String {
    format!(
        "{COOKIE_PREFIX}_{}",
        host_of(headers).map(host_port).unwrap_or(0)
    )
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for v in headers.get_all(header::COOKIE) {
        let Ok(s) = v.to_str() else { continue };
        for part in s.split(';') {
            if let Some((k, val)) = part.trim().split_once('=') {
                if k == name && !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// `localhost`, `127.x.y.z`, `[::1]` (any port): the only Host values for which unauthenticated
/// loopback access is granted. Defeats DNS-rebinding against the dev server.
fn host_is_local(headers: &HeaderMap) -> bool {
    // HTTP/1.0 without Host: no browser does that, so there is no rebinding to defend against.
    let Some(h) = host_of(headers) else {
        return true;
    };
    let name = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        h.rsplit_once(':').map(|(n, _)| n).unwrap_or(h)
    };
    name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

fn set_cookie(name: &str, value: &str, max_age: u64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{name}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}"
    ))
    .expect("cookie is ascii")
}

fn unauthorized() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "authentication required",
    )
}

fn forbidden(code: &'static str, msg: &str) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, code, msg)
}

fn origin_allowed(a: &AuthState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if a.opts.dev_cors && DEV_ORIGINS.iter().any(|o| o.eq_ignore_ascii_case(origin)) {
        return true;
    }
    let Some(host) = host_of(headers) else {
        return false;
    };
    let auth = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    auth.is_some_and(|o| o.eq_ignore_ascii_case(host))
}

/// Read-only restrictions for guests (02 §8): no people/face data, no settings/assistant, no
/// filesystem browsing.
fn guest_allows(method: &Method, path: &str, query: Option<&str>) -> bool {
    if !matches!(*method, Method::GET | Method::HEAD) {
        return false;
    }
    let segs: Vec<&str> = path
        .trim_start_matches("/api")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    const DENIED_ROOT: [&str; 14] = [
        "people",
        "faces",
        "taste",
        "settings",
        "assistant",
        "system",
        "fs",
        "xmp",
        "cache",
        "onboarding",
        "masks",
        "models",
        "besttake",
        "analysis",
    ];
    const DENIED_SUB: [&str; 6] = [
        "people",
        "faces",
        "analysis",
        "bystanders",
        "besttake",
        "beauty",
    ];
    let first = segs.first().copied().unwrap_or("");
    if DENIED_ROOT.contains(&first) {
        return false;
    }
    if segs.get(2).is_some_and(|s| DENIED_SUB.contains(s)) {
        return false;
    }
    if first == "photos" || first == "groups" {
        if let Some(q) = query {
            if let Ok(pairs) = serde_urlencoded::from_str::<Vec<(String, String)>>(q) {
                const PERSON_KEYS: [&str; 5] = [
                    "persons",
                    "exclude_persons",
                    "person_mode",
                    "person_state",
                    "faces_min",
                ];
                if pairs
                    .iter()
                    .any(|(k, v)| PERSON_KEYS.contains(&k.as_str()) && !v.is_empty())
                    || pairs.iter().any(|(k, v)| k == "faces_max" && !v.is_empty())
                {
                    return false;
                }
            }
        }
    }
    true
}

// --------------------------------------------------------------- middleware

pub async fn middleware(State(a): State<Arc<AuthState>>, mut req: Request, next: Next) -> Response {
    // Plain `oneshot` calls in tests carry no ConnectInfo: treated as loopback.
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_canonical())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let proxied = [
        "x-forwarded-for",
        "forwarded",
        "x-real-ip",
        "x-forwarded-host",
    ]
    .iter()
    .any(|h| req.headers().contains_key(*h));
    let peer = Peer {
        ip,
        trusted_local: ip.is_loopback() && !proxied,
    };

    // CSRF / cross-site WebSocket hijacking: state-changing requests and upgrades must be
    // same-origin when they carry an Origin header.
    let is_upgrade = req
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    let unsafe_method = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if (unsafe_method || is_upgrade) && !origin_allowed(&a, req.headers()) {
        return forbidden("forbidden_origin", "cross-origin request refused").into_response();
    }

    let path = req.uri().path().to_string();
    let is_api = path == "/api" || path.starts_with("/api/");

    if !is_api {
        // First document load of the desktop shell: `/?token=...` -> cookie + redirect.
        if *req.method() == Method::GET {
            if let (Some(tok), Some(given)) = (&a.opts.session_token, query_token(req.uri())) {
                if ct_eq(&given, tok) {
                    return exchange_token(&a, &req);
                }
            }
        }
        return next.run(req).await;
    }

    let exempt = path == "/api/health" || path == "/api/auth" || path.starts_with("/api/auth/");
    let ident = if path == "/api/health" {
        None
    } else {
        a.identify(req.headers(), req.uri(), peer)
    };
    if !exempt {
        let Some(id) = ident else {
            return unauthorized().into_response();
        };
        if id.role == Role::Guest && !guest_allows(req.method(), &path, req.uri().query()) {
            return forbidden("forbidden", "guests have read-only access to photos")
                .into_response();
        }
    }
    req.extensions_mut().insert(peer);
    req.extensions_mut().insert(MaybeIdentity(ident));
    next.run(req).await
}

fn exchange_token(a: &AuthState, req: &Request) -> Response {
    let id = a.create_session(Role::Owner, true);
    let pairs: Vec<(String, String)> = req
        .uri()
        .query()
        .and_then(|q| serde_urlencoded::from_str(q).ok())
        .unwrap_or_default();
    let rest: Vec<(String, String)> = pairs.into_iter().filter(|(k, _)| k != "token").collect();
    // A single leading slash: never a protocol-relative redirect.
    let mut loc = format!("/{}", req.uri().path().trim_start_matches('/'));
    if !rest.is_empty() {
        if let Ok(q) = serde_urlencoded::to_string(&rest) {
            loc.push('?');
            loc.push_str(&q);
        }
    }
    let mut resp = Redirect::to(&loc).into_response();
    let h = resp.headers_mut();
    h.append(
        header::SET_COOKIE,
        set_cookie(
            &cookie_name(req.headers()),
            &id,
            a.opts.session_ttl.as_secs(),
        ),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

// ------------------------------------------------------------------- routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/password", post(set_password))
        .route("/system/lan", get(lan_info))
}

#[derive(Deserialize)]
pub struct LoginBody {
    password: String,
}

fn with_cookie(mut resp: Response, cookie: HeaderValue) -> Response {
    resp.headers_mut().append(header::SET_COOKIE, cookie);
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

async fn login(
    State(st): State<AppState>,
    Extension(peer): Extension<Peer>,
    headers: HeaderMap,
    ApiJson(b): ApiJson<LoginBody>,
) -> ApiResult<Response> {
    let a = st.auth.clone();
    if !a.opts.lan {
        return Err(forbidden("lan_disabled", "LAN access is not enabled"));
    }
    if let Some(wait) = a.retry_after(peer.ip) {
        let mut e = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "too many failed login attempts; try again later",
        );
        e.extra = Some(json!({ "retry_after": wait }));
        let mut resp = e.into_response();
        if let Ok(v) = HeaderValue::from_str(&wait.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return Ok(resp);
    }
    let a2 = a.clone();
    let role = tokio::task::spawn_blocking(move || {
        if a2.store.verify_owner(&b.password) {
            Some(Role::Owner)
        } else if a2.store.verify_guest(&b.password) {
            Some(Role::Guest)
        } else {
            None
        }
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))?;
    let Some(role) = role else {
        a.record_failure(peer.ip);
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "wrong password",
        ));
    };
    a.clear_failures(peer.ip);
    let id = a.create_session(role, false);
    let resp = Json(json!({ "role": role.as_str() })).into_response();
    Ok(with_cookie(
        resp,
        set_cookie(&cookie_name(&headers), &id, a.opts.session_ttl.as_secs()),
    ))
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let name = cookie_name(&headers);
    if let Some(id) = cookie_value(&headers, &name) {
        st.auth.drop_session(&id);
    }
    with_cookie(
        StatusCode::NO_CONTENT.into_response(),
        set_cookie(&name, "", 0),
    )
}

async fn me(
    State(st): State<AppState>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
) -> Json<Value> {
    Json(json!({
        "role": id.map(|i| i.role.as_str()),
        "lan": st.auth.opts.lan,
    }))
}

#[derive(Deserialize)]
pub struct PasswordBody {
    password: String,
    #[serde(default)]
    guest_password: Option<String>,
}

async fn set_password(
    State(st): State<AppState>,
    Extension(peer): Extension<Peer>,
    Extension(MaybeIdentity(id)): Extension<MaybeIdentity>,
    headers: HeaderMap,
    ApiJson(b): ApiJson<PasswordBody>,
) -> ApiResult<Response> {
    let a = st.auth.clone();
    match id {
        Some(i) if i.role == Role::Owner => {}
        Some(_) => {
            return Err(forbidden(
                "forbidden",
                "only the owner may change passwords",
            ))
        }
        None => {
            // First-time setup from the local machine, without credentials.
            if a.store.has_owner_password() || !peer.trusted_local {
                return Err(unauthorized());
            }
        }
    }
    let a2 = a.clone();
    tokio::task::spawn_blocking(move || {
        a2.store
            .set_passwords(&b.password, b.guest_password.as_deref())
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))??;
    let resp = Json(json!({ "ok": true })).into_response();
    // The change invalidated every password session, including the caller's: re-issue theirs.
    if matches!(
        id,
        Some(Identity {
            source: Source::Session { desktop: false },
            ..
        })
    ) {
        let new = a.create_session(Role::Owner, false);
        return Ok(with_cookie(
            resp,
            set_cookie(&cookie_name(&headers), &new, a.opts.session_ttl.as_secs()),
        ));
    }
    Ok(resp)
}

async fn lan_info(State(st): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let a = &st.auth;
    let enabled = a.opts.lan;
    let configured = a.store.lan().enabled;
    let mut urls: Vec<String> = Vec::new();
    if enabled {
        let bound = a.port.load(Ordering::Relaxed);
        let port = if bound != 0 {
            bound
        } else {
            host_of(&headers).map(host_port).unwrap_or(80)
        };
        urls = lan_addresses()
            .into_iter()
            .map(|ip| format!("http://{ip}:{port}"))
            .collect();
    }
    let qr = urls.first().and_then(|u| qr_svg(u));
    Json(json!({
        "enabled": enabled,
        "urls": urls,
        "qr_svg": qr,
        "restart_required": configured != enabled,
    }))
}

/// Private-looking IPv4 addresses of this machine's interfaces (192.168/16, 10/8, 172.16/12
/// first).
pub fn lan_addresses() -> Vec<std::net::Ipv4Addr> {
    let mut ips: Vec<std::net::Ipv4Addr> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, ip)| match ip {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() => {
                Some(v4)
            }
            _ => None,
        })
        .collect();
    ips.sort_by_key(|ip| !ip.is_private());
    ips.dedup();
    ips
}

pub fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_rules() {
        let g = |m: &Method, p: &str, q: Option<&str>| guest_allows(m, p, q);
        assert!(g(
            &Method::GET,
            "/api/photos",
            Some("session_id=1&sort=name")
        ));
        assert!(g(&Method::GET, "/api/thumb/3", None));
        assert!(g(&Method::GET, "/api/original/3", None));
        assert!(!g(&Method::POST, "/api/export", None));
        assert!(!g(&Method::PATCH, "/api/photos", None));
        assert!(!g(&Method::GET, "/api/people", None));
        assert!(!g(&Method::GET, "/api/faces/3/crop", None));
        assert!(!g(&Method::GET, "/api/bursts/3/faces", None));
        assert!(!g(&Method::GET, "/api/photos/3/people", None));
        assert!(!g(&Method::GET, "/api/photos/3/analysis", None));
        assert!(!g(&Method::GET, "/api/taste", None));
        assert!(!g(&Method::GET, "/api/settings", None));
        assert!(!g(&Method::GET, "/api/assistant/status", None));
        assert!(!g(&Method::GET, "/api/fs/list", None));
        assert!(!g(&Method::GET, "/api/system/lan", None));
        assert!(!g(
            &Method::GET,
            "/api/photos",
            Some("session_id=1&persons=2")
        ));
        assert!(g(
            &Method::GET,
            "/api/photos",
            Some("session_id=1&persons=")
        ));
    }

    #[test]
    fn host_checks() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(header::HOST, HeaderValue::from_str(v).unwrap());
            m
        };
        assert!(host_is_local(&h("localhost:5173")));
        assert!(host_is_local(&h("127.0.0.1:7878")));
        assert!(host_is_local(&h("[::1]:7878")));
        assert!(!host_is_local(&h("evil.example:7878")));
        assert!(!host_is_local(&h("192.168.1.5:7878")));
        assert_eq!(host_port("127.0.0.1:7878"), 7878);
        assert_eq!(host_port("localhost"), 80);
    }

    #[test]
    fn qr_is_svg() {
        let s = qr_svg("http://192.168.1.10:7878").unwrap();
        assert!(s.contains("<svg"));
    }
}
