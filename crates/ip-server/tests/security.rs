//! Authentication, roles, LAN mode, CORS and the path whitelist (docs/api-contract-m6.md §B).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ip_core::auth::LanSettings;
use ip_core::testutil::FakeImaging;
use ip_core::{Core, CoreConfig};
use ip_server::{build_router_with, spawn_with_core, SecurityStore, ServerConfig, ServerOptions};
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OWNER_PW: &str = "owner password 1";
const GUEST_PW: &str = "guest password 1";
const LAN_PEER: &str = "192.168.1.50:51000";

#[cfg(windows)]
const SYSTEM_DIR: &str = "C:\\Windows";
#[cfg(not(windows))]
const SYSTEM_DIR: &str = "/etc";

struct Env {
    core: Arc<Core>,
    store: SecurityStore,
    data: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: None,
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    let store = SecurityStore::open(data.path());
    Env { core, store, data }
}

impl Env {
    fn app(&self, opts: ServerOptions) -> Router {
        build_router_with(self.core.clone(), None, opts)
    }
    /// LAN mode with an owner and a guest password.
    fn lan_app(&self, tweak: impl FnOnce(&mut ServerOptions)) -> Router {
        self.store.set_passwords(OWNER_PW, Some(GUEST_PW)).unwrap();
        self.store
            .set_lan(LanSettings {
                enabled: true,
                port: 7878,
                guest_enabled: true,
            })
            .unwrap();
        let mut o = ServerOptions {
            lan: true,
            dev_cors: false,
            ..ServerOptions::default()
        };
        tweak(&mut o);
        self.app(o)
    }
}

fn token_opts() -> ServerOptions {
    ServerOptions {
        session_token: Some(TOKEN.into()),
        dev_cors: false,
        ..ServerOptions::default()
    }
}

fn dev_opts() -> ServerOptions {
    ServerOptions {
        dev_cors: false,
        ..ServerOptions::default()
    }
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
    /// `name=value` of the first Set-Cookie.
    fn cookie(&self) -> Option<String> {
        let v = self.headers.get(header::SET_COOKIE)?.to_str().ok()?;
        Some(v.split(';').next()?.to_string())
    }
    fn set_cookie_raw(&self) -> String {
        self.headers
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }
}

#[derive(Default, Clone)]
struct Req {
    method: Option<Method>,
    uri: String,
    body: Option<Value>,
    headers: Vec<(String, String)>,
    peer: Option<&'static str>,
}

fn get(uri: &str) -> Req {
    Req {
        uri: uri.into(),
        ..Default::default()
    }
}

fn post(uri: &str, body: Value) -> Req {
    Req {
        method: Some(Method::POST),
        uri: uri.into(),
        body: Some(body),
        ..Default::default()
    }
}

impl Req {
    fn h(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    fn cookie(self, c: &str) -> Self {
        self.h("cookie", c)
    }
    fn bearer(self, t: &str) -> Self {
        self.h("authorization", &format!("Bearer {t}"))
    }
    fn lan(mut self) -> Self {
        self.peer = Some(LAN_PEER);
        self
    }
    fn method(mut self, m: Method) -> Self {
        self.method = Some(m);
        self
    }
    fn body(mut self, b: Value) -> Self {
        self.body = Some(b);
        self
    }
}

async fn send(app: &Router, r: Req) -> Resp {
    let mut b = Request::builder()
        .method(r.method.unwrap_or(Method::GET))
        .uri(&r.uri);
    if !r.headers.iter().any(|(k, _)| k == "host") {
        b = b.header(header::HOST, "127.0.0.1:7878");
    }
    for (k, v) in &r.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    let mut req = match &r.body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    if let Some(p) = r.peer {
        req.extensions_mut()
            .insert(ConnectInfo(p.parse::<SocketAddr>().unwrap()));
    }
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Resp {
        status,
        headers,
        body,
    }
}

async fn login(app: &Router, pw: &str) -> Resp {
    send(
        app,
        post("/api/auth/login", json!({ "password": pw })).lan(),
    )
    .await
}

fn enc(p: &str) -> String {
    p.replace('%', "%25")
        .replace('\\', "%5C")
        .replace(':', "%3A")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23")
}

// ---------------------------------------------------------------- desktop token

#[tokio::test]
async fn token_mode_requires_credentials() {
    let e = env();
    let app = e.app(token_opts());
    let r = send(&app, get("/api/sessions")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.code(), "unauthorized");
    // unknown routes do not leak either
    assert_eq!(
        send(&app, get("/api/nope")).await.status,
        StatusCode::UNAUTHORIZED
    );
    // health stays open
    assert_eq!(send(&app, get("/api/health")).await.status, StatusCode::OK);
    // bearer
    let r = send(&app, get("/api/sessions").bearer(TOKEN)).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(&app, get("/api/sessions").bearer("wrong")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(&app, get("/api/sessions").bearer(&TOKEN[..10])).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // ?token= on an API call (what a WebSocket client without headers would use)
    let r = send(&app, get(&format!("/api/sessions?token={TOKEN}"))).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(&app, get("/api/sessions?token=nope")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // the desktop token also works from loopback in the presence of a LAN peer? No: it is the
    // credential, wherever it comes from; a bad one never does.
    let r = send(&app, get("/api/sessions").lan()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(&app, get("/api/sessions").lan().bearer(TOKEN)).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn token_exchange_sets_cookie_and_redirects() {
    let e = env();
    let app = e.app(token_opts());
    let r = send(&app, get(&format!("/?token={TOKEN}"))).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.headers.get(header::LOCATION).unwrap(), "/");
    let raw = r.set_cookie_raw();
    assert!(raw.contains("HttpOnly"), "{raw}");
    assert!(raw.contains("SameSite=Strict"), "{raw}");
    assert!(raw.contains("Path=/"), "{raw}");
    assert!(raw.contains("Max-Age=604800"), "{raw}");
    assert!(
        !raw.contains(TOKEN),
        "the cookie must not contain the token itself"
    );
    let cookie = r.cookie().unwrap();
    assert!(cookie.starts_with("ip_session_7878="), "{cookie}");
    assert_eq!(cookie.split('=').nth(1).unwrap().len(), 64, "256-bit id");

    let r = send(&app, get("/api/sessions").cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(&app, get("/api/auth/me").cookie(&cookie)).await;
    assert_eq!(r.json()["role"], "owner");

    // other query parameters survive, the token does not
    let r = send(&app, get(&format!("/library?a=1&token={TOKEN}&b=x%20y"))).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(
        r.headers.get(header::LOCATION).unwrap(),
        "/library?a=1&b=x+y"
    );
    // never a protocol-relative redirect
    let r = send(&app, get(&format!("//evil.example/x?token={TOKEN}"))).await;
    assert_eq!(r.headers.get(header::LOCATION).unwrap(), "/evil.example/x");
    // a wrong token neither redirects nor sets a cookie
    let r = send(&app, get("/?token=wrong")).await;
    assert_ne!(r.status, StatusCode::SEE_OTHER);
    assert!(r.headers.get(header::SET_COOKIE).is_none());
    // a forged cookie is worthless
    let r = send(
        &app,
        get("/api/sessions").cookie(&format!("ip_session_7878={}", "ab".repeat(32))),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn desktop_session_survives_password_change() {
    let e = env();
    let app = e.app(token_opts());
    let r = send(&app, get(&format!("/?token={TOKEN}"))).await;
    let cookie = r.cookie().unwrap();
    e.store.set_passwords(OWNER_PW, None).unwrap();
    let r = send(&app, get("/api/sessions").cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::OK);
}

// ------------------------------------------------------------------ dev mode

#[tokio::test]
async fn dev_mode_allows_only_real_loopback_clients() {
    let e = env();
    let app = e.app(dev_opts());
    assert_eq!(
        send(&app, get("/api/sessions")).await.status,
        StatusCode::OK
    );
    let r = send(&app, get("/api/auth/me")).await;
    assert_eq!(r.json()["role"], "owner");
    assert_eq!(r.json()["lan"], false);
    // remote peer
    let r = send(&app, get("/api/sessions").lan()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // reverse proxy on the same machine: the peer looks local but the request is not
    let r = send(&app, get("/api/sessions").h("x-forwarded-for", "8.8.8.8")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // DNS rebinding: loopback peer, foreign Host
    let r = send(&app, get("/api/sessions").h("host", "evil.example:7878")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(&app, get("/api/sessions").h("host", "localhost:5173")).await;
    assert_eq!(r.status, StatusCode::OK);
}

// ----------------------------------------------------------------- LAN login

#[tokio::test]
async fn lan_login_me_logout() {
    let e = env();
    let app = e.lan_app(|_| {});
    // remote clients need a login
    let r = send(&app, get("/api/sessions").lan()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(&app, get("/api/auth/me").lan()).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({"role": null, "lan": true}));

    let r = login(&app, "wrong password").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.code(), "invalid_credentials");
    assert!(r.headers.get(header::SET_COOKIE).is_none());

    let r = login(&app, OWNER_PW).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["role"], "owner");
    let raw = r.set_cookie_raw();
    assert!(
        raw.contains("HttpOnly") && raw.contains("SameSite=Strict"),
        "{raw}"
    );
    let cookie = r.cookie().unwrap();

    let r = send(&app, get("/api/sessions").lan().cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(&app, get("/api/auth/me").lan().cookie(&cookie)).await;
    assert_eq!(r.json(), json!({"role": "owner", "lan": true}));

    let r = send(
        &app,
        post("/api/auth/logout", json!({})).lan().cookie(&cookie),
    )
    .await;
    assert!(r.status.is_success());
    assert!(r.set_cookie_raw().contains("Max-Age=0"));
    let r = send(&app, get("/api/sessions").lan().cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(&app, get("/api/auth/me").lan().cookie(&cookie)).await;
    assert_eq!(r.json()["role"], Value::Null);
}

#[tokio::test]
async fn login_is_refused_outside_lan_mode() {
    let e = env();
    e.store.set_passwords(OWNER_PW, None).unwrap();
    let app = e.app(token_opts());
    let r = login(&app, OWNER_PW).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.code(), "lan_disabled");
}

#[tokio::test]
async fn guest_is_read_only() {
    let e = env();
    let app = e.lan_app(|_| {});
    let r = login(&app, GUEST_PW).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["role"], "guest");
    let cookie = r.cookie().unwrap();
    let g = |uri: &str| get(uri).lan().cookie(&cookie);

    assert_eq!(send(&app, g("/api/sessions")).await.status, StatusCode::OK);
    let me = send(&app, g("/api/auth/me")).await;
    assert_eq!(me.json()["role"], "guest");
    for uri in [
        "/api/people",
        "/api/people/best",
        "/api/faces/1/crop",
        "/api/bursts/1/faces",
        "/api/photos/1/people",
        "/api/photos/1/analysis",
        "/api/taste",
        "/api/settings",
        "/api/assistant/status",
        "/api/fs/roots",
        "/api/fs/list",
        "/api/system/lan",
        "/api/photos?session_id=1&persons=3",
    ] {
        let r = send(&app, g(uri)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(r.code(), "forbidden", "{uri}");
    }
    for (m, uri) in [
        (Method::PATCH, "/api/photos"),
        (Method::POST, "/api/export"),
        (Method::POST, "/api/import"),
        (Method::PUT, "/api/edits/1"),
        (Method::DELETE, "/api/sessions/1"),
        (Method::POST, "/api/taste/reset"),
        (Method::POST, "/api/auth/password"),
    ] {
        let r = send(
            &app,
            g(uri).method(m.clone()).body(json!({"password": "x"})),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{m} {uri}");
    }
    // the guest may log out
    let r = send(
        &app,
        post("/api/auth/logout", json!({})).lan().cookie(&cookie),
    )
    .await;
    assert!(r.status.is_success());
    assert_eq!(
        send(&app, g("/api/sessions")).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn guest_login_needs_guest_enabled() {
    let e = env();
    let app = e.lan_app(|_| {});
    e.store
        .set_lan(LanSettings {
            enabled: true,
            port: 7878,
            guest_enabled: false,
        })
        .unwrap();
    assert_eq!(login(&app, GUEST_PW).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, OWNER_PW).await.status, StatusCode::OK);
}

#[tokio::test]
async fn login_rate_limit() {
    let e = env();
    let app = e.lan_app(|_| {});
    for i in 0..5 {
        let r = login(&app, &format!("bad password {i}")).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    // the 6th attempt is refused, even with the right password
    let r = login(&app, OWNER_PW).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.code(), "too_many_requests");
    assert!(r.headers.get(header::RETRY_AFTER).is_some());
    // another address is not affected
    let r = send(&app, {
        let mut q = post("/api/auth/login", json!({"password": OWNER_PW}));
        q.peer = Some("192.168.1.99:40000");
        q
    })
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn login_rate_limit_window_expires() {
    let e = env();
    let app = e.lan_app(|o| {
        o.login_window = Duration::from_millis(300);
    });
    for _ in 0..5 {
        login(&app, "bad password").await;
    }
    assert_eq!(
        login(&app, OWNER_PW).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(login(&app, OWNER_PW).await.status, StatusCode::OK);
}

#[tokio::test]
async fn sessions_expire_with_sliding_window() {
    let e = env();
    let app = e.lan_app(|o| o.session_ttl = Duration::from_millis(700));
    let cookie = login(&app, OWNER_PW).await.cookie().unwrap();
    let ok = |c: &str| send(&app, get("/api/sessions").lan().cookie(c));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(ok(&cookie).await.status, StatusCode::OK); // refreshes the window
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(ok(&cookie).await.status, StatusCode::OK, "sliding");
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        ok(&cookie).await.status,
        StatusCode::UNAUTHORIZED,
        "expired"
    );
}

#[tokio::test]
async fn password_change_invalidates_sessions() {
    let e = env();
    let app = e.lan_app(|_| {});
    let owner = login(&app, OWNER_PW).await.cookie().unwrap();
    let guest = login(&app, GUEST_PW).await.cookie().unwrap();

    // guests cannot change it
    let r = send(
        &app,
        post("/api/auth/password", json!({"password": "hacked password"}))
            .lan()
            .cookie(&guest),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // too short
    let r = send(
        &app,
        post("/api/auth/password", json!({"password": "short"}))
            .lan()
            .cookie(&owner),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let r = send(
        &app,
        post(
            "/api/auth/password",
            json!({"password": "brand new secret", "guest_password": "new guest secret"}),
        )
        .lan()
        .cookie(&owner),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let renewed = r.cookie().expect("the caller gets a fresh session");
    assert_ne!(renewed, owner);
    // old sessions are gone, the renewed one works
    for old in [&owner, &guest] {
        let r = send(&app, get("/api/sessions").lan().cookie(old)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let r = send(&app, get("/api/sessions").lan().cookie(&renewed)).await;
    assert_eq!(r.status, StatusCode::OK);
    // old passwords stop working, new ones work
    assert_eq!(login(&app, OWNER_PW).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login(&app, "brand new secret").await.status, StatusCode::OK);
    assert_eq!(
        login(&app, "new guest secret").await.json()["role"],
        "guest"
    );
}

#[tokio::test]
async fn first_time_password_only_from_loopback() {
    let e = env();
    let app = e.app(token_opts());
    // remote peer, no credentials
    let r = send(
        &app,
        post("/api/auth/password", json!({"password": OWNER_PW})).lan(),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // proxied
    let r = send(
        &app,
        post("/api/auth/password", json!({"password": OWNER_PW})).h("x-forwarded-for", "1.2.3.4"),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(!e.store.has_owner_password());
    // local, first time, no credentials
    let r = send(
        &app,
        post("/api/auth/password", json!({"password": OWNER_PW})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(e.store.verify_owner(OWNER_PW));
    // afterwards the owner is required
    let r = send(
        &app,
        post("/api/auth/password", json!({"password": "hijack password"})),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(
        &app,
        post("/api/auth/password", json!({"password": "legit password"})).bearer(TOKEN),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(e.store.verify_owner("legit password"));
}

// ---------------------------------------------------------------- CORS / CSRF

#[tokio::test]
async fn no_cors_headers_by_default() {
    let e = env();
    let app = e.app(dev_opts());
    let r = send(&app, get("/api/health").h("origin", "http://evil.example")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    // preflight
    let r = send(
        &app,
        get("/api/photos")
            .method(Method::OPTIONS)
            .h("origin", "http://localhost:5173")
            .h("access-control-request-method", "POST"),
    )
    .await;
    assert!(r.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    // cross-site writes are refused outright
    let r = send(
        &app,
        post("/api/import", json!({"path": "x"})).h("origin", "http://evil.example"),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.code(), "forbidden_origin");
    let r = send(
        &app,
        post("/api/import", json!({"path": "x"})).h("origin", "http://localhost:5173"),
    )
    .await;
    assert_eq!(
        r.code(),
        "forbidden_origin",
        "dev origin is off without the flag"
    );
    let r = send(
        &app,
        post("/api/import", json!({"path": "x"})).h("origin", "null"),
    )
    .await;
    assert_eq!(r.code(), "forbidden_origin");
    // same-origin writes are fine (the import itself fails later, but not on the origin)
    let r = send(
        &app,
        post("/api/import", json!({"path": SYSTEM_DIR})).h("origin", "http://127.0.0.1:7878"),
    )
    .await;
    assert_eq!(r.code(), "forbidden_path");
}

#[tokio::test]
async fn dev_cors_flag_allows_vite_origins_only() {
    let e = env();
    let app = e.app(ServerOptions {
        dev_cors: true,
        ..ServerOptions::default()
    });
    for origin in ["http://localhost:5173", "http://127.0.0.1:5173"] {
        let r = send(&app, get("/api/health").h("origin", origin)).await;
        assert_eq!(
            r.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            origin
        );
        assert_eq!(
            r.headers
                .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .unwrap(),
            "true"
        );
        let r = send(
            &app,
            post("/api/import", json!({"path": SYSTEM_DIR})).h("origin", origin),
        )
        .await;
        assert_eq!(r.code(), "forbidden_path", "write passes the origin check");
    }
    let r = send(&app, get("/api/health").h("origin", "http://evil.example")).await;
    assert!(r.headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    let r = send(
        &app,
        post("/api/import", json!({"path": "x"})).h("origin", "http://evil.example"),
    )
    .await;
    assert_eq!(r.code(), "forbidden_origin");
}

// ------------------------------------------------------------------ whitelist

fn strict(e: &Env) -> Router {
    e.app(ServerOptions {
        dev_cors: false,
        ..ServerOptions::default()
    })
}

#[tokio::test]
async fn whitelist_rejects_outside_paths_everywhere() {
    let e = env();
    let app = strict(&e);
    let outside = SYSTEM_DIR;
    let r = send(&app, get(&format!("/api/fs/list?path={}", enc(outside)))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.code(), "forbidden_path");
    let r = send(&app, post("/api/import", json!({"path": outside}))).await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::FORBIDDEN, "forbidden_path")
    );
    let r = send(
        &app,
        post("/api/export", json!({"ids": [1], "dest": outside})),
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::FORBIDDEN, "forbidden_path")
    );
    let r = send(
        &app,
        post(
            "/api/luts/import",
            json!({"path": format!("{outside}{}x.cube", std::path::MAIN_SEPARATOR)}),
        ),
    )
    .await;
    assert_eq!(
        (r.status, r.code().as_str()),
        (StatusCode::FORBIDDEN, "forbidden_path")
    );
    // roots endpoint lists only allowed roots
    let r = send(&app, get("/api/fs/roots")).await;
    let roots = r.json()["roots"].clone();
    let roots: Vec<String> = roots
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        !roots.iter().any(|r| r.eq_ignore_ascii_case(outside)),
        "{roots:?}"
    );
}

#[tokio::test]
async fn whitelist_allows_configured_roots_and_blocks_traversal() {
    let e = env();
    // a root outside home: the tempdir may be inside home on Windows, so use an explicit one
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    e.store.add_root(root.path()).unwrap();
    let app = strict(&e);
    let rp = root.path().to_string_lossy().into_owned();
    let sep = std::path::MAIN_SEPARATOR;

    let r = send(&app, get(&format!("/api/fs/list?path={}", enc(&rp)))).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert!(r.json()["parent"].is_null() || r.json()["parent"].is_string());
    let sub = format!("{rp}{sep}sub");
    let r = send(&app, get(&format!("/api/fs/list?path={}", enc(&sub)))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.json()["parent"].is_string(),
        "parent inside the root is shown"
    );

    let attempts = [
        format!("{rp}{sep}sub{sep}..{sep}..{sep}"),
        format!("{rp}{sep}..{sep}"),
        format!("{rp}/../.."),
        format!("{rp}{sep}sub{sep}..{sep}sub"),
        format!("{rp}\\..\\.."),
        format!(
            "..{sep}..{sep}{}",
            SYSTEM_DIR.trim_start_matches(['/', '\\'])
        ),
    ];
    for p in attempts {
        let r = send(&app, get(&format!("/api/fs/list?path={}", enc(&p)))).await;
        assert!(
            r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
            "{p:?} -> {}",
            r.status
        );
        assert_ne!(r.status, StatusCode::OK, "{p:?}");
        let r = send(&app, post("/api/import", json!({ "path": p }))).await;
        assert!(
            r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
            "{p:?} -> {}",
            r.status
        );
        let r = send(&app, post("/api/export", json!({"ids": [1], "dest": p}))).await;
        assert!(
            r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
            "{p:?} -> {}",
            r.status
        );
    }
    // percent-encoded `..` is decoded by the query parser and then refused
    let r = send(
        &app,
        get(&format!("/api/fs/list?path={}%2F%2e%2e", enc(&rp))),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // NUL byte
    let r = send(&app, get(&format!("/api/fs/list?path={}%00", enc(&rp)))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // an import inside the root passes the whitelist (and then fails on its own terms)
    let r = send(
        &app,
        post("/api/import", json!({"path": format!("{rp}{sep}missing")})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn whitelist_blocks_windows_device_and_unc_paths() {
    let e = env();
    let app = strict(&e);
    for p in [
        r"\\localhost\c$\Windows",
        r"\\127.0.0.1\c$",
        r"\\?\C:\Windows",
        r"\\?\UNC\localhost\c$",
        r"\\.\PhysicalDrive0",
        "//server/share/photos",
        r"C:\Users\Public\NUL",
        r"C:\Users\Public\photo.jpg:hidden",
        r"C:foo",
        r"\Windows",
    ] {
        let r = send(&app, get(&format!("/api/fs/list?path={}", enc(p)))).await;
        assert_ne!(r.status, StatusCode::OK, "{p}");
        assert!(
            r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
            "{p} -> {}",
            r.status
        );
        let r = send(&app, post("/api/import", json!({"path": p}))).await;
        assert!(
            r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
            "{p} -> {}",
            r.status
        );
        let r = send(&app, post("/api/luts/import", json!({"path": p}))).await;
        assert!(
            r.status == StatusCode::FORBIDDEN || r.status == StatusCode::BAD_REQUEST,
            "{p} -> {}",
            r.status
        );
    }
    // UNC / device forms are 403 (not merely "bad request") even though they are absolute
    for p in [
        r"\\localhost\c$\Windows",
        r"\\?\C:\Windows",
        "//server/share",
    ] {
        let r = send(&app, post("/api/import", json!({"path": p}))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{p}");
        assert_eq!(r.code(), "forbidden_path");
    }
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn whitelist_blocks_symlink_and_junction_escapes() {
    let e = env();
    let root = tempfile::tempdir().unwrap();
    // the target must lie outside every allowed root: use the system directory
    let link = root.path().join("escape");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(SYSTEM_DIR, &link).is_ok();
    #[cfg(windows)]
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(SYSTEM_DIR)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !made {
        eprintln!("cannot create a link here; skipping");
        return;
    }
    e.store.add_root(root.path()).unwrap();
    let app = strict(&e);
    let lp = link.to_string_lossy().into_owned();
    let r = send(&app, get(&format!("/api/fs/list?path={}", enc(&lp)))).await;
    assert_eq!(
        r.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let nested = format!("{lp}{}new", std::path::MAIN_SEPARATOR);
    let r = send(
        &app,
        post("/api/export", json!({"ids": [1], "dest": nested})),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = send(&app, post("/api/import", json!({"path": lp}))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    #[cfg(windows)]
    let _ = std::fs::remove_dir(&link); // remove the junction, not its target
}

#[tokio::test]
async fn imported_roots_become_allowed() {
    let e = env();
    let root = tempfile::tempdir().unwrap();
    let photos = root.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    ip_core::testutil::make_photos(&photos, 1);
    // import through the core (as the CLI does), outside any configured root
    e.core
        .import(ip_core::ImportRequest {
            path: photos.to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    let app = strict(&e);
    let r = send(
        &app,
        get(&format!(
            "/api/fs/list?path={}",
            enc(&photos.to_string_lossy())
        )),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
}

// ------------------------------------------------------------- /system/lan

#[tokio::test]
async fn lan_info_has_urls_and_qr() {
    let e = env();
    let app = e.lan_app(|_| {});
    let owner = login(&app, OWNER_PW).await.cookie().unwrap();
    let r = send(&app, get("/api/system/lan").lan().cookie(&owner)).await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["enabled"], true);
    let urls = v["urls"].as_array().unwrap();
    for u in urls {
        let u = u.as_str().unwrap();
        assert!(u.starts_with("http://") && u.ends_with(":7878"), "{u}");
        assert!(!u.contains("127.0.0.1"));
    }
    if !urls.is_empty() {
        assert!(v["qr_svg"].as_str().unwrap().contains("<svg"));
    }

    let local = e.app(dev_opts());
    let v = send(&local, get("/api/system/lan")).await.json();
    assert_eq!(v["enabled"], false);
    assert_eq!(v["urls"], json!([]));
    assert!(v["qr_svg"].is_null());
}

// ------------------------------------------------------------------ spawn / WS

#[tokio::test]
async fn lan_mode_needs_a_password_and_binds_all_interfaces() {
    let e = env();
    let cfg = ServerConfig {
        port: 0,
        lan: true,
        ..Default::default()
    };
    let err = spawn_with_core(e.core.clone(), &cfg)
        .await
        .err()
        .expect("must refuse");
    assert!(format!("{err:#}").contains("password"), "{err:#}");
    // a non-loopback host without LAN mode is refused too
    let cfg = ServerConfig {
        host: "0.0.0.0".into(),
        port: 0,
        ..Default::default()
    };
    assert!(spawn_with_core(e.core.clone(), &cfg).await.is_err());
    // with a password it binds 0.0.0.0
    let cfg = ServerConfig {
        port: 0,
        lan: true,
        password: Some(OWNER_PW.into()),
        ..Default::default()
    };
    let server = spawn_with_core(e.core.clone(), &cfg).await.unwrap();
    assert!(server.addr.ip().is_unspecified(), "{}", server.addr);
    assert!(server.security.verify_owner(OWNER_PW));
    server.shutdown().await.unwrap();
    // `lan.enabled` in settings without a password stays local instead of failing
    let e2 = env();
    e2.store.set_passwords(OWNER_PW, None).unwrap();
    e2.store
        .set_lan(LanSettings {
            enabled: true,
            port: 7878,
            guest_enabled: false,
        })
        .unwrap();
    let cfg = ServerConfig {
        port: 0,
        ..Default::default()
    };
    let s = spawn_with_core(e2.core.clone(), &cfg).await.unwrap();
    assert!(s.addr.ip().is_unspecified());
    s.shutdown().await.unwrap();
    let _ = &e.data;
}

#[tokio::test]
async fn websocket_requires_auth() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Error;
    let e = env();
    let cfg = ServerConfig {
        port: 0,
        session_token: Some(TOKEN.into()),
        ..Default::default()
    };
    let server = spawn_with_core(e.core.clone(), &cfg).await.unwrap();
    let base = format!("ws://{}/api/events", server.addr);

    // no credentials
    match tokio_tungstenite::connect_async(&base).await {
        Err(Error::Http(resp)) => assert_eq!(resp.status(), 401),
        other => panic!("expected 401, got {:?}", other.map(|_| ())),
    }
    // wrong token
    match tokio_tungstenite::connect_async(format!("{base}?token=bad")).await {
        Err(Error::Http(resp)) => assert_eq!(resp.status(), 401),
        other => panic!("expected 401, got {:?}", other.map(|_| ())),
    }
    // ?token=
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("{base}?token={TOKEN}"))
        .await
        .expect("token query");
    let _ = ws.close(None).await;
    // bearer
    let mut req = base.as_str().into_client_request().unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("bearer");
    let _ = ws.close(None).await;
    // cookie from the token exchange
    let client_get = |path: String| async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            server.addr
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    };
    let resp = client_get(format!("/?token={TOKEN}")).await;
    assert!(resp.starts_with("HTTP/1.1 303"), "{resp}");
    let cookie = resp
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().split(';').next().unwrap().to_string())
        .expect("cookie");
    let mut req = base.as_str().into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("cookie");
    let _ = ws.close(None).await;
    // cross-site WebSocket hijacking: valid cookie but a foreign Origin
    let mut req = base.as_str().into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req.headers_mut()
        .insert("origin", "http://evil.example".parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Err(Error::Http(resp)) => assert_eq!(resp.status(), 403),
        other => panic!("expected 403, got {:?}", other.map(|_| ())),
    }
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn plain_http_peer_is_loopback_in_dev_mode_over_tcp() {
    let e = env();
    let cfg = ServerConfig {
        port: 0,
        ..Default::default()
    };
    let server = spawn_with_core(e.core.clone(), &cfg).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    s.write_all(
        format!(
            "GET /api/sessions HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            server.addr
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    // the same request with a rebinding Host is refused
    let mut s = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    s.write_all(b"GET /api/sessions HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 401"), "{out}");
    server.shutdown().await.unwrap();
}
