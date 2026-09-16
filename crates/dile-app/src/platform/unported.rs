//! The desktop on a platform Dile has not been ported to: four questions, four honest noes.
//!
//! This module answers everything [`super::win32`] answers, and every answer is *no*. That is
//! not a placeholder — it is the truth about a build that has no desktop integration written
//! for it yet, and saying it plainly is what lets the rest of the application be honest in
//! turn. The tray runs, the trigger runs, a dictation is recorded, transcribed and cleaned,
//! and the card shows it. What does not happen is the last step: nothing is placed on a
//! screen coordinate, nothing is put on the clipboard, and nothing is pasted anywhere.
//!
//! **Why a no rather than an approximation.** Each of these has a real answer on Linux and
//! each one costs something a person has to decide about first:
//!
//! * [`Hwnd::foreground`] — a Wayland client is not told which window has the focus, and that
//!   is a deliberate part of the protocol rather than a gap in it. Without it there is no
//!   "→ VS Code" on the card, no `Ctrl+Shift+V` for the terminal family, and no "that window
//!   is gone, so I did not paste" guarantee.
//! * [`window::primary_monitor`] — `xdg-shell` has no global coordinate space, so a client
//!   cannot ask where a display begins, nor put a window at a point on one.
//! * [`Clipboard::start`] — the clipboard is reachable, but only by taking the keyboard focus
//!   for the moment the selection is set, which is the one thing the panel promises never to
//!   do.
//!
//! An approximation here would paste a sentence into whatever happened to be in front. A no
//! leaves the text in the card where the user can read it and take it themselves, and says
//! why in the log. `docs/BUILDING.md` carries the same sentences for the person running it.

use std::path::PathBuf;
use std::time::Duration;

use super::screen::Monitor;

/// A window handle, as a number that can cross a thread boundary.
///
/// **Uninhabited on purpose.** [`Hwnd::foreground`] is the only thing anywhere in this
/// application that produces one, and on this platform it cannot: nothing tells a client
/// which window has the focus. Saying that in the type rather than in a comment means the
/// compiler agrees — every method below that would need a real handle is unreachable, and
/// reads as unreachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hwnd(std::convert::Infallible);

impl Hwnd {
    /// The window the user is working in — never known on this platform.
    #[must_use]
    pub const fn foreground() -> Option<Hwnd> {
        None
    }

    /// The handle as a plain number, for a log line or a settings key.
    #[must_use]
    pub fn as_isize(self) -> isize {
        match self.0 {}
    }

    /// Whether this window is still there.
    #[must_use]
    pub fn exists(self) -> bool {
        match self.0 {}
    }

    /// The full path of the program this window belongs to.
    #[must_use]
    pub fn process_path(self) -> Option<PathBuf> {
        match self.0 {}
    }

    /// Which display this window is on.
    #[must_use]
    pub fn monitor(self) -> Option<Monitor> {
        match self.0 {}
    }

    /// Bring this window back to the front.
    #[must_use]
    pub fn focus(self) -> bool {
        match self.0 {}
    }
}

/// Windows, monitors and the foreground — the same four questions, with no answers.
pub mod window {
    use super::super::screen::Monitor;

    /// The primary display, for when there is no window to ask about.
    ///
    /// `None`: `xdg-shell` has no global coordinate space, so there is no rectangle to
    /// return. The panel is left wherever the compositor put it.
    #[must_use]
    pub const fn primary_monitor() -> Option<Monitor> {
        None
    }

    /// Type the paste chord into whatever has the focus.
    ///
    /// `false`. Nothing here can synthesise a key press without a permission the application
    /// has not asked for, and there is no way to know which window would receive it.
    #[must_use]
    pub const fn send_paste_chord(with_shift: bool) -> bool {
        let _ = with_shift;
        false
    }
}

/// What can go wrong with a clipboard.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    /// There is no clipboard integration on this platform yet.
    #[error(
        "this build has no clipboard integration: a dictation can be read in the panel and copied by hand"
    )]
    Unported,
}

/// A promise that a piece of text will be handed over when somebody pastes.
///
/// Never handed out on this platform: [`Clipboard::start`] fails, so nothing reaches the
/// point of making a promise.
#[derive(Clone, Debug)]
pub struct Receipt {
    never: std::convert::Infallible,
}

impl Receipt {
    /// Wait until the text is handed over, or until the deadline passes.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let _ = timeout;
        match self.never {}
    }
}

/// The clipboard, and the three things this application does with it.
#[derive(Debug)]
pub struct Clipboard {
    never: std::convert::Infallible,
}

impl Clipboard {
    /// Start the clipboard agent.
    ///
    /// # Errors
    ///
    /// Always, on this platform. The caller is expected to carry on without one: the
    /// application still records, still transcribes and still shows the result.
    pub const fn start() -> Result<Clipboard, ClipboardError> {
        Err(ClipboardError::Unported)
    }

    /// What the clipboard holds as text right now.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        match self.never {}
    }

    /// Put text on the clipboard and leave it there.
    ///
    /// # Errors
    ///
    /// Unreachable: no [`Clipboard`] is ever constructed on this platform.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let _ = text;
        match self.never {}
    }

    /// Offer text as a delayed render, with a receipt for when it is taken.
    ///
    /// # Errors
    ///
    /// Unreachable: no [`Clipboard`] is ever constructed on this platform.
    pub fn offer(&self, text: &str) -> Result<Receipt, ClipboardError> {
        let _ = text;
        match self.never {}
    }

    /// Give the user's own clipboard back.
    ///
    /// # Errors
    ///
    /// Unreachable: no [`Clipboard`] is ever constructed on this platform.
    pub fn restore(&self, previous: Option<String>) -> Result<(), ClipboardError> {
        let _ = previous;
        match self.never {}
    }
}
