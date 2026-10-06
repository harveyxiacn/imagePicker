//! imagePicker Android media plugin (contract: docs/api-contract-m8.md section A).
//!
//! Commands are invoked from the SPA as `plugin:imagepicker-media|<command>`. All of them are
//! implemented in Kotlin (`android/`); on desktop the plugin is a stub and the app does not
//! register it.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};

mod commands;
mod error;
pub mod models;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

#[cfg(desktop)]
pub use desktop::ImagepickerMedia;
pub use error::{Error, Result};
#[cfg(mobile)]
pub use mobile::ImagepickerMedia;

/// Access to the plugin from Rust: `app.imagepicker_media().list_albums()`.
pub trait MediaExt<R: Runtime> {
    fn imagepicker_media(&self) -> &ImagepickerMedia<R>;
}

impl<R: Runtime, T: Manager<R>> MediaExt<R> for T {
    fn imagepicker_media(&self) -> &ImagepickerMedia<R> {
        self.state::<ImagepickerMedia<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("imagepicker-media")
        .invoke_handler(tauri::generate_handler![
            commands::request_permission,
            commands::list_albums,
            commands::share,
            commands::trash,
            commands::start_foreground,
            commands::stop_foreground,
            commands::keep_awake,
            commands::publish_exports,
        ])
        .setup(|app, api| {
            #[cfg(mobile)]
            let media = mobile::init(app, api)?;
            #[cfg(desktop)]
            let media = desktop::init(app, api)?;
            app.manage(media);
            Ok(())
        })
        .build()
}
