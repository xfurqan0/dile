//! The desktop, per platform: which window is in front, which screen it is on, and the
//! clipboard.
//!
//! Everything above this module is platform-free. `panel.rs` decides where a card belongs and
//! `paste.rs` decides what to call the target and which chord to send it; both of them ask
//! this module the four questions that only an operating system can answer, and neither of
//! them names one. That is why their tables are ordinary unit tests on a machine with no
//! clipboard and no foreground window — on **any** machine, now that there is more than one
//! kind.
//!
//! | Platform | Foreground window | Screen coordinates | Paste chord | Clipboard |
//! |---|---|---|---|---|
//! | Windows | [`win32`] | [`win32`] | [`win32`] | [`win32`] |
//! | Linux | [`unported`] | [`unported`] | [`unported`] | **[`linux`]** |
//! | everything else | [`unported`] | [`unported`] | [`unported`] | [`unported`] |
//!
//! Linux is why that is a table of four columns rather than a list of two modules. A platform
//! is not ported or unported as a whole: three of those questions have no answer under
//! Wayland by design, and the fourth — the clipboard — does. [`DELIVERY`] is what the
//! difference adds up to, and it is a value rather than a `cfg` so that both of the panel's
//! paths are ordinary code with ordinary tests on either kind of machine.
//!
//! [`screen`] sits outside that split because a rectangle is not a platform question. The
//! panel's placement arithmetic is tested on every platform this workspace builds on, which
//! is the same reason `dile-hotkey` keeps its state machine away from its listener.
//!
//! ## What [`unported`] is, and what it is not
//!
//! It is not a fallback and it is not a stub that pretends. Every one of its answers is the
//! true one for a platform whose desktop integration has not been written: there is no
//! foreground window it can name, so it names none; there is no clipboard agent it can start,
//! so starting one fails with a sentence that says which platform and why. The application
//! runs on top of that — the tray works, the trigger works, a dictation is transcribed and
//! shown — and the parts that need a desktop say they cannot rather than doing something
//! approximate.
//!
//! On Linux three of those answers are **permanently** no rather than not-yet: a Wayland
//! client is not told which window has the focus, cannot place a window at a screen
//! coordinate, and — until the package that builds it — cannot synthesise a key press. Those
//! are the protocol's decisions rather than this application's, and `unported` states them for
//! Linux as exactly what they are. The clipboard was the one that had a route, so [`linux`]
//! is the module that took it.

pub mod screen;

// A display is what the application above this seam works in; the rectangles it is made of
// are its innards, and `screen::Rect` is the name for them.
pub use screen::Monitor;

#[cfg(windows)]
pub mod win32;

#[cfg(windows)]
pub use win32::{Clipboard, ClipboardError, Hwnd, window};

#[cfg(not(windows))]
pub mod unported;

#[cfg(target_os = "linux")]
pub mod linux;

// Linux keeps three of `unported`'s four answers and brings its own clipboard. The split is
// here rather than inside either module so that neither of them has to know about the other.
#[cfg(target_os = "linux")]
pub use linux::{Clipboard, ClipboardError};

#[cfg(target_os = "linux")]
pub use unported::{Hwnd, window};

#[cfg(all(not(windows), not(target_os = "linux")))]
pub use unported::{Clipboard, ClipboardError, Hwnd, window};

/// How a finished dictation gets from the panel into what the user is typing in.
///
/// The one product decision this seam carries, because it is the one that a missing platform
/// answer decides rather than merely limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Dile puts the text where the caret is: it knows the target window, it can bring it
    /// back to the front, and it can send it a paste chord.
    Paste,
    /// Dile puts the text on the clipboard and says so, and the person presses `Ctrl+V`
    /// wherever they want it.
    ///
    /// Not a degraded paste. There is no window to aim at under Wayland — a client is not
    /// told which one has the focus — so a paste here would go to whatever happened to be in
    /// front, which is the one thing `paste.rs` exists to refuse. The hand-over is a smaller
    /// promise that is kept.
    Clipboard,
}

/// How this build hands a dictation over.
///
/// `cfg!` rather than `#[cfg]` on purpose: this is a constant both branches of `panel.rs`
/// compile against, so the clipboard path is type-checked on Windows and the paste path is
/// type-checked on Linux.
pub const DELIVERY: Delivery = if cfg!(windows) {
    Delivery::Paste
} else {
    Delivery::Clipboard
};

#[cfg(test)]
mod tests {
    use super::{DELIVERY, Delivery};

    #[test]
    fn a_clipboard_hand_over_and_a_paste_chord_are_never_both_on() {
        // Two decisions taken in two places that have to agree, and a guard for the package
        // that builds the other one. Synthesising `Ctrl+V` on Linux is a `uinput` device away
        // — the trigger already asks for that permission — and the day that lands, this fails
        // until somebody looks at this constant again. Which is the point: a build that could
        // send a chord and still handed over on the clipboard would be doing the harder half
        // of the work and none of the useful half.
        if DELIVERY == Delivery::Clipboard {
            assert!(
                !super::window::send_paste_chord(false),
                "this build hands over on the clipboard and can also send Ctrl+V"
            );
            assert!(
                !super::window::send_paste_chord(true),
                "this build hands over on the clipboard and can also send Ctrl+Shift+V"
            );
        }
    }

    #[test]
    fn each_platform_hands_over_the_way_its_desktop_allows() {
        if cfg!(target_os = "linux") {
            assert_eq!(DELIVERY, Delivery::Clipboard);
        }
        if cfg!(windows) {
            assert_eq!(DELIVERY, Delivery::Paste);
        }
    }
}
