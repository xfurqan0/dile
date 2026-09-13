//! Everything Dile knows without a window.
//!
//! Two programs need the same four answers — *what did the user set?*, *which tier did this
//! machine decide on?*, *which model does that tier run and where is it?*, and *how do I
//! talk to the engine process?* — and only one of them is a Tauri application. The tray
//! (`dile-app`) and the command line (`dile-cli`) both read this crate, so `dile transcribe`
//! uses the settings the settings window wrote, the model the tray downloaded and the tier
//! the first-run probe decided, rather than a second opinion about any of them.
//!
//! ```text
//!   dile-app  ─┐
//!              ├─▶ dile-client ─▶ dile-engine-host ─▶ transcribe-cpp
//!   dile-cli  ─┘        │
//!                       └─ settings.json · engine.json · models/
//! ```
//!
//! **No Tauri in here, and that is the boundary.** Everything in this crate is `std`, serde
//! and this workspace's own leaf crates. The pieces that genuinely need an `AppHandle` — the
//! two directories Tauri names from the bundle identifier — stayed in `dile-app`, and
//! [`paths`] is how a program without an `AppHandle` names the same two directories. A test
//! there states the layout both are expected to produce.
//!
//! **No runtime in here either.** [`host`] starts `dile-engine-host` and speaks
//! [`dile_engine_proto`] to it over a pipe; `transcribe-cpp` is linked by that process and
//! by nothing else in this workspace. That is what makes "a driver fault costs the dictation
//! and not the application" a property of the architecture rather than a hope, and it is why
//! the command line gets crash isolation for free.

#![deny(unsafe_code)]

pub mod download;
pub mod host;
pub mod models;
pub mod paths;
pub mod settings;
pub mod tier;
