//! The two directories Dile keeps things in, for a program with no `AppHandle`.
//!
//! The tray asks Tauri, which derives both from the bundle identifier in `tauri.conf.json`.
//! `dile transcribe` has no Tauri in it and must still land on the same two directories, or
//! it would read a settings file nobody wrote and look for a model nobody downloaded. So the
//! layout is written down here, once, with the identifier next to it — and the test at the
//! bottom states it rather than trusting it.
//!
//! | | Windows |
//! |---|---|
//! | [`config_dir`] | `%APPDATA%\io.github.xfurqan0.dile` — `settings.json`, `engine.json` |
//! | [`local_data_dir`] | `%LOCALAPPDATA%\io.github.xfurqan0.dile` — `models\` |
//!
//! Roaming for the two small files a person would want on their next machine, local for the
//! gigabyte of weights they would not. That is Tauri's split as well as this one, which is
//! the point: the same two answers, from two programs, for one machine.
//!
//! **v1 is Windows.** The other two arms exist so this crate builds on the platforms
//! `dile-core` already builds on, and they follow the same XDG rules Tauri's own directory
//! crate does; nothing has been run against them, and v2 is where that changes.

use std::path::PathBuf;

/// The bundle identifier, which is what both directories are named after.
///
/// The same string as `tauri.conf.json`'s `identifier`, and `dile-app`'s test says so.
pub const IDENTIFIER: &str = "io.github.xfurqan0.dile";

/// Anything that can stop this crate naming a directory.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PathError {
    /// The operating system would not say where the user's data lives.
    #[error("the {0} environment variable is not set, so Dile cannot find its own files")]
    NoHome(&'static str),
}

/// `<app config>`: where `settings.json` and `engine.json` live.
///
/// # Errors
///
/// The platform would not name a configuration directory.
pub fn config_dir() -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(from_env("APPDATA")?).join(IDENTIFIER))
    }
    #[cfg(not(windows))]
    {
        match std::env::var("XDG_CONFIG_HOME") {
            Ok(base) if !base.trim().is_empty() => Ok(PathBuf::from(base).join(IDENTIFIER)),
            _ => Ok(PathBuf::from(from_env("HOME")?)
                .join(".config")
                .join(IDENTIFIER)),
        }
    }
}

/// `<app local data>`: where `models/` lives.
///
/// # Errors
///
/// The platform would not name a local data directory.
pub fn local_data_dir() -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(from_env("LOCALAPPDATA")?).join(IDENTIFIER))
    }
    #[cfg(not(windows))]
    {
        match std::env::var("XDG_DATA_HOME") {
            Ok(base) if !base.trim().is_empty() => Ok(PathBuf::from(base).join(IDENTIFIER)),
            _ => Ok(PathBuf::from(from_env("HOME")?)
                .join(".local")
                .join("share")
                .join(IDENTIFIER)),
        }
    }
}

/// One environment variable, or the error that names it.
fn from_env(name: &'static str) -> Result<String, PathError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(PathError::NoHome(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::{IDENTIFIER, config_dir, local_data_dir};

    #[test]
    fn both_directories_end_in_the_bundle_identifier_and_are_not_the_same_one() {
        let config = config_dir().expect("a configuration directory");
        let data = local_data_dir().expect("a local data directory");

        assert_eq!(
            config.file_name().and_then(|name| name.to_str()),
            Some(IDENTIFIER),
            "the settings would not be found where the application writes them"
        );
        assert_eq!(
            data.file_name().and_then(|name| name.to_str()),
            Some(IDENTIFIER)
        );

        // Roaming and local are different directories on Windows, and the models must not
        // end up in the one that follows a user between machines.
        #[cfg(windows)]
        assert_ne!(config, data, "a gigabyte of weights would start roaming");
    }

    #[test]
    fn the_identifier_is_the_reverse_domain_name_the_bundle_declares() {
        assert_eq!(IDENTIFIER, "io.github.xfurqan0.dile");
    }
}
