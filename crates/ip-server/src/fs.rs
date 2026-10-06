//! Server-side directory browser for the WebUI folder picker.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use ip_imaging::ImageFormat;
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiQuery, ApiResult};
use crate::routes::AppState;

#[derive(Serialize)]
pub struct Roots {
    pub roots: Vec<String>,
}

pub async fn get_roots(State(st): State<AppState>) -> Json<Roots> {
    let roots = st.auth.root_set(&st.core).await;
    Json(Roots {
        roots: roots
            .display()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    })
}

#[derive(Deserialize)]
pub struct ListParams {
    pub path: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub dirs: Vec<String>,
    pub image_count: usize,
}

pub fn list_dir(path: &Path) -> std::io::Result<Listing> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut image_count = 0;
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let p = entry.path();
        if ft.is_dir() {
            dirs.push(p);
        } else if ImageFormat::from_path(&p).is_some() {
            image_count += 1;
        }
    }
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    Ok(Listing {
        path: path.to_string_lossy().into_owned(),
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
        dirs: dirs
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        image_count,
    })
}

pub async fn get_list(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<ListParams>,
) -> ApiResult<Json<Listing>> {
    let raw = match q.path.filter(|p| !p.trim().is_empty()) {
        Some(p) => p,
        // no home directory on Android: start in the first shared-storage root
        None => dirs::home_dir()
            .or_else(|| ip_core::roots::default_roots().into_iter().next())
            .ok_or_else(|| ApiError::bad_request("path is required"))?
            .to_string_lossy()
            .into_owned(),
    };
    if !Path::new(&raw).is_absolute() {
        return Err(ApiError::bad_request("path must be absolute"));
    }
    let roots = st.auth.root_set(&st.core).await;
    let listing = tokio::task::spawn_blocking(move || {
        let path = roots
            .check(&raw)
            .map_err(|e| ApiError::new(StatusCode::FORBIDDEN, "forbidden_path", e.0))?;
        let mut listing = list_dir(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found(format!("directory not found: {}", path.display()))
            } else {
                ApiError::bad_request(format!("cannot read directory {}: {e}", path.display()))
            }
        })?;
        // "Up" never leaves the whitelist.
        listing.parent = roots
            .parent_within(&path)
            .map(|p| p.to_string_lossy().into_owned());
        Ok::<_, ApiError>(listing)
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))??;
    Ok(Json(listing))
}
