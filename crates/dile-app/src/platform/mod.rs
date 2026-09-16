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
//! | Platform | Module | What it can do |
//! |---|---|---|
//! | Windows | [`win32`] | all of it |
//! | everything else | [`unported`] | nothing, and says so |
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
//! On Linux each of those answers has a known replacement and a known cost, and none of them
//! is a small one: a Wayland client cannot be told which window has the focus, cannot place a
//! window at a screen coordinate, and cannot put text on the clipboard without taking the
//! keyboard focus for a moment. Those are product decisions before they are code, which is
//! why this module is a seam rather than a half-built second implementation.

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

#[cfg(not(windows))]
pub use unported::{Clipboard, ClipboardError, Hwnd, window};
