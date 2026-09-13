//! What can go wrong, and nothing else.
//!
//! These messages are for a maintainer reading a log, not for a user reading a window.
//! `docs/PROJECT.md` keeps the user-facing strings in `locales/`, and a library crate has no
//! business holding any.

/// A failure in the OS adapter.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The operating system refused to install the global keyboard hook.
    ///
    /// On Windows the usual causes are a session with no message queue and another process
    /// holding the hook chain hostage; on the other platforms it is a missing permission.
    #[error("the global keyboard hook could not be installed: {0}")]
    Hook(String),

    /// The listener thread could not be started.
    #[error("the hotkey listener thread could not be started: {0}")]
    Thread(String),

    /// The listener is gone — it was dropped, or its thread ended.
    #[error("the hotkey listener is no longer running")]
    NotRunning,
}
