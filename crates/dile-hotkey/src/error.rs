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

    /// Linux: the kernel's input devices turned this user away.
    ///
    /// Separated from [`Error::Hook`] because it is the one failure with a fix the person in
    /// front of the machine can apply, and because the fix is not the obvious one. Dile reads
    /// key presses from the kernel's input devices, which is what lets one trigger behave the
    /// same under Wayland, under X11 and on a bare console — and no distribution grants that
    /// to an ordinary user by default.
    ///
    /// **Two devices, one permission.** `/dev/input/event*` is how a press is seen;
    /// `/dev/uinput` is how one is swallowed, because grabbing a keyboard means re-injecting
    /// everything that is not the trigger through a clone. The second is asked for even when
    /// the trigger blocks nothing, since the panel adds Enter, Esc and `Ctrl+C` to the same
    /// set while it is on screen.
    ///
    /// `handy-keys` suggests `usermod -aG input`, and Dile does not: that grants every process
    /// this user ever starts, an SSH session included, the ability to read every keystroke on
    /// the machine for as long as the account exists. The `uaccess` rule in
    /// `packaging/linux/70-dile-input.rules` grants it to whoever is physically logged in at
    /// the seat instead, takes effect without a logout, and goes away when they log out.
    #[error(
        "the keyboard is not readable by this user: /dev/input/event* answers the trigger and \
         /dev/uinput is what lets a chord be swallowed, and one of the two said no. \
         packaging/linux/70-dile-input.rules opens both, and \"Keyboard access on Linux\" in \
         docs/BUILDING.md says exactly what that grants"
    )]
    KeyboardAccess,

    /// The listener thread could not be started.
    #[error("the hotkey listener thread could not be started: {0}")]
    Thread(String),

    /// The listener is gone — it was dropped, or its thread ended.
    #[error("the hotkey listener is no longer running")]
    NotRunning,
}
