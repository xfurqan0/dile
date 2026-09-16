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
//! | Platform | Foreground window | Screen coordinates | Aimed paste chord | Clipboard | Unaimed chord |
//! |---|---|---|---|---|---|
//! | Windows | [`win32`] | [`win32`] | [`win32`] | [`win32`] | — |
//! | Linux | [`unported`] | [`unported`] | [`unported`] | **[`linux`]** | **[`uinput`]** |
//! | everything else | [`unported`] | [`unported`] | [`unported`] | [`unported`] | [`unported`] |
//!
//! Linux is why that is a table of columns rather than a list of two modules. A platform
//! is not ported or unported as a whole: three of those questions have no answer under
//! Wayland by design, and the clipboard does. [`DELIVERY`] is what the
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
//! coordinate, and cannot send a key press to a window it cannot name. Those are the
//! protocol's decisions rather than this application's, and `unported` states them for Linux
//! as exactly what they are. The clipboard was the one that had a route, so [`linux`] is the
//! module that took it.
//!
//! The last column is the one thing that is neither: a key press **can** be synthesised on
//! Linux, through the `uinput` permission the trigger already asks for, and it goes to
//! whatever holds the keyboard rather than to a window anybody chose. [`uinput`] is that, and
//! [`AUTO_PASTE_OFFERED`] is the constant that says where the settings window may offer it.
//! It is a different and smaller thing from the aimed chord in the column before it, which is
//! why it is a column of its own rather than an answer in that one.

pub mod screen;

// A display is what the application above this seam works in; the rectangles it is made of
// are its innards, and `screen::Rect` is the name for them.
pub use screen::Monitor;

#[cfg(windows)]
pub mod win32;

#[cfg(windows)]
pub use win32::{Clipboard, ClipboardError, Hwnd, window};

#[cfg(windows)]
pub use unported::AutoPaste;

// Compiled on every platform, and on Windows for one item only: `win32` answers every other
// question this module has, and the unaimed chord is not a question `win32` has. The file
// itself says which half of it each platform takes.
pub mod unported;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub mod uinput;

// Linux keeps three of `unported`'s answers and brings its own clipboard and its own
// unaimed chord. The split is here rather than inside any of the modules so that none of
// them has to know about the others.
#[cfg(target_os = "linux")]
pub use linux::{Clipboard, ClipboardError};

// The error type is deliberately **not** re-exported alongside it, on any platform: the one
// caller reads a locale key off a failure rather than matching on its variants, and a binary
// crate's unused `pub use` is a warning this workspace treats as an error. It is named by the
// module that produces it, which is where its variants are worth reading.
#[cfg(target_os = "linux")]
pub use uinput::AutoPaste;

#[cfg(target_os = "linux")]
pub use unported::{Hwnd, window};

#[cfg(all(not(windows), not(target_os = "linux")))]
pub use unported::{AutoPaste, Clipboard, ClipboardError, Hwnd, window};

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

/// Whether this build can press `Ctrl+V` at whatever holds the keyboard, and may therefore
/// offer the experimental switch that does.
///
/// **Not the same question as [`DELIVERY`], and not the same as a paste.** It is true exactly
/// where two things hold at once: the hand-over is a clipboard one, so there is something for
/// a chord to add; and there is a way to synthesise a key press at all. Linux is the only
/// platform where both are true — [`uinput`] is the way, and the permission it needs is the
/// one the trigger already asks for.
///
/// On Windows it is false because a dictation is pasted into the window it was **aimed at**,
/// which is the larger promise; a switch offering an unaimed press next to it would be a
/// worse option presented as an extra.
pub const AUTO_PASTE_OFFERED: bool = cfg!(target_os = "linux");

/// The locale key of the card's line when an auto-paste that was asked for did not happen and
/// there is nothing more useful to say than that.
///
/// Here rather than in either module, because both of them return it and the panel names it
/// too: three places, one string.
pub const AUTO_PASTE_FAILED: &str = "panel.state.autopaste.failed";

#[cfg(test)]
mod tests {
    use super::{AUTO_PASTE_OFFERED, DELIVERY, Delivery};

    #[test]
    fn a_clipboard_hand_over_and_an_aimed_paste_chord_are_never_both_on() {
        // Two decisions taken in two places that have to agree. `send_paste_chord` means
        // *type the chord into the window this dictation was aimed at*, and a build that
        // could do that and still handed over on the clipboard would be refusing to use an
        // answer it has.
        //
        // **D-WP-L4 is the package that looked at this and did not change it.** What landed
        // there is a chord at whatever holds the keyboard, which is a different function in a
        // different module behind a setting that is off by default; the window this dictation
        // was aimed at is still not a thing a Wayland client can name, so this stays false
        // and this test stays a real guard rather than a historical one.
        if DELIVERY == Delivery::Clipboard {
            assert!(
                !super::window::send_paste_chord(false),
                "this build hands over on the clipboard and can also aim Ctrl+V"
            );
            assert!(
                !super::window::send_paste_chord(true),
                "this build hands over on the clipboard and can also aim Ctrl+Shift+V"
            );
        }
    }

    #[test]
    fn the_unaimed_chord_is_only_offered_where_the_hand_over_needs_one() {
        // The switch adds something only where a dictation is handed over rather than
        // pasted. Offering it next to a real paste would be offering a worse option as an
        // extra, and this is the assertion that says so on the platform that has the real one.
        if AUTO_PASTE_OFFERED {
            assert_eq!(
                DELIVERY,
                Delivery::Clipboard,
                "a platform that pastes has nothing to add an unaimed chord to"
            );
        }
        // The same invariant from the other end is deliberately not written a second time:
        // `assert!(!CONST)` is something the compiler already decided, which is exactly what
        // clippy's `assertions_on_constants` objects to, and the implication above is the
        // whole rule anyway.
        //
        // And it is a Linux answer today. A second platform arriving here is a decision, not
        // a side effect of a `cfg`.
        assert_eq!(AUTO_PASTE_OFFERED, cfg!(target_os = "linux"));
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
