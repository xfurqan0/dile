//! The one way anything in this application reaches the user.
//!
//! Two threads had this to themselves in WP2 — the session and the level forwarder — and
//! WP3 adds a third, the engine supervisor, which has its own states to report and its own
//! reasons to change the tray. Three threads writing to the tray through three copies of
//! the same three lines is how a product ends up with a tooltip that contradicts its icon,
//! so the seam is a type: [`Ui`] is cheap to clone, safe to call from any thread, and the
//! only thing in this crate that knows both the tray and the panel exist.
//!
//! **The resting state is a value, not a constant.** WP2 could show `Idle` whenever a
//! dictation ended, because idle was the only thing the application could be when it was
//! not recording. It is not any more: a machine whose GPU probe failed rests on the CPU
//! tier, a machine with no model rests with nothing to dictate into, and an engine that
//! will not start rests broken. [`Ui::rest`] shows whichever of those is true, and
//! [`Ui::set_resting`] is how the engine changes the answer — so no caller has to remember
//! which of five states "back to normal" means today.

use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, EventTarget, Manager};

use crate::i18n::Strings;
use crate::tray::{self, Status};

/// The window the review panel lives in. Declared in `tauri.conf.json`, hidden until WP5b.
const PANEL_WINDOW: &str = "panel";

/// One meter reading, on its way to the panel's level bar.
pub const EVENT_LEVEL: &str = "dile://level";
/// One session state change, on its way to the panel.
pub const EVENT_STATE: &str = "dile://state";
/// One engine state change — tier, model, download progress — on its way to the panel.
pub const EVENT_ENGINE: &str = "dile://engine";

/// The payload of [`EVENT_STATE`].
#[derive(Clone, Debug, Serialize)]
pub struct StatePayload {
    /// One of `tray::Status::event_state`.
    pub state: &'static str,
}

/// Everything a background thread needs to reach the application with.
#[derive(Clone)]
pub struct Ui {
    app: AppHandle,
    /// The catalogue, which WP5 made replaceable: the language is a setting now, and
    /// changing it re-renders the tray rather than asking for a restart.
    strings: Arc<RwLock<Arc<Strings>>>,
    /// Whether the panel window exists. Checked once, because the answer cannot change
    /// until WP5 creates windows at run time — and a `get_webview_window` per level event
    /// would be a hash lookup twenty times a second for an answer that is always the same.
    panel: bool,
    /// What the tray goes back to when nothing is happening. See the module documentation.
    resting: Arc<Mutex<Status>>,
    /// The trigger the tooltip names, as the settings spell it — `RightCtrl`, not the
    /// words a person reads.
    ///
    /// Held here rather than read from the settings on every repaint: the tray is repainted
    /// several times per dictation, and what it needs is one short string that changes once
    /// in a blue moon. Stored unspelled because the language is a setting too, and a label
    /// resolved at start-up would still be in the old language after a switch.
    hotkey: Arc<RwLock<String>>,
}

impl Ui {
    /// Take hold of the application, once, at start-up.
    #[must_use]
    pub fn new(
        app: AppHandle,
        strings: Arc<RwLock<Arc<Strings>>>,
        hotkey: Arc<RwLock<String>>,
    ) -> Self {
        let panel = app.get_webview_window(PANEL_WINDOW).is_some();
        if !panel {
            // Not fatal, and not even unusual until WP5: the window is declared in
            // `tauri.conf.json`, so this is a configuration change nobody meant to make.
            log::warn!("no {PANEL_WINDOW} window; level and state events go nowhere");
        }
        Ui {
            app,
            strings,
            panel,
            resting: Arc::new(Mutex::new(Status::Idle)),
            hotkey,
        }
    }

    /// The application handle, for the few things that need more than the tray — the
    /// consent dialog and the two platform directories the engine reads and writes.
    #[must_use]
    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// The catalogue, in the user's language.
    ///
    /// An `Arc` rather than a borrow because the catalogue can be replaced under a caller:
    /// the language is a setting, and a reference handed out across a lock would be a
    /// reference to the language the user just stopped using.
    #[must_use]
    pub fn strings(&self) -> Arc<Strings> {
        match self.strings.read() {
            Ok(strings) => Arc::clone(&strings),
            Err(_) => Arc::new(Strings::for_locale("en")),
        }
    }

    /// The shared catalogue slot, for whoever changes the language.
    #[must_use]
    pub fn catalogue(&self) -> Arc<RwLock<Arc<Strings>>> {
        Arc::clone(&self.strings)
    }

    /// The trigger the tooltip names, in the language the interface is in.
    ///
    /// A chord comes back as it is written; a lone modifier comes back as words, because
    /// `RightCtrl` is a spelling for a settings file and "Right Ctrl" is one for a person.
    /// See [`crate::i18n::trigger_label`].
    #[must_use]
    pub fn hotkey(&self) -> String {
        let trigger = match self.hotkey.read() {
            Ok(hotkey) => hotkey.clone(),
            Err(_) => String::new(),
        };
        crate::i18n::trigger_label(&self.strings(), &trigger)
    }

    /// Show a state on the tray and tell the panel about it.
    pub fn show(&self, status: Status) {
        tray::show(&self.app, &self.strings(), &self.hotkey(), status);
        self.emit(
            EVENT_STATE,
            StatePayload {
                state: status.event_state(),
            },
        );
    }

    /// Change what "nothing is happening" looks like, and show it.
    pub fn set_resting(&self, status: Status) {
        match self.resting.lock() {
            Ok(mut resting) => *resting = status,
            // A poisoned lock means another thread panicked while holding it. The tray is
            // not worth taking the process down over, so the state is dropped and said so.
            Err(_) => log::warn!("the resting state could not be changed"),
        }
        self.rest();
    }

    /// Show whatever "nothing is happening" means on this machine today.
    pub fn rest(&self) {
        let status = match self.resting.lock() {
            Ok(resting) => *resting,
            Err(_) => Status::Idle,
        };
        self.show(status);
    }

    /// Send one event to the panel window, or to nowhere if there is no panel.
    pub fn emit<P: Serialize + Clone>(&self, event: &str, payload: P) {
        if !self.panel {
            return;
        }
        if let Err(error) =
            self.app
                .emit_to(EventTarget::webview_window(PANEL_WINDOW), event, payload)
        {
            // At twenty a second this could fill a log on its own, so it is a debug line:
            // the panel is not listening yet, and a failure here costs one frame of a meter.
            log::debug!("{event} was not delivered: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EVENT_ENGINE, EVENT_LEVEL, EVENT_STATE, StatePayload};
    use crate::tray::Status;

    #[test]
    fn the_three_event_names_are_distinct_and_namespaced() {
        let names = [EVENT_LEVEL, EVENT_STATE, EVENT_ENGINE];
        for name in names {
            assert!(name.starts_with("dile://"), "{name} is not one of ours");
        }
        let mut sorted = names;
        sorted.sort_unstable();
        let count = sorted.len();
        let mut unique = sorted.to_vec();
        unique.dedup();
        assert_eq!(unique.len(), count);
    }

    #[test]
    fn the_state_payload_is_the_shape_wp5_will_read() {
        let state = serde_json::to_value(StatePayload {
            state: Status::Recording.event_state(),
        })
        .expect("a state payload serializes");
        assert_eq!(state["state"], "recording");
    }
}
