//! Server-side directory browser for the WebUI folder picker.

use std::path::{Path, PathBuf};

use axum::Json;
use ip_imaging::ImageFormat;
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiQuery, ApiResult};

#[derive(Serialize)]
pub struct Roots {
    pub roots: Vec<String>,
}

pub fn roots() -> Vec<String> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(h) = dirs::home_dir() {
        out.push(h);
    }
    if let Some(p) = dirs::picture_dir() {
        if p.is_dir() {
            out.push(p);
        }
    }
    #[cfg(windows)]
    for letter in b'C'..=b'Z' {
        let p = PathBuf::from(format!("{}:\\", letter as char));
        if p.exists() {
            out.push(p);
        }
    }
    #[cfg(not(windows))]
    out.push(PathBuf::from("/"));
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

pub async fn get_roots() -> Json<Roots> {
    Json(Roots {
        roots: tokio::task::spawn_blocking(roots).await.unwrap_or_default(),
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

pub async fn get_list(ApiQuery(q): ApiQuery<ListParams>) -> ApiResult<Json<Listing>> {
    let path = match q.path.filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => dirs::home_dir().ok_or_else(|| ApiError::bad_request("path is required"))?,
    };
    if !path.is_absolute() {
        return Err(ApiError::bad_request("path must be absolute"));
    }
    let listing = tokio::task::spawn_blocking(move || {
        let path: PathBuf = path.components().collect(); // drop trailing separators / `.`
        list_dir(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found(format!("directory not found: {}", path.display()))
            } else {
                ApiError::bad_request(format!("cannot read directory {}: {e}", path.display()))
            }
        })
    })
    .await
    .map_err(|e| {
        ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            e.to_string(),
        )
    })??;
    Ok(Json(listing))
}
