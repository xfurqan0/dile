//! The Windows desktop: the only unsafe code in this application, fenced into one module.
//!
//! Everything else in `dile-app` is safe Rust, and the crate says so: `main.rs` carries
//! `#![deny(unsafe_code)]` rather than `#![forbid]`, and `forbid` was the right word right up
//! to WP5b — a tray icon, a settings form and a subprocess need no pointers. A **paste** does.
//! Windows has no safe way to say "put this on the clipboard, hand it over when the target
//! asks for it, and give the user's own clipboard back afterwards", nor to place a window on
//! the monitor that holds somebody else's foreground window without asking the operating
//! system for a `HWND`.
//!
//! So the exception is a module rather than a scattering of blocks. `deny` can be lifted in
//! one place and only in one place, which is what the two `#![allow(unsafe_code)]` lines in
//! [`window`] and [`clipboard`] are: an allowance a reviewer can find by grepping for it.
//! Every `unsafe` block inside them carries a `// SAFETY:` line saying what makes the call
//! sound. Nothing else in the crate may add one — `paste.rs` holds the *decisions* about
//! pasting (which chord, which application, what to restore) and not one raw call, which is
//! also why those decisions are unit-testable on a machine with no clipboard.
//!
//! **Nothing here is about Dile.** The two submodules know about windows, monitors, processes
//! and the clipboard. What a dictation is, when it pastes and what the panel says about it
//! belong to `panel.rs` and `paste.rs`.
//!
//! [`super::screen`] holds the two types that are not a Windows question — a rectangle and a
//! display — so that the panel's placement arithmetic stays testable on a platform that has
//! no `MONITORINFOEXW` to read them out of.

pub mod clipboard;
pub mod window;

pub use clipboard::{Clipboard, ClipboardError};
pub use window::Hwnd;
