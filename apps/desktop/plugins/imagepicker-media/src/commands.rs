use tauri::{command, AppHandle, Runtime};

use crate::models::*;
use crate::{MediaExt, Result};

#[command]
pub(crate) async fn request_permission<R: Runtime>(app: AppHandle<R>) -> Result<PermissionState> {
    app.imagepicker_media().request_permission()
}

#[command]
pub(crate) async fn list_albums<R: Runtime>(app: AppHandle<R>) -> Result<AlbumList> {
    app.imagepicker_media().list_albums()
}

#[command]
pub(crate) async fn share<R: Runtime>(
    app: AppHandle<R>,
    paths: Vec<String>,
) -> Result<ShareResult> {
    app.imagepicker_media().share(paths)
}

#[command]
pub(crate) async fn trash<R: Runtime>(
    app: AppHandle<R>,
    paths: Vec<String>,
) -> Result<TrashResult> {
    app.imagepicker_media().trash(paths)
}

#[command]
pub(crate) async fn start_foreground<R: Runtime>(
    app: AppHandle<R>,
    title: Option<String>,
    text: Option<String>,
    progress: Option<u8>,
) -> Result<()> {
    app.imagepicker_media().start_foreground(ForegroundArgs {
        title: title.unwrap_or_default(),
        text: text.unwrap_or_default(),
        progress,
    })
}

#[command]
pub(crate) async fn stop_foreground<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    app.imagepicker_media().stop_foreground()
}

#[command]
pub(crate) async fn keep_awake<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<()> {
    app.imagepicker_media().keep_awake(on)
}

#[command]
pub(crate) async fn publish_exports<R: Runtime>(
    app: AppHandle<R>,
    paths: Vec<String>,
) -> Result<PublishResult> {
    app.imagepicker_media().publish_exports(paths)
}
