//! imagePicker desktop shell.
//!
//! Architecture (docs/02-架构设计.md §3): the SPA always talks HTTP/WS to the embedded Rust
//! server. Tauri only provides the window, native dialogs, the menu and OS drag-and-drop paths.
//! The main window is therefore opened at the *server URL*, so the SPA uses relative `/api`
//! exactly like the WebUI does.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use ip_server::{RunningServer, ServerConfig};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::window::Color;
use tauri::{
    AppHandle, DragDropEvent, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};
use tauri_plugin_dialog::DialogExt;

const REPO_URL: &str = "https://github.com/harveyxiacn/imagePicker";
/// Matches `--bg` of the SPA dark theme, so there is no white flash before first paint.
const BG: (u8, u8, u8) = (0x1e, 0x1e, 0x1e);
/// `web/vite.config.ts` proxies `/api` to this port in dev.
const DEV_API_PORT: u16 = 7878;

struct ServerHandle(Mutex<Option<RunningServer>>);

/// A folder the user chose through a native dialog or by dropping it on the window is allowed
/// for the API's path whitelist (import / export / folder browser), even outside home or the
/// removable drives.
fn allow_root(app: &AppHandle, path: &Path) {
    if let Some(h) = app.try_state::<ServerHandle>() {
        if let Ok(server) = h.0.lock() {
            if let Some(s) = server.as_ref() {
                if let Err(e) = s.security.add_root(path) {
                    tracing::warn!(error = %e, "could not remember the chosen folder");
                }
            }
        }
    }
}

/// Native folder picker. `None` = cancelled.
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |p| {
        let _ = tx.send(p);
    });
    let picked = rx.await.ok().flatten()?;
    let path = picked.into_path().ok()?;
    allow_root(&app, &path);
    Some(path.to_string_lossy().into_owned())
}

/// "Reveal in Explorer/Finder" for a photo path (selects the file in its folder).
#[tauri::command]
fn reveal_in_folder(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("path does not exist: {path}"));
    }
    tauri_plugin_opener::reveal_item_in_dir(&p).map_err(|e| e.to_string())
}

/// Tell the SPA to import `path`. From any page other than Home the SPA is first navigated
/// home; the path is handed over via sessionStorage (picked up in `web/src/platform`).
fn dispatch_import(win: &WebviewWindow, path: &Path) {
    let p = serde_json::to_string(&path.to_string_lossy()).unwrap_or_else(|_| "\"\"".into());
    let js = format!(
        "(function(p){{if(location.pathname!=='/'){{try{{sessionStorage.setItem('ip-pending-import',p)}}catch(e){{}}location.assign('/')}}else{{window.dispatchEvent(new CustomEvent('ip-native-import',{{detail:p}}))}}}})({p})"
    );
    let _ = win.eval(js);
}

fn dispatch_drag(win: &WebviewWindow, over: bool) {
    let _ = win.eval(format!(
        "window.dispatchEvent(new CustomEvent('ip-native-drag',{{detail:{over}}}))"
    ));
}

fn focus_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let import = MenuItem::with_id(app, "import", "Import Folder…", true, Some("CmdOrCtrl+O"))?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &import,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let fullscreen = MenuItem::with_id(app, "fullscreen", "Toggle Fullscreen", true, Some("F11"))?;
    let view = Submenu::with_items(app, "View", true, &[&fullscreen])?;
    let github = MenuItem::with_id(app, "github", "GitHub", true, None::<&str>)?;
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About imagePicker"), None)?,
            &github,
        ],
    )?;
    #[cfg(target_os = "macos")]
    {
        let edit = Submenu::with_items(
            app,
            "Edit",
            true,
            &[
                &PredefinedMenuItem::undo(app, None)?,
                &PredefinedMenuItem::redo(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::cut(app, None)?,
                &PredefinedMenuItem::copy(app, None)?,
                &PredefinedMenuItem::paste(app, None)?,
                &PredefinedMenuItem::select_all(app, None)?,
            ],
        )?;
        let appm = Submenu::with_items(
            app,
            "imagePicker",
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?;
        return Menu::with_items(app, &[&appm, &file, &edit, &view, &help]);
    }
    #[cfg(not(target_os = "macos"))]
    Menu::with_items(app, &[&file, &view, &help])
}

/// Per-launch secret (256 bit). The server only accepts API calls that carry it: the window is
/// opened once at `/?token=<token>`, the server answers with an HttpOnly SameSite=Strict cookie
/// and a redirect that strips the token, so the SPA (fetch, `<img>`, WebSocket) is authenticated
/// by the cookie and the token never reaches page JavaScript.
fn session_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("os rng");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn start_server(app: &AppHandle, token: &str) -> anyhow::Result<(RunningServer, String)> {
    let dev = tauri::is_dev();
    let web_dir = if dev {
        None // Vite serves the SPA; it proxies /api to DEV_API_PORT.
    } else {
        let dir = app
            .path()
            .resource_dir()
            .context("resource dir")?
            .join("web");
        anyhow::ensure!(
            dir.join("index.html").exists(),
            "bundled SPA missing at {}",
            dir.display()
        );
        Some(dir)
    };
    // M7: the AI runtime installer needs the bundled `uv` (Tauri `externalBin`, installed next to
    // the executable) and the bundled worker source (resources). Both are absent in `tauri dev`.
    let uv_path = std::env::current_exe().ok().and_then(|exe| {
        let p = exe
            .parent()?
            .join(if cfg!(windows) { "uv.exe" } else { "uv" });
        p.is_file().then_some(p)
    });
    let worker_src = app
        .path()
        .resource_dir()
        .ok()
        .map(|d| d.join("ai-worker"))
        .filter(|d| d.join("pyproject.toml").is_file());
    tracing::info!(uv = ?uv_path, worker_src = ?worker_src, "bundled AI runtime installer");
    let mut cfg = ServerConfig {
        uv_path,
        worker_src,
        host: "127.0.0.1".into(),
        port: if dev { DEV_API_PORT } else { 0 },
        data_dir: std::env::var_os("IMAGEPICKER_DATA_DIR").map(PathBuf::from),
        web_dir,
        // Dev: the SPA comes from Vite (another origin that cannot do the token exchange), so
        // the server stays in loopback-only development mode and allows the Vite origins.
        session_token: (!dev).then(|| token.to_string()),
        dev_cors: dev,
        // LAN access (if enabled in settings) is picked up from `security.json` by the server.
        ..ServerConfig::default()
    };
    let server = match tauri::async_runtime::block_on(ip_server::spawn(cfg.clone())) {
        Ok(s) => s,
        Err(e) if dev => {
            tracing::warn!(
                "dev API port {DEV_API_PORT} unavailable ({e:#}); using an ephemeral port"
            );
            cfg.port = 0;
            tauri::async_runtime::block_on(ip_server::spawn(cfg))?
        }
        Err(e) => return Err(e),
    };
    tracing::info!(addr = %server.addr, "embedded server started");
    let url = if dev {
        app.config()
            .build
            .dev_url
            .as_ref()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "http://localhost:5173".into())
    } else {
        format!("http://127.0.0.1:{}/?token={token}", server.addr.port())
    };
    Ok((server, url))
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ip_server=info".into()),
        )
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            focus_main(app)
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![pick_folder, reveal_in_folder])
        .setup(|app| {
            let handle = app.handle().clone();
            let token = session_token();
            let (server, url) = start_server(&handle, &token)?;
            app.manage(ServerHandle(Mutex::new(Some(server))));

            app.set_menu(build_menu(&handle)?)?;
            app.on_menu_event(|app, ev| match ev.id().as_ref() {
                "import" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(p) = pick_folder(app.clone()).await {
                            if let Some(w) = app.get_webview_window("main") {
                                dispatch_import(&w, Path::new(&p));
                            }
                        }
                    });
                }
                "fullscreen" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let on = w.is_fullscreen().unwrap_or(false);
                        let _ = w.set_fullscreen(!on);
                    }
                }
                "github" => {
                    use tauri_plugin_opener::OpenerExt;
                    let _ = app.opener().open_url(REPO_URL, None::<&str>);
                }
                _ => {}
            });

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("imagePicker")
                .inner_size(1360.0, 860.0)
                .min_inner_size(1024.0, 700.0)
                .background_color(Color(BG.0, BG.1, BG.2, 255))
                .build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            let WindowEvent::DragDrop(dd) = event else {
                return;
            };
            let Some(win) = window.app_handle().get_webview_window(window.label()) else {
                return;
            };
            match dd {
                DragDropEvent::Enter { .. } => dispatch_drag(&win, true),
                DragDropEvent::Leave => dispatch_drag(&win, false),
                DragDropEvent::Drop { paths, .. } => {
                    dispatch_drag(&win, false);
                    // A folder is imported as-is; for loose files, import their folder.
                    let target =
                        paths.iter().find(|p| p.is_dir()).cloned().or_else(|| {
                            paths.iter().find_map(|p| p.parent().map(Path::to_path_buf))
                        });
                    if let Some(t) = target {
                        allow_root(window.app_handle(), &t);
                        dispatch_import(&win, &t);
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build the imagePicker app");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            if let Some(h) = app.try_state::<ServerHandle>() {
                if let Some(server) = h.0.lock().ok().and_then(|mut g| g.take()) {
                    let _ = tauri::async_runtime::block_on(server.shutdown());
                    tracing::info!("embedded server stopped");
                }
            }
        }
    });
}
