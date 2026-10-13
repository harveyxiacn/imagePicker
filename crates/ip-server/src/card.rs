//! `GET /api/import/sources` and `POST /api/import/card` (PRD F1.8, `docs/api-contract-m1.md`
//! "从存储卡导入"). The server only lists cards; a copy starts when the client asks for it.

use std::path::Path;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use ip_core::card::{card_sources, DEFAULT_PATTERN};
use ip_core::CardImportRequest;
use serde_json::{json, Value};

use crate::error::{ApiError, ApiJson, ApiResult};
use crate::routes::{whitelisted, AppState};

async fn detected() -> ApiResult<Vec<ip_core::CardSource>> {
    tokio::task::spawn_blocking(card_sources)
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("card detection failed: {e}"),
            )
        })
}

/// `GET /api/import/sources`: removable volumes that carry a `DCIM` folder.
pub async fn sources() -> ApiResult<Json<Value>> {
    Ok(Json(json!({
        "sources": detected().await?,
        "default_pattern": DEFAULT_PATTERN,
    })))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `POST /api/import/card`: copies the new photos of a card into `dest` and imports them. The
/// source must be a card listed by `/api/import/sources` right now or a folder inside the allowed
/// roots; the destination must be inside the allowed roots. 202 `{task_id}`.
pub async fn import_card(
    State(st): State<AppState>,
    ApiJson(mut req): ApiJson<CardImportRequest>,
) -> ApiResult<Response> {
    if !req.source.trim().is_empty() {
        let given = req.source.clone();
        let list = detected().await?;
        let card = list
            .iter()
            .find(|s| same_path(Path::new(&s.path), Path::new(given.trim())));
        req.source = match card {
            Some(s) => s.path.clone(),
            None => whitelisted(&st, &given).await?,
        };
    }
    if !req.dest.trim().is_empty() {
        req.dest = whitelisted(&st, &req.dest).await?;
    }
    let task_id = st.core.card_import(req).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))).into_response())
}
