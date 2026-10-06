use serde::de::DeserializeOwned;
use tauri::plugin::PluginApi;
use tauri::{AppHandle, Runtime};

use crate::models::*;
use crate::{Error, Result};

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> Result<ImagepickerMedia<R>> {
    Ok(ImagepickerMedia(std::marker::PhantomData))
}

/// Desktop stub: every command reports `Unsupported`.
pub struct ImagepickerMedia<R: Runtime>(std::marker::PhantomData<fn() -> R>);

impl<R: Runtime> ImagepickerMedia<R> {
    pub fn request_permission(&self) -> Result<PermissionState> {
        Err(Error::Unsupported)
    }
    pub fn list_albums(&self) -> Result<AlbumList> {
        Err(Error::Unsupported)
    }
    pub fn share(&self, _paths: Vec<String>) -> Result<ShareResult> {
        Err(Error::Unsupported)
    }
    pub fn trash(&self, _paths: Vec<String>) -> Result<TrashResult> {
        Err(Error::Unsupported)
    }
    pub fn start_foreground(&self, _args: ForegroundArgs) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn stop_foreground(&self) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn keep_awake(&self, _on: bool) -> Result<()> {
        Err(Error::Unsupported)
    }
    pub fn publish_exports(&self, _paths: Vec<String>) -> Result<PublishResult> {
        Err(Error::Unsupported)
    }
}
