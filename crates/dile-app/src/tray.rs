//! The tray icon, its menu, and the one thing it says about the session.
//!
//! The tray is the whole of Dile's permanent interface. There is no main window and there
//! never will be: the product is a hotkey, a strip that appears for a second and a menu with
//! a way into the settings and a way out.
//!
//! **Two menu entries, and one of them is a placeholder.** `Settings` has no window to open
//! until WP5 and does nothing rather than opening something empty; `Quit` is real from the
//! first commit, because a tray application that can only be stopped from Task Manager is one
//! users learn to distrust.
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
use tauri::{App, AppHandle};

use crate::i18n::Strings;

/// Identifier of the one tray icon this application owns.
pub const TRAY_ID: &str = "dile";

/// Menu item: open the settings window. WP5.
const MENU_SETTINGS: &str = "dile-settings";
/// Menu item: stop the application.
const MENU_QUIT: &str = "dile-quit";

/// The hotkey shown in the tooltip.
///
/// A constant rather than a reading of [`crate::config::AppConfig`], because a chord has no
/// display form yet: `dile_hotkey::Chord` is a set of modifier families and a key, and
/// turning one into `Ctrl+Alt+Space` belongs with the settings window (WP5), where the user
/// can also change it. Until then this is the chord the application actually registers, so
/// the tooltip says something true.
const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";

/// The idle mark: the accent bar in the application's green.
const ICON_IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
/// The recording mark: the accent bar in red.
const ICON_RECORDING: &[u8] = include_bytes!("../icons/tray-recording.png");
/// The working mark: the accent bar in amber.
const ICON_WORKING: &[u8] = include_bytes!("../icons/tray-working.png");

/// What the tray is saying about the session.
///
/// Three of these are the states of a dictation; two are answers to a question the user is
/// about to ask — *did it hear me?* and *is there a microphone at all?* Both of those keep
/// the idle icon, because a colour with no dictation behind it reads as activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing is happening, and the hotkey is armed.
    Idle,
    /// The key is down and audio is being captured.
    Recording,
    /// The key is up and the recording is on its way. WP3 is what makes this last longer
    /// than a blink.
    Working,
    /// The recording held no speech, so nothing was sent anywhere.
    ///
    /// It stays on the tray until the next press rather than reverting after a moment: it is
    /// the only feedback there is until WP5's panel exists, and feedback nobody can read in
    /// time is not feedback.
    NothingHeard,
    /// The microphone could not be opened. The application keeps running.
    NoMicrophone,
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
        }
    }

    /// The PNG this state paints on the tray.
    const fn icon_bytes(self) -> &'static [u8] {
        match self {
            Status::Idle | Status::NothingHeard | Status::NoMicrophone => ICON_IDLE,
            Status::Recording => ICON_RECORDING,
            Status::Working => ICON_WORKING,
        }
    }

    /// This state's tooltip, in the user's language.
    fn tooltip(self, strings: &Strings) -> String {
        // Every tooltip is offered the hotkey; only the idle one has a hole for it, and
        // `interpolate` leaves the rest alone.
        strings.format(self.tooltip_key(), &[("hotkey", DEFAULT_HOTKEY)])
    }
}

/// Build the tray icon and attach its menu.
///
/// Called once from the setup hook, with the application idle: the hotkey listener starts
/// after it, so the first thing the user sees is the state the application is really in.
pub fn create(app: &App, strings: &Strings) -> tauri::Result<()> {
    let settings = MenuItem::with_id(
        app,
        MENU_SETTINGS,
        strings.text("tray.menu.settings"),
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(
        app,
        MENU_QUIT,
        strings.text("tray.menu.quit"),
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&settings, &separator, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(Status::Idle.icon_bytes())?)
        .tooltip(Status::Idle.tooltip(strings))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            // TODO(WP5): open the settings window here. Deliberately not an empty window: a
            // menu entry that opens nothing is a bug report, one that does nothing yet is a
            // skeleton. The arm is written out rather than folded into the catch-all so
            // that WP5 has one obvious place to fill in.
            MENU_SETTINGS => {}
            MENU_QUIT => app.exit(0),
            // Every id this menu builds is handled above, so reaching here means a future
            // menu item whose arm was forgotten.
            other => log::warn!("a tray menu item has no handler: {other}"),
        })
        .build(app)?;

    Ok(())
}

/// Put a state on the tray: its icon, and its tooltip in the user's language.
///
/// Safe to call from any thread — Tauri dispatches the change to the main one. Every failure
/// is logged and swallowed, because none of them is worth ending a session over.
pub fn show(app: &AppHandle, strings: &Strings, status: Status) {
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

    if let Err(error) = tray.set_tooltip(Some(status.tooltip(strings))) {
        log::warn!("the tray tooltip could not be changed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{ICON_IDLE, ICON_RECORDING, ICON_WORKING, Status};
    use crate::i18n::Strings;

    const EVERY_STATUS: [Status; 5] = [
        Status::Idle,
        Status::Recording,
        Status::Working,
        Status::NothingHeard,
        Status::NoMicrophone,
    ];

    #[test]
    fn every_state_has_a_tooltip_in_both_languages() {
        for language in ["en", "tr"] {
            let strings = Strings::for_locale(language);
            for status in EVERY_STATUS {
                let tooltip = status.tooltip(&strings);
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
