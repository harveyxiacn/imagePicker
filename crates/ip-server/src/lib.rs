//! HTTP + WebSocket API (axum). Contract: docs/api-contract-m1.md.

pub mod error;
pub mod fs;
pub mod routes;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::routing::{get, post};
use axum::Router;
use ip_core::{Core, CoreConfig};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

pub use routes::AppState;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    /// `0` binds an ephemeral port.
    pub port: u16,
    pub data_dir: Option<PathBuf>,
    pub web_dir: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 7878,
            data_dir: None,
            web_dir: None,
        }
    }
}

/// The full application router (`/api/*` plus SPA fallback).
pub fn build_router(core: Arc<Core>, web_dir: Option<PathBuf>) -> Router {
    let state = AppState { core, web_dir };
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
        .route("/events", get(ws::events))
        .fallback(routes::api_not_found)
        .method_not_allowed_fallback(routes::method_not_allowed);
    Router::new()
        .nest("/api", api)
        .fallback(routes::fallback)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// A server running in the background (used by the Tauri shell and tests).
pub struct RunningServer {
    pub addr: SocketAddr,
    pub core: Arc<Core>,
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
    let addr = listener.local_addr()?;
    let app = build_router(core.clone(), web_dir);
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
    });
    Ok(RunningServer {
        addr,
        core,
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
    let listener = TcpListener::bind((config.host.as_str(), config.port))
        .await
        .with_context(|| format!("bind {}:{}", config.host, config.port))?;
    serve_listener(listener, core, config.web_dir.clone())
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
