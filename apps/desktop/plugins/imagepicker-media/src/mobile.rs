use serde::de::DeserializeOwned;
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::{AppHandle, Runtime};

use crate::models::*;
use crate::Result;

const PLUGIN_IDENTIFIER: &str = "app.tauri.imagepickermedia";

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<ImagepickerMedia<R>> {
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "ImagePickerMediaPlugin")?;
    Ok(ImagepickerMedia(handle))
}

/// Bridge to the Kotlin `ImagePickerMediaPlugin`.
pub struct ImagepickerMedia<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> ImagepickerMedia<R> {
    pub fn request_permission(&self) -> Result<PermissionState> {
        Ok(self.0.run_mobile_plugin("requestPermission", ())?)
    }
    pub fn list_albums(&self) -> Result<AlbumList> {
        Ok(self.0.run_mobile_plugin("listAlbums", ())?)
    }
    pub fn share(&self, paths: Vec<String>) -> Result<ShareResult> {
        Ok(self.0.run_mobile_plugin("share", PathsArgs { paths })?)
    }
    pub fn trash(&self, paths: Vec<String>) -> Result<TrashResult> {
        Ok(self.0.run_mobile_plugin("trash", PathsArgs { paths })?)
    }
    pub fn start_foreground(&self, args: ForegroundArgs) -> Result<()> {
        Ok(self.0.run_mobile_plugin("startForeground", args)?)
    }
    pub fn stop_foreground(&self) -> Result<()> {
        Ok(self.0.run_mobile_plugin("stopForeground", ())?)
    }
    pub fn keep_awake(&self, on: bool) -> Result<()> {
        Ok(self
            .0
            .run_mobile_plugin("keepAwake", KeepAwakeArgs { on })?)
    }
    pub fn publish_exports(&self, paths: Vec<String>) -> Result<PublishResult> {
        Ok(self
            .0
            .run_mobile_plugin("publishExports", PathsArgs { paths })?)
    }
}
