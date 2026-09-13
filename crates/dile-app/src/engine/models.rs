//! The models, from the tray's side.
//!
//! The table itself — the two tiers, the pinned URLs and the checksums every download is
//! verified against — lives in [`dile_client::models`], because `dile transcribe` runs the
//! same weights from the same directory and downloads them the same way.
//!
//! What is left here is [`directory`]: the one line that needs an `AppHandle`, because
//! `<app local data>` is named after the bundle identifier and Tauri is what reads it.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

pub use dile_client::models::*;

/// Where models are kept: `<app local data>/models/`.
///
/// # Errors
///
/// The platform would not name a local data directory.
pub fn directory(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    Ok(directory_in(&app.path().app_local_data_dir()?))
}
