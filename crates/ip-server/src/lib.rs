//! HTTP + WebSocket API (axum). Contracts: docs/api-contract-m1.md .. m3.md.

pub mod auth;
pub mod error;
pub mod fs;
pub mod m2;
pub mod m3;
pub mod m4;
pub mod m5;
pub mod m6;
pub mod routes;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use ip_core::{Core, CoreConfig};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub use auth::{AuthState, ServerOptions};
pub use ip_core::auth::SecurityStore;
pub use routes::AppState;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    /// `0` binds an ephemeral port.
    pub port: u16,
    pub data_dir: Option<PathBuf>,
    pub web_dir: Option<PathBuf>,
    /// Desktop session token. `None` = development mode: loopback clients need no credentials.
    pub session_token: Option<String>,
    /// LAN mode (bind `0.0.0.0`, password login). Requires a password in the security store.
    /// Also enabled by `lan.enabled` in `<data_dir>/security.json`.
    pub lan: bool,
    /// Allow the Vite dev origins through CORS (also: env `IMAGEPICKER_DEV_CORS=1`).
    pub dev_cors: bool,
    /// Extra allowed filesystem roots (in addition to home, Pictures, removable drives,
    /// imported roots and `security.json` roots).
    pub extra_roots: Vec<PathBuf>,
    /// Set (or change) the owner password in the security store before starting.
    pub password: Option<String>,
    /// Set the guest password together with `password` (`Some("")` removes it).
    pub guest_password: Option<String>,
}

impl ServerConfig {
    fn options(&self, lan: bool) -> ServerOptions {
        ServerOptions {
            session_token: self.session_token.clone(),
            lan,
            dev_cors: self.dev_cors || ServerOptions::default().dev_cors,
            extra_roots: self.extra_roots.clone(),
            ..ServerOptions::default()
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 7878,
            data_dir: None,
            web_dir: None,
            session_token: None,
            lan: false,
            dev_cors: false,
            extra_roots: Vec::new(),
            password: None,
            guest_password: None,
        }
    }
}

/// The router in development mode (no token, loopback only) for tests and embedding. The
/// system temp dir is an additional allowed root here so tests can import temp folders on every
/// OS; the production entry points (`spawn*`, `serve_listener_with`) never add it.
pub fn build_router(core: Arc<Core>, web_dir: Option<PathBuf>) -> Router {
    let opts = ServerOptions {
        extra_roots: vec![std::env::temp_dir()],
        ..ServerOptions::default()
    };
    build_router_with(core, web_dir, opts)
}

pub fn build_router_with(core: Arc<Core>, web_dir: Option<PathBuf>, opts: ServerOptions) -> Router {
    let auth = AuthState::new(&core, opts);
    build_router_auth(core, web_dir, auth)
}

/// The full application router (`/api/*` plus SPA fallback) behind the auth middleware.
pub fn build_router_auth(
    core: Arc<Core>,
    web_dir: Option<PathBuf>,
    auth: Arc<AuthState>,
) -> Router {
    let dev_cors = auth.dev_cors();
    let state = AppState {
        core,
        web_dir,
        auth: auth.clone(),
    };
    let api = Router::new()
        .route("/health", get(routes::health))
        .route("/import", post(routes::import))
        .route("/sessions", get(routes::list_sessions))
        .route(
            "/sessions/{id}",
            get(routes::get_session).delete(routes::delete_session),
        )
        .route(
            "/photos",
            get(routes::list_photos).patch(routes::patch_photos),
        )
        .route("/photos/{id}", get(routes::get_photo))
        .route("/thumb/{id}", get(routes::thumb))
        .route("/preview/{id}", get(routes::preview))
        .route("/original/{id}", get(routes::original))
        .route("/viewport", post(routes::viewport))
        .route("/export", post(routes::export))
        .route("/fs/roots", get(fs::get_roots))
        .route("/fs/list", get(fs::get_list))
        .route("/system/hardware", get(m2::hardware))
        .route("/models", get(m2::models))
        .route("/models/ensure", post(m2::models_ensure))
        .route("/analysis/run", post(m2::analysis_run))
        .route("/analysis/cancel", post(m2::analysis_cancel))
        .route("/analysis/status", get(m2::analysis_status))
        .route("/photos/{id}/analysis", get(m2::photo_analysis))
        .route("/photos/accept-ai", post(m2::accept_ai))
        .route("/groups", get(m2::groups))
        .route("/groups/split", post(m2::groups_split))
        .route("/groups/merge", post(m2::groups_merge))
        .route("/bursts/{id}/faces", get(m2::burst_faces))
        .route("/faces/{id}/crop", get(m2::face_crop))
        .route("/faces/{id}/person", post(m2::set_face_person))
        .route("/people", get(m2::people))
        .route("/people/merge", post(m2::merge_people))
        .route("/people/{id}", patch(m2::patch_person))
        .route(
            "/edits/{id}",
            get(m3::get_edit).put(m3::put_edit).delete(m3::delete_edit),
        )
        .route("/edits/{id}/auto", post(m3::auto_edit))
        .route("/edits/sync", post(m3::sync_edits))
        .route("/render/preview", post(m3::render_preview))
        .route("/presets", get(m3::list_presets).post(m3::create_preset))
        .route("/presets/{id}", delete(m3::delete_preset))
        .route("/luts", get(m3::list_luts))
        .route("/luts/import", post(m3::import_lut))
        .route("/masks/{id}", get(m3::mask))
        .route("/photos/{id}/people", get(m4::photo_people))
        .route("/photos/{id}/beauty/prepare", post(m4::beauty_prepare))
        .route(
            "/people/{id}/beauty-profile",
            get(m4::get_beauty_profile).put(m4::put_beauty_profile),
        )
        .route("/edits/apply-profiles", post(m4::apply_profiles))
        .route("/people/best", get(m4::people_best))
        .route(
            "/faces/search",
            post(m4::faces_search).layer(DefaultBodyLimit::max(m4::SEARCH_BODY_LIMIT)),
        )
        .route(
            "/collections",
            get(m4::list_collections).post(m4::create_collection),
        )
        .route(
            "/collections/{id}",
            patch(m4::patch_collection).delete(m4::delete_collection),
        )
        .route("/bursts/{id}/besttake", get(m5::besttake_plan))
        .route("/bursts/{id}/besttake/auto", post(m5::besttake_auto))
        .route("/besttake", post(m5::besttake))
        .route("/photos/{id}/bystanders", get(m5::bystanders))
        .route("/photos/{id}/inpaint", post(m5::inpaint))
        .route("/photos/{id}/enhance", post(m5::enhance))
        .route("/assets/{photo_id}/{asset}", get(m5::asset))
        .route("/taste", get(m4::taste))
        .route("/taste/reset", post(m4::taste_reset))
        .route("/assistant/status", get(m6::assistant_status))
        .route("/assistant/plan", post(m6::assistant_plan))
        .route("/assistant/execute", post(m6::assistant_execute))
        .route("/assistant/describe", post(m6::assistant_describe))
        .route("/assistant/suggest", post(m6::assistant_suggest))
        .route("/settings", get(m6::get_settings).patch(m6::patch_settings))
        .route("/cache", get(m6::get_cache))
        .route("/cache/clear", post(m6::clear_cache))
        .route("/models/{id}", delete(m6::delete_model))
        .route("/onboarding", get(m6::onboarding))
        .route("/onboarding/done", post(m6::onboarding_done))
        .route("/faces", delete(m6::clear_faces))
        .route("/xmp/sync", post(m6::xmp_sync))
        .route("/photos/{id}/tags", get(m6::photo_tags))
        .route("/photos/tags", post(m6::set_tags))
        .route("/events", get(ws::events))
        .merge(auth::routes())
        .fallback(routes::api_not_found)
        .method_not_allowed_fallback(routes::method_not_allowed);
    // Same-origin only: without the dev flag the allow-list is empty, so no
    // `Access-Control-Allow-Origin` header is ever emitted.
    let origins: Vec<axum::http::HeaderValue> = if dev_cors {
        auth::DEV_ORIGINS
            .iter()
            .map(|o| axum::http::HeaderValue::from_static(o))
            .collect()
    } else {
        Vec::new()
    };
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(dev_cors)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ]);
    Router::new()
        .nest("/api", api)
        .fallback(routes::fallback)
        .layer(axum::middleware::from_fn(m5::tidy_json))
        .layer(axum::middleware::from_fn_with_state(auth, auth::middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// A server running in the background (used by the Tauri shell and tests).
pub struct RunningServer {
    pub addr: SocketAddr,
    pub core: Arc<Core>,
    /// Persisted LAN / password / roots settings (shared file with the settings API).
    pub security: SecurityStore,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl RunningServer {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Graceful shutdown (open WebSocket connections are dropped).
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.task.abort();
        let _ = self.task.await;
        Ok(())
    }

    /// Waits until the server task ends (it only ends on error).
    pub async fn wait(self) -> anyhow::Result<()> {
        self.task.await??;
        Ok(())
    }
}

/// Serve `core` on an already bound listener.
pub fn serve_listener(
    listener: TcpListener,
    core: Arc<Core>,
    web_dir: Option<PathBuf>,
) -> anyhow::Result<RunningServer> {
    serve_listener_with(listener, core, web_dir, ServerOptions::default())
}

pub fn serve_listener_with(
    listener: TcpListener,
    core: Arc<Core>,
    web_dir: Option<PathBuf>,
    opts: ServerOptions,
) -> anyhow::Result<RunningServer> {
    let addr = listener.local_addr()?;
    let auth = AuthState::new(&core, opts);
    auth.set_port(addr.port());
    let security = auth.store().clone();
    let app = build_router_auth(core.clone(), web_dir, auth);
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = rx.await;
        })
        .await
    });
    Ok(RunningServer {
        addr,
        core,
        security,
        shutdown: Some(tx),
        task,
    })
}

/// Opens the core with real imaging, binds `host:port` (port 0 = ephemeral) and serves in the
/// background. The returned `addr` is the actual bound address.
pub async fn spawn(config: ServerConfig) -> anyhow::Result<RunningServer> {
    let core = Core::open(CoreConfig::new(config.data_dir.clone())).context("open catalog")?;
    spawn_with_core(core, &config).await
}

pub async fn spawn_with_core(
    core: Arc<Core>,
    config: &ServerConfig,
) -> anyhow::Result<RunningServer> {
    let store = SecurityStore::open(core.data_dir());
    if let Some(pw) = &config.password {
        store
            .set_passwords(pw, config.guest_password.as_deref())
            .context("set password")?;
        if let Some(g) = &config.guest_password {
            let mut lan = store.lan();
            lan.guest_enabled = !g.is_empty();
            store.set_lan(lan).context("guest access")?;
        }
    }
    let snap = store.snapshot();
    let mut lan = config.lan || snap.lan.enabled;
    if lan && !snap.has_owner_password() {
        anyhow::ensure!(
            !config.lan,
            "LAN mode requires a password: set one first (imagepicker serve --lan --password <p>)"
        );
        tracing::warn!("LAN access is enabled in settings but no password is set; staying local");
        lan = false;
    }
    let loopback_host = matches!(config.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    anyhow::ensure!(
        lan || loopback_host,
        "binding to {} exposes the server to the network: use LAN mode (--lan) with a password",
        config.host
    );
    let (host, port) = if lan {
        let host = if loopback_host {
            "0.0.0.0"
        } else {
            config.host.as_str()
        };
        let port = if config.port == 0 {
            snap.lan.port
        } else {
            config.port
        };
        (host.to_string(), port)
    } else {
        (config.host.clone(), config.port)
    };
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("bind {host}:{port}"))?;
    serve_listener_with(listener, core, config.web_dir.clone(), config.options(lan))
}

/// Runs until Ctrl-C (or a fatal server error).
pub async fn run(config: ServerConfig) -> anyhow::Result<()> {
    let server = spawn(config).await?;
    tracing::info!(addr = %server.addr, data_dir = %server.core.data_dir().display(), "imagepicker listening on http://{}", server.addr);
    let addr = server.addr;
    tokio::select! {
        res = server.wait() => res,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!(%addr, "shutting down");
            Ok(())
        }
    }
}
