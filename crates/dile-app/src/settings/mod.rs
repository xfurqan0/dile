//! The settings, from the tray's side.
//!
//! The document itself — every field, its default, the clamping and the atomic write —
//! lives in [`dile_client::settings`], because `dile transcribe` reads the same file and a
//! command line has no business linking Tauri to do it. What is left here is the three
//! things only an application needs: [`store`], which tells the threads that own the hook,
//! the microphone and the engine what actually moved; [`commands`], which is how the
//! settings window asks; and [`window`], which is the window itself.
//!
//! [`path`] is the fourth: the one line of the settings that genuinely needs an `AppHandle`,
//! because the directory is named after the bundle identifier and Tauri is what reads it.

pub mod commands;
pub mod store;
pub mod window;

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

pub use dile_client::settings::*;
pub use store::SettingsStore;

/// Where the settings live: `<app config>/settings.json`.
///
/// The same directory as `engine.json` — see [`crate::engine::state::tier_path`], which
/// names the other file in it.
///
/// # Errors
///
/// The platform would not name a configuration directory.
pub fn path(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    Ok(app.path().app_config_dir()?.join(FILE))
}
