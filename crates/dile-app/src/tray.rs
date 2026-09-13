//! The tray icon, its menu, and the one thing it says about the session.
//!
//! The tray is the whole of Dile's permanent interface. There is no main window and there
//! never will be: the product is a hotkey, a strip that appears for a second and a menu with
//! a way into the settings and a way out.
//!
//! **Two menu entries, and both of them are real.** `Settings` opens the window WP5 built —
//! once, whatever a person does to the menu — and `Quit` has been real from the first commit,
//! because a tray application that can only be stopped from Task Manager is one users learn to
//! distrust.
//!
//! **The menu is rebuilt when the language changes**, rather than its items being relabelled
//! one by one. `docs/PROJECT.md` §6 WP6 asks for a language switch without a restart, and the
//! handler lives on the tray icon rather than on the menu — so a new menu inherits it and the
//! entries keep working.
//!
//! **The icon carries the state.** WP2 gives the tray three icons — the same waveform mark
//! with its accent bar in the idle green, in red while recording and in amber while working
//! — because the panel of `docs/PROJECT.md` §3 does not exist yet and, when it does, it will
//! be hidden most of the time. The tooltip says the same thing in words, from `locales/`.
//! `docs/BUILDING.md` carries the command that regenerates the three files.
//!
//! **Failures here are reported, never fatal.** A tray that cannot repaint is a cosmetic
//! problem in a process that is still recording; taking the application down over it would
//! turn a wrong colour into a lost dictation.
//!
//! **A menu means the right button belongs to the shell.** With one attached, Windows opens
//! the menu on right click and leaves the left button to us — which is the button WP5 wires
//! to the panel. Doing both at once is worse than either: the menu takes the focus and the
//! panel blurs behind it.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Manager, Runtime};

use crate::i18n::Strings;

/// Identifier of the one tray icon this application owns.
pub const TRAY_ID: &str = "dile";

/// Menu item: open the settings window.
const MENU_SETTINGS: &str = "dile-settings";
/// Menu item: stop the application.
const MENU_QUIT: &str = "dile-quit";

/// The idle mark: the accent bar in the application's green.
const ICON_IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
/// The recording mark: the accent bar in red.
const ICON_RECORDING: &[u8] = include_bytes!("../icons/tray-recording.png");
/// The working mark: the accent bar in amber.
const ICON_WORKING: &[u8] = include_bytes!("../icons/tray-working.png");

/// What the tray is saying about the session.
///
/// Three of these are the states of a dictation and the rest are answers to a question the
/// user is about to ask — *did it hear me?*, *is there a microphone at all?*, *why is this
/// taking so long?*, *why does nothing come out?* Everything that is not an active dictation
/// keeps the idle icon, because a colour with no dictation behind it reads as activity.
///
/// **Five of these arrived with WP3**, because the engine can be in a state the session has
/// no words for: no weights, weights arriving, a tier that lost its probe, a process that
/// will not stay up. `docs/PROJECT.md` §6 WP3 asks for exactly that — a machine on the
/// fallback tier "says so" — and the tray is where it says it until WP5 builds somewhere
/// better.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing is happening, and the hotkey is armed.
    Idle,
    /// The key is down and audio is being captured.
    Recording,
    /// The key is up and the recording is being transcribed.
    Working,
    /// The recording held no speech, so nothing was sent anywhere.
    ///
    /// It stays on the tray until the next press rather than reverting after a moment: it is
    /// the only feedback there is until WP5's panel exists, and feedback nobody can read in
    /// time is not feedback.
    NothingHeard,
    /// The microphone could not be opened. The application keeps running.
    NoMicrophone,
    /// The engine is being brought up: the first-run probe, a model load, a respawn.
    Preparing,
    /// Model weights are arriving, with how far along they are.
    Downloading(u8),
    /// There are no weights on this machine, because the download was declined.
    ///
    /// The hotkey still records; it simply has nowhere to send what it captured, and the log
    /// says so on every press rather than the application pretending to work.
    NoModel,
    /// Running on the CPU fallback tier, because the first-run GPU probe did not pass.
    CpuTier,
    /// The engine process went down too many times in a row and is not being restarted.
    EngineFailed,
}

impl Status {
    /// The name this state travels under in the `dile://state` event.
    ///
    /// Stable, because WP5's panel switches on these strings: they are an interface, not a
    /// debugging convenience.
    #[must_use]
    pub const fn event_state(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Recording => "recording",
            Status::Working => "working",
            Status::NothingHeard => "nothing-heard",
            Status::NoMicrophone => "no-microphone",
            Status::Preparing => "preparing",
            Status::Downloading(_) => "downloading",
            Status::NoModel => "no-model",
            Status::CpuTier => "cpu-tier",
            Status::EngineFailed => "engine-failed",
        }
    }

    /// The catalogue key of this state's tooltip.
    const fn tooltip_key(self) -> &'static str {
        match self {
            Status::Idle => "tray.tooltip.idle",
            Status::Recording => "tray.tooltip.recording",
            Status::Working => "tray.tooltip.working",
            Status::NothingHeard => "tray.tooltip.nothing",
            Status::NoMicrophone => "tray.tooltip.nomicrophone",
            Status::Preparing => "tray.tooltip.preparing",
            Status::Downloading(_) => "tray.tooltip.downloading",
            Status::NoModel => "tray.tooltip.nomodel",
            Status::CpuTier => "tray.tooltip.cputier",
            Status::EngineFailed => "tray.tooltip.enginefailed",
        }
    }

    /// The PNG this state paints on the tray.
    const fn icon_bytes(self) -> &'static [u8] {
        match self {
            Status::Idle
            | Status::NothingHeard
            | Status::NoMicrophone
            | Status::NoModel
            | Status::CpuTier
            | Status::EngineFailed => ICON_IDLE,
            Status::Recording => ICON_RECORDING,
            // The engine doing something the user is waiting for is the same amber as a
            // dictation being transcribed, because from the tray they are the same thing:
            // the application is busy and the answer is not here yet.
            Status::Working | Status::Preparing | Status::Downloading(_) => ICON_WORKING,
        }
    }

    /// This state's tooltip, in the user's language, naming the chord the user chose.
    fn tooltip(self, strings: &Strings, hotkey: &str) -> String {
        // Every tooltip is offered both holes; `interpolate` leaves alone the ones a given
        // string does not have, so there is no per-state parameter list to keep in step.
        let percent = match self {
            Status::Downloading(percent) => percent.to_string(),
            _ => String::new(),
        };
        strings.format(
            self.tooltip_key(),
            &[("hotkey", hotkey), ("percent", percent.as_str())],
        )
    }
}

/// Build the tray icon and attach its menu.
///
/// Called once from the setup hook, with the application idle: the hotkey listener starts
/// after it, so the first thing the user sees is the state the application is really in.
pub fn create(app: &App, strings: &Strings, hotkey: &str) -> tauri::Result<()> {
    let menu = build_menu(app, strings)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(Status::Idle.icon_bytes())?)
        .tooltip(Status::Idle.tooltip(strings, hotkey))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SETTINGS => crate::settings::window::open(app),
            MENU_QUIT => app.exit(0),
            // Every id this menu builds is handled above, so reaching here means a future
            // menu item whose arm was forgotten.
            other => log::warn!("a tray menu item has no handler: {other}"),
        })
        .build(app)?;

    Ok(())
}

/// The two entries, in the language of the moment.
fn build_menu<R: Runtime, M: Manager<R>>(manager: &M, strings: &Strings) -> tauri::Result<Menu<R>> {
    let settings = MenuItem::with_id(
        manager,
        MENU_SETTINGS,
        strings.text("tray.menu.settings"),
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(manager)?;
    let quit = MenuItem::with_id(
        manager,
        MENU_QUIT,
        strings.text("tray.menu.quit"),
        true,
        None::<&str>,
    )?;
    Menu::with_items(manager, &[&settings, &separator, &quit])
}

/// Put the menu back in a new language.
///
/// A whole new menu rather than two `set_text` calls: the separator and the order are part of
/// the menu, the handler is not — it belongs to the tray icon — and rebuilding is the one
/// operation that cannot leave an entry in the previous language.
pub fn relabel(app: &AppHandle, strings: &Strings) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        log::warn!("no tray icon to put a new menu on");
        return;
    };
    match build_menu(app, strings) {
        Ok(menu) => {
            if let Err(error) = tray.set_menu(Some(menu)) {
                log::warn!("the tray menu could not be replaced: {error}");
            }
        }
        Err(error) => log::warn!("the tray menu could not be rebuilt: {error}"),
    }
}

/// Put a state on the tray: its icon, and its tooltip in the user's language.
///
/// Safe to call from any thread — Tauri dispatches the change to the main one. Every failure
/// is logged and swallowed, because none of them is worth ending a session over.
pub fn show(app: &AppHandle, strings: &Strings, hotkey: &str, status: Status) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        log::warn!("no tray icon to show the {} state on", status.event_state());
        return;
    };

    match Image::from_bytes(status.icon_bytes()) {
        Ok(icon) => {
            if let Err(error) = tray.set_icon(Some(icon)) {
                log::warn!("the tray icon could not be changed: {error}");
            }
        }
        Err(error) => log::warn!("a tray icon could not be decoded: {error}"),
    }

    if let Err(error) = tray.set_tooltip(Some(status.tooltip(strings, hotkey))) {
        log::warn!("the tray tooltip could not be changed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{ICON_IDLE, ICON_RECORDING, ICON_WORKING, Status};
    use crate::i18n::Strings;

    const EVERY_STATUS: [Status; 10] = [
        Status::Idle,
        Status::Recording,
        Status::Working,
        Status::NothingHeard,
        Status::NoMicrophone,
        Status::Preparing,
        Status::Downloading(42),
        Status::NoModel,
        Status::CpuTier,
        Status::EngineFailed,
    ];

    #[test]
    fn every_state_has_a_tooltip_in_both_languages() {
        for language in ["en", "tr"] {
            let strings = Strings::for_locale(language);
            for status in EVERY_STATUS {
                let tooltip = status.tooltip(&strings, "Ctrl+Alt+Space");
                // A key with no string behind it renders as the key itself, which is exactly
                // the failure this catches.
                assert_ne!(
                    tooltip,
                    status.tooltip_key(),
                    "{language}: {} has no tooltip",
                    status.event_state()
                );
                assert!(
                    !tooltip.contains('{'),
                    "{language}: a tooltip kept an unfilled hole: {tooltip}"
                );
            }
        }
    }

    #[test]
    fn the_three_icons_are_pngs_and_no_two_are_the_same_file() {
        const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
        for icon in [ICON_IDLE, ICON_RECORDING, ICON_WORKING] {
            assert!(icon.starts_with(PNG_MAGIC));
        }
        assert_ne!(ICON_IDLE, ICON_RECORDING);
        assert_ne!(ICON_RECORDING, ICON_WORKING);
        assert_ne!(ICON_IDLE, ICON_WORKING);
    }

    #[test]
    fn the_download_tooltip_carries_the_number_and_the_others_do_not_go_looking_for_one() {
        for language in ["en", "tr"] {
            let strings = Strings::for_locale(language);
            let tooltip = Status::Downloading(42).tooltip(&strings, "Ctrl+Alt+Space");
            assert!(
                tooltip.contains("42"),
                "{language}: the download tooltip lost its percentage: {tooltip}"
            );
            // Nothing else has a hole for it, so nothing else may end up with a stray number.
            assert!(
                !Status::Idle
                    .tooltip(&strings, "Ctrl+Alt+Space")
                    .contains("42")
            );
        }
    }

    #[test]
    fn the_state_names_the_panel_switches_on_are_unique() {
        let mut names: Vec<&str> = EVERY_STATUS
            .iter()
            .map(|status| status.event_state())
            .collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }
}
