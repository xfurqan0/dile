//! Dile — hold a key, speak, let go.
//!
//! **WP2 is as far as this binary goes, and it says so.** What it does today: put an icon in
//! the system tray, install the global hotkey, open the microphone, and record for as long as
//! `Ctrl+Alt+Space` is held. What comes back is an audio buffer, a level stream and a verdict
//! on whether anything was said — and then it stops, because the engine is WP3's.
//!
//! | Behaviour | Package |
//! |---|---|
//! | Hotkey, microphone, level stream, VAD, recording cap | **WP2, done** |
//! | The engine, in a process of its own, and the model downloader | WP3 |
//! | Turkish cleanup, hallucination filter, dictionary | WP4 |
//! | The review panel, paste with clipboard restore, settings | WP5 |
//! | Installer, winget, signing preparation | WP7 |
//!
//! **No window is shown at start**, and that is a product decision rather than an
//! oversight. Dile has no main window: the panel of `docs/PROJECT.md` §3 appears when the
//! user speaks and hides itself again. `tauri.conf.json` therefore declares the panel with
//! `visible: false`, and nothing here shows it — a window that flashes on start-up is the
//! first thing users complain about in a tray application.
//!
//! **Two failures, treated differently.** A global keyboard hook that will not install is
//! fatal: every one of Dile's features begins with a key press, so a tray icon that cannot
//! hear one is a lie in the notification area. A microphone that will not open is not: it is
//! reported on the tray and tried again at the next press, because a headset can be plugged
//! in a minute later.

// A tray app has no console. Kept in debug builds so `cargo tauri dev` still prints.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod config;
mod i18n;
mod session;
mod tray;

use std::sync::Arc;

use config::AppConfig;
use i18n::Strings;

fn main() {
    // Debug builds print the session to the console `cargo tauri dev` attaches; a release
    // build is windowed, has nowhere to write, and installs no logger at all. WP5 brings the
    // log file, and with it the reporting a released build can do.
    //
    // `RUST_LOG` still overrides the level, which is how a capture problem gets looked at:
    // `RUST_LOG=dile_capture=debug cargo tauri dev`.
    #[cfg(debug_assertions)]
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .try_init();

    // One language decision for the whole process. WP5 makes it the settings override and
    // then the operating system's UI language; every string already goes through it, so the
    // tray menu and the panel cannot end up in two different languages the way nazar-tray's
    // did between its WP4 and WP5.
    let strings = Arc::new(Strings::system());

    // One configuration for the whole process, and no file behind it yet: WP5 is what reads
    // and writes these. See `config.rs` for what each default is and why.
    let config = AppConfig::default();

    tauri::Builder::default()
        .setup(move |app| {
            tray::create(app, &strings)?;
            // The hook is installed after the tray, so a failure has an icon to be reported
            // next to — and so the first state the user sees is the one the app is in.
            session::start(app.handle(), strings, config)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Dile failed to start");
}
