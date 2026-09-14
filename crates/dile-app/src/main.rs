//! Dile — hold a key, speak, let go.
//!
//! **Hold the key, speak, let go, and cleaned Turkish comes back.** What this binary does
//! today: put an icon in the system tray, install the global hotkey, open the microphone,
//! record for as long as the chord is held, and hand the buffer to an engine running in a
//! process of its own — which transcribes it, passes it through the Turkish cleanup rules and
//! shows the result in a panel the user can read, fix or cancel before it is pasted into
//! whatever they were typing in, with their own clipboard given back afterwards. Everything
//! about that is a setting a person can change while it runs. The product loop is closed.
//!
//! | Behaviour | Package |
//! |---|---|
//! | Hotkey, microphone, level stream, VAD, recording cap | **WP2, done** |
//! | The engine in its own process, the Vulkan probe, the model downloader | **WP3, done** |
//! | Turkish cleanup, hallucination filter, dictionary | **WP4, done** |
//! | Settings file, settings window, dictionary editing, autostart | **WP5a, done** |
//! | The review panel and paste with clipboard restore | **WP5b, done** |
//! | Installer, winget, signing preparation | WP7 |
//!
//! **No window is shown at start**, and that is a product decision rather than an oversight.
//! Dile has no main window: the panel of `docs/PROJECT.md` §3 appears when the user speaks and
//! hides itself again, and the settings window is built the first time somebody asks for it.
//! A window that flashes on start-up is the first thing users complain about in a tray
//! application. The panel window *exists* from the first frame — it is declared in
//! `tauri.conf.json`, created hidden and non-activating — because a window built at the moment
//! somebody starts speaking would miss the first half-second of the level meter.
//!
//! **Three failures, treated differently.** A global keyboard hook that will not install is
//! fatal: every one of Dile's features begins with a key press, so a tray icon that cannot
//! hear one is a lie in the notification area. A microphone that will not open is not: it is
//! reported on the tray and tried again at the next press, because a headset can be plugged
//! in a minute later. An engine that will not start is not either, and that is WP3's whole
//! point — the hotkey, the microphone and the tray keep working while the engine is missing,
//! downloading, probing, restarting or given up on, and the tray says which.
//!
//! **Every setting applies without a restart.** The settings file is read once, here, and
//! held in a [`settings::SettingsStore`] that three threads subscribe to: the session
//! re-installs the hook and re-opens the microphone, the engine supervisor reads the
//! strictness and the dictionary at the moment it uses them, and the two listeners below
//! carry the language to the tray and the autostart switch to the registry.

// A tray app has no console. Kept in debug builds so `cargo tauri dev` still prints.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// `deny` rather than `forbid`, and `src/win32/` is the reason: WP5b's paste has no safe
// wrapper anywhere in the stack, and `forbid` cannot be lifted even by the module that needs
// it. Every other module in this crate is still unsafe-free, and `win32`'s own documentation
// says what the exception buys and where its boundary is.
#![deny(unsafe_code)]

mod engine;
mod i18n;
mod panel;
mod paste;
mod session;
mod settings;
mod tray;
mod ui;
mod win32;

use std::sync::{Arc, RwLock};

use i18n::Strings;
use settings::{Settings, SettingsStore};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
use ui::Ui;

fn main() {
    // Debug builds print the session to the console `cargo tauri dev` attaches; a release
    // build is windowed, has nowhere to write, and installs no logger at all. WP7 brings the
    // log file, and with it the reporting a released build can do.
    //
    // `RUST_LOG` still overrides the level, which is how a capture problem gets looked at:
    // `RUST_LOG=dile_capture=debug cargo tauri dev`.
    #[cfg(debug_assertions)]
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .try_init();

    tauri::Builder::default()
        // The consent dialog of WP3, and the only thing this plugin is used for. It is
        // called from Rust, on the engine's own thread, and no webview is granted any dialog
        // permission: a webview that could open a native dialog could also open one that
        // looks like this application asking a question, and nothing in the panel or the
        // settings window has a question to ask.
        .plugin(tauri_plugin_dialog::init())
        // Start with Windows. Registered with the application rather than exposed to a
        // webview: `capabilities/settings.json` grants the settings window none of this
        // plugin's permissions, because the switch is applied from the listener below.
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            settings::commands::get_strings,
            settings::commands::get_settings,
            settings::commands::set_settings,
            settings::commands::list_input_devices,
            settings::commands::engine_status,
            settings::commands::rerun_probe,
            settings::commands::capture_hotkey,
            settings::commands::open_models_dir,
            settings::commands::close_settings,
            panel::commands::panel_context,
            panel::commands::panel_resize,
            panel::commands::panel_take_focus,
            panel::commands::panel_release_focus,
            panel::commands::panel_edited,
            panel::commands::panel_transfer,
            panel::commands::panel_copy,
            panel::commands::panel_cancel,
            panel::commands::panel_rerecord,
            panel::commands::panel_close,
            panel::commands::panel_reclean,
        ])
        .setup(|app| {
            // One settings document for the whole process, read once. Every value the
            // application used to hold as a constant comes out of here.
            let store = match settings::path(app.handle()) {
                Ok(path) => SettingsStore::open(path),
                Err(error) => {
                    // No configuration directory means no file to read or write. The
                    // application still runs, on the defaults, and says so.
                    log::error!("no configuration directory, so the defaults are in use: {error}");
                    SettingsStore::new(None, Settings::default())
                }
            };
            match store.path() {
                Some(path) => log::info!("settings file: {}", path.display()),
                None => log::warn!("the settings are not being written anywhere"),
            }
            let settings = store.get();

            // One language decision for the whole process, and one trigger for every
            // tooltip. Both are replaceable, because both are settings: the tray menu, the
            // dialogs and the settings window all read the same catalogue, so they cannot end
            // up in two different languages the way nazar-tray's did between its WP4 and WP5.
            //
            // The trigger is held as the settings spell it — `RightCtrl` — rather than as the
            // words a tooltip shows, because those words are in the language of the moment and
            // this outlives a language change. `Ui::hotkey` is where the two meet.
            let strings = Arc::new(RwLock::new(Arc::new(Strings::for_setting(
                settings.ui.language.tag(),
            ))));
            let hotkey = Arc::new(RwLock::new(settings.hotkey.trigger.clone()));

            {
                let catalogue = strings.read().expect("a fresh catalogue is uncontended");
                let label = i18n::trigger_label(catalogue.as_ref(), &settings.hotkey.trigger);
                tray::create(app, catalogue.as_ref(), &label)?;
            }

            // One handle on the tray and the panel, shared by every background thread. It
            // also carries the resting state, which is why it is built before the engine:
            // the engine is what changes the answer to "what does idle look like here".
            let ui = Ui::new(
                app.handle().clone(),
                Arc::clone(&strings),
                Arc::clone(&hotkey),
            );
            app.manage(ui.clone());
            app.manage(store.clone());

            // What the settings file says is what the machine should be doing, so the two are
            // reconciled once at start-up: a user who turned autostart on and then reinstalled
            // the application would otherwise have a switch that says yes and a registry that
            // says nothing.
            apply_autostart(app.handle(), settings.ui.autostart);

            // The panel comes before the engine and before the hook, because both of them
            // can reach it within milliseconds of starting: a dictation with nowhere to
            // appear would be the one failure this package exists to remove. Building it also
            // starts the clipboard's own thread, which is what makes the paste possible.
            let review = panel::Panel::new(app.handle(), store.clone(), ui.clone());
            app.manage(Arc::clone(&review));

            // The engine comes up on its own thread and may spend a long time doing it — a
            // consent dialog, a download, a cold Vulkan probe. Nothing below waits for it,
            // because a tray application that will not respond to its hotkey until a
            // gigabyte has arrived is one nobody would leave running.
            let engine =
                engine::EngineClient::start(ui.clone(), store.clone(), Arc::clone(&review));
            // Handed to the run loop below, which is the only thing that knows when the
            // application is closing, and to the settings window's engine group.
            app.manage(engine.clone());

            // The four changes nobody else owns: the language, the trigger the tooltip names,
            // the autostart switch and the tier. The session owns the hook and the
            // microphone, and the engine supervisor reads the strictness and the dictionary
            // where it uses them.
            let handle = app.handle().clone();
            let listening_ui = ui.clone();
            let listening_engine = engine.clone();
            let tooltip_hotkey = Arc::clone(&hotkey);
            store.on_change(move |change| {
                if change.language {
                    relanguage(&handle, &listening_ui, change.settings.ui.language);
                }
                if change.hotkey {
                    match tooltip_hotkey.write() {
                        Ok(mut trigger) => trigger.clone_from(&change.settings.hotkey.trigger),
                        Err(_) => log::warn!("the tooltip still names the previous trigger"),
                    }
                    listening_ui.rest();
                }
                if change.autostart {
                    apply_autostart(&handle, change.settings.ui.autostart);
                }
                if change.engine {
                    listening_engine.retier();
                }
            });

            // The hook is installed last, so a failure has an icon to be reported next to —
            // and so the first state the user sees is the one the application is in.
            session::start(ui, store, engine, Arc::clone(&review))?;

            // **Debug builds only.** There is no way to click a tray menu from a script, so
            // this is how the settings window gets opened by one — a screenshot for a report,
            // or a hand check that the window still lays out after a change. A release build
            // does not contain the code that reads it.
            // **Debug builds only**, and the same reason as the switch below it: the four
            // panel states cannot be reached from a script, because reaching them means
            // speaking into a microphone. `DILE_OPEN_PANEL=result` renders one with sample
            // text so it can be looked at and screenshotted.
            #[cfg(debug_assertions)]
            if let Ok(state) = std::env::var("DILE_OPEN_PANEL")
                && !state.trim().is_empty()
            {
                panel::debug_open(&review, state.trim());
            }

            #[cfg(debug_assertions)]
            if let Ok(group) = std::env::var("DILE_OPEN_SETTINGS")
                && !group.trim().is_empty()
            {
                log::warn!("DILE_OPEN_SETTINGS is set: opening the settings window at start");
                // `1` means the window; anything else names the group to open it on.
                let group = (group != "1").then_some(group);
                settings::window::open_at(app.handle(), group.as_deref());
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Dile failed to start")
        // `build` + `run` rather than `Builder::run`, for one event. The engine's thread can
        // be a long way inside something when the user picks Quit — half of a half-gigabyte
        // download, most likely — and `Exit` is the only place that knows to tell it to stop
        // between chunks rather than in the middle of a write.
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<engine::EngineClient>().stop();
            }
        });
}

/// Put the interface into another language, everywhere it is already showing.
///
/// The tray menu is rebuilt, the tooltip is repainted, and the settings window — which is
/// where the switch was flipped — gets its title back and re-reads the catalogue itself.
fn relanguage(app: &tauri::AppHandle, ui: &Ui, language: settings::Language) {
    let catalogue = Arc::new(Strings::for_setting(language.tag()));
    log::info!("the interface language is now {}", catalogue.locale());

    match ui.catalogue().write() {
        Ok(mut held) => *held = Arc::clone(&catalogue),
        Err(_) => {
            log::error!("the catalogue could not be replaced; the language did not change");
            return;
        }
    }

    let handle = app.clone();
    let repaint = ui.clone();
    // Menus belong to the main thread. Called from it, this is a queued closure; called from
    // anywhere else, it is the only way to touch one.
    let queued = app.run_on_main_thread(move || {
        tray::relabel(&handle, &catalogue);
        settings::window::retitle(&handle, &catalogue.text("settings.title"));
        repaint.rest();
    });
    if let Err(error) = queued {
        log::warn!("the tray could not be put into the new language: {error}");
    }
}

/// Write the start-with-Windows switch, and say what happened.
///
/// Never fatal. A registry value that will not write is a switch that did not take, and the
/// honest place for that is the log — a dictation application that refused to start because
/// it could not add itself to `Run` would be a worse product than one that did not autostart.
fn apply_autostart(app: &tauri::AppHandle, wanted: bool) {
    let manager = app.autolaunch();
    match manager.is_enabled() {
        Ok(current) if current == wanted => return,
        Ok(_) => {}
        Err(error) => log::warn!("the autostart entry could not be read: {error}"),
    }

    let written = if wanted {
        manager.enable()
    } else {
        manager.disable()
    };
    match written {
        Ok(()) => log::info!("autostart is now {}", if wanted { "on" } else { "off" }),
        Err(error) => log::error!("the autostart entry could not be written: {error}"),
    }
}
