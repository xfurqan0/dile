//! Dile — hold a key, speak, let go.
//!
//! **Hold the key, speak, let go, and cleaned Turkish comes back.** What this binary does
//! today: put an icon in the system tray, install the global hotkey, open the microphone,
//! record for as long as `Ctrl+Alt+Space` is held, and hand the buffer to an engine running
//! in a process of its own — which transcribes it, passes it through the Turkish cleanup
//! rules and logs the result. What is missing is the last step of the product: somewhere for
//! that text to *go*, which is WP5's panel and paste.
//!
//! | Behaviour | Package |
//! |---|---|
//! | Hotkey, microphone, level stream, VAD, recording cap | **WP2, done** |
//! | The engine in its own process, the Vulkan probe, the model downloader | **WP3, done** |
//! | Turkish cleanup, hallucination filter, dictionary | **WP4, done** |
//! | The review panel, paste with clipboard restore, settings | WP5 |
//! | Installer, winget, signing preparation | WP7 |
//!
//! **No window is shown at start**, and that is a product decision rather than an
//! oversight. Dile has no main window: the panel of `docs/PROJECT.md` §3 appears when the
//! user speaks and hides itself again. `tauri.conf.json` therefore declares the panel with
//! `visible: false`, and nothing here shows it — a window that flashes on start-up is the
//! first thing users complain about in a tray application.
//!
//! **Three failures, treated differently.** A global keyboard hook that will not install is
//! fatal: every one of Dile's features begins with a key press, so a tray icon that cannot
//! hear one is a lie in the notification area. A microphone that will not open is not: it is
//! reported on the tray and tried again at the next press, because a headset can be plugged
//! in a minute later. An engine that will not start is not either, and that is WP3's whole
//! point — the hotkey, the microphone and the tray keep working while the engine is missing,
//! downloading, probing, restarting or given up on, and the tray says which.

// A tray app has no console. Kept in debug builds so `cargo tauri dev` still prints.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

mod config;
mod engine;
mod i18n;
mod session;
mod tray;
mod ui;

use std::sync::Arc;

use config::AppConfig;
use i18n::Strings;
use ui::Ui;

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
        // The consent dialog of WP3, and the only thing this plugin is used for. It is
        // called from Rust, on the engine's own thread, and the panel is **not** granted any
        // dialog permission in `capabilities/default.json`: a webview that could open a
        // native dialog could also open one that looks like this application asking a
        // question, and nothing in the panel has a question to ask.
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            tray::create(app, &strings)?;

            // One handle on the tray and the panel, shared by every background thread. It
            // also carries the resting state, which is why it is built before the engine:
            // the engine is what changes the answer to "what does idle look like here".
            let ui = Ui::new(app.handle().clone(), strings);

            // The engine comes up on its own thread and may spend a long time doing it — a
            // consent dialog, a download, a cold Vulkan probe. Nothing below waits for it,
            // because a tray application that will not respond to its hotkey until a
            // gigabyte has arrived is one nobody would leave running.
            let engine = engine::EngineClient::start(ui.clone(), &config);

            // The hook is installed last, so a failure has an icon to be reported next to —
            // and so the first state the user sees is the one the application is in.
            session::start(ui, config, engine)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Dile failed to start");
}
