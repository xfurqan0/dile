//! The trigger: when the user means to dictate, and when they do not.
//!
//! `docs/PROJECT.md` §6 gives WP2 two halves. This crate is the key half — the other is the
//! microphone, and the two meet in the application rather than in each other. So there is no
//! Tauri here, no audio device, and no user-facing string: the crate turns key events into
//! four verbs and hands them over.
//!
//! ## Two layers, and why the seam is where it is
//!
//! | Layer | Module | Depends on |
//! |---|---|---|
//! | The rules | [`state`] | nothing but `std` |
//! | The operating system | [`listener`] | `handy-keys`, Windows only |
//!
//! Everything that is hard about a push-to-talk trigger is a timing question — was that a
//! tap or a sentence, did the release arrive, is this the eleventh auto-repeat of a key that
//! is already down — and a global keyboard hook is the worst place in the system to test a
//! timing question. So [`HotkeyMachine`] reads no clock: every event carries the instant it
//! happened at, which puts all twelve scenarios of WP2 in an ordinary unit test that runs in
//! microseconds. [`listener`] is then allowed to be thin, because it holds no rules — it
//! translates virtual-key codes into [`Event`]s, feeds them in, and forwards what comes back.
//!
//! It is also why a macOS or Linux port is a module rather than a rewrite: the rules are
//! already portable, and they are already tested on every platform this workspace builds on.
//!
//! ## The four verbs, and the three WP5b added
//!
//! [`Action::StartRecording`], [`Action::StopRecording`], [`Action::DiscardRecording`],
//! [`Action::OpenPanelIdle`]. Recording starts at the **press**, before anyone knows whether
//! the press will turn out to be a real one, so that the first syllable is already in the
//! buffer — which makes `DiscardRecording` a normal outcome rather than an error path. The
//! reasoning behind each rule is in the [`state`] module documentation.
//!
//! [`Action::PanelTransfer`], [`Action::PanelCancel`] and [`Action::PanelCopy`] are the
//! review panel's Enter, Esc and `Ctrl+C`. They are not a trigger and they are off by
//! default: the application arms them with [`HotkeyListener::panel_keys`] while the panel is
//! on screen, which both blocks the three keys and turns them into these verbs. The panel
//! never takes focus, so reading them from the hook is the only place they can be read at
//! all — and blocking them is why the Enter that transfers does not also land in the
//! document behind the panel.
//!
//! ## What the application still owes the machine
//!
//! * [`HotkeyMachine::reset`] on focus loss, because the key-up may never arrive.
//! * [`Event::Tick`] a few times a second, so the safety ceiling can fire during a press
//!   that produces no events at all. [`listener`] does both of these for you.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod keys;
pub mod state;

pub use config::{
    DEFAULT_HOLD_TAKEOVER_MS, DEFAULT_MAX_HOLD_MS, DEFAULT_PRESS_THRESHOLD_MS, HotkeyConfig, Mode,
};
pub use error::Error;
pub use keys::{Chord, ChordParseError, Key, MainKey, ModifierFamily, ModifierKey, ModifierOnly};
pub use state::{Action, Actions, Event, HotkeyMachine, PanelKey};

#[cfg(target_os = "windows")]
pub mod capture;

#[cfg(target_os = "windows")]
pub mod listener;

#[cfg(target_os = "windows")]
pub use capture::{Capture, next_chord};

#[cfg(target_os = "windows")]
pub use listener::{Emitted, HotkeyListener};

/// The version of this crate, for the settings page and bug reports.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
