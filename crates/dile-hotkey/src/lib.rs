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
//! | The operating system | [`listener`] | `handy-keys`, on Windows and Linux |
//!
//! **One trigger, two shapes.** [`Trigger`] is either one modifier key held on its own — the
//! right Ctrl, which is what Dile ships — or a chord such as `Ctrl+Alt+Space`, which is what a
//! user changes it to. Until 2026-09-14 there were two settings, a chord plus an optional
//! modifier-only "second key"; the maintainer's hand test of the 0.1.0 installer removed the
//! second one by making it the first. The rules the two shapes differ under are in [`state`].
//!
//! Everything that is hard about a push-to-talk trigger is a timing question — was that a
//! tap or a sentence, did the release arrive, is this the eleventh auto-repeat of a key that
//! is already down — and a global keyboard hook is the worst place in the system to test a
//! timing question. So [`HotkeyMachine`] reads no clock: every event carries the instant it
//! happened at, which puts all twelve scenarios of WP2 in an ordinary unit test that runs in
//! microseconds. [`listener`] is then allowed to be thin, because it holds no rules — it
//! translates virtual-key codes into [`Event`]s, feeds them in, and forwards what comes back.
//!
//! It is also why a macOS port is a module rather than a rewrite: the rules are already
//! portable, and they are already tested on every platform this workspace builds on. Linux
//! turned out to need less than that — `handy-keys` reads `/dev/input/event*` directly, which
//! needs neither an X11 nor a Wayland connection and keeps the right Ctrl distinguishable
//! from the left one, so [`listener`] compiles there unchanged. The cost is moved rather than
//! removed: those devices are readable only with a permission a distribution does not grant
//! by default, and [`Error`] is where that is said in a sentence.
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
pub use keys::{
    Chord, ChordParseError, Key, MainKey, ModifierFamily, ModifierKey, ModifierOnly, Trigger,
};
pub use state::{Action, Actions, Event, HotkeyMachine, PanelKey};

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod capture;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod listener;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use capture::{Capture, next_trigger};

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use listener::{Emitted, HotkeyListener, Remote};

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
