//! One copy of the settings, and how everything that cares finds out they changed.
//!
//! WP5's requirement is that a change applies **without restarting the application**. That is
//! not a property of a settings file; it is a property of whoever is holding the old value.
//! Four things hold one — the hotkey listener, the microphone, the engine supervisor and the
//! tray's language — they live on three different threads, and none of them can be asked to
//! poll a file.
//!
//! So the store is the one copy, and a change is an event:
//!
//! ```text
//!   settings window ──set──▶ SettingsStore ──Change──▶ session   (hotkey, microphone)
//!                                 │                 ├─▶ engine    (strictness, dictionary, tier)
//!                                 │                 └─▶ tray      (language)
//!                                 ▼
//!                          settings.json
//! ```
//!
//! **A [`Change`] says what moved, not just that something did.** Re-opening a microphone
//! because somebody typed a dictionary entry would drop the pre-roll ring on every keystroke;
//! re-installing a global keyboard hook because the cleanup level moved would be worse. So
//! the flags are compared field by field and each listener acts on the ones that are its own.
//!
//! **Listeners run on the thread that called [`SettingsStore::set`]** — the command thread —
//! and are expected to be short. The session thread's listener sets a flag its own loop
//! reads, because re-installing a keyboard hook belongs on the thread that owns it.
//!
//! **A failed write is reported and the new values still apply.** The alternative is a
//! settings window where a read-only disk makes the controls stop responding, which tells the
//! user less than a saved-state that says it could not save.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use super::Settings;

/// What moved, and what it now is.
///
/// Every flag is a question a listener asks: *is this mine?* They are computed by comparing
/// the two documents rather than reported by the caller, so a window that sends the whole
/// document on every keystroke — which is exactly what the settings window does — still only
/// re-opens the microphone when the microphone changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The settings as they now are, already validated.
    pub settings: Settings,
    /// The chord, the mode or the second key moved: the listener is re-installed.
    pub hotkey: bool,
    /// The device, the cap or the pre-roll moved: the microphone is re-opened.
    pub capture: bool,
    /// The cleanup level moved: the next dictation is cleaned differently.
    pub cleanup: bool,
    /// The dictionary moved: it is rebuilt, and the next transcription carries a new prompt.
    pub dictionary: bool,
    /// The tier override moved: the engine restarts on the tier the user asked for.
    pub engine: bool,
    /// The interface language moved: the catalogue is reloaded and the tray re-rendered.
    pub language: bool,
    /// The start-with-Windows switch moved: the registry value is written.
    pub autostart: bool,
}

impl Change {
    /// Compare two documents and say what a listener would care about.
    #[must_use]
    pub fn between(before: &Settings, after: &Settings) -> Change {
        Change {
            hotkey: before.hotkey != after.hotkey,
            capture: before.capture != after.capture,
            cleanup: before.cleanup.strictness != after.cleanup.strictness,
            dictionary: before.dictionary != after.dictionary,
            engine: before.engine != after.engine,
            language: before.ui.language != after.ui.language,
            autostart: before.ui.autostart != after.ui.autostart,
            settings: after.clone(),
        }
    }
}

/// A listener's side of the store.
type Listener = Box<dyn Fn(&Change) + Send + Sync + 'static>;

struct Inner {
    /// Where the file is, or `None` for a store that is not backed by one — which is what
    /// the tests use, and what the application falls back to if the platform will not name a
    /// configuration directory.
    path: Option<PathBuf>,
    current: RwLock<Settings>,
    listeners: Mutex<Vec<Listener>>,
}

/// The one copy of the settings, shared by every thread that holds one of their values.
///
/// Cheap to clone; every clone is the same store.
#[derive(Clone)]
pub struct SettingsStore {
    inner: Arc<Inner>,
}

impl SettingsStore {
    /// A store over a document, with or without a file behind it.
    #[must_use]
    pub fn new(path: Option<PathBuf>, settings: Settings) -> Self {
        SettingsStore {
            inner: Arc::new(Inner {
                path,
                current: RwLock::new(settings.validated()),
                listeners: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Read the file at `path` and hold what it said.
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        let settings = Settings::load(&path);
        SettingsStore::new(Some(path), settings)
    }

    /// The settings as they are now.
    ///
    /// A clone rather than a guard: every caller wants a value it can hold across a
    /// transcription or a recording, and a lock held for that long would be a lock held by
    /// the settings window's next keystroke.
    #[must_use]
    pub fn get(&self) -> Settings {
        match self.inner.current.read() {
            Ok(settings) => settings.clone(),
            // A poisoned lock means a listener panicked while the store was being written.
            // The defaults are a worse answer than the truth but a better one than a crash
            // in a tray application, and the log says which happened.
            Err(_) => {
                log::error!("the settings lock is poisoned; the defaults are being used");
                Settings::default()
            }
        }
    }

    /// Where the settings are written, when there is a file.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref()
    }

    /// Ask to be told when something changes.
    ///
    /// The listener runs on the thread that called [`SettingsStore::set`] and should be
    /// short: set a flag, send on a channel, write a registry value.
    pub fn on_change(&self, listener: impl Fn(&Change) + Send + Sync + 'static) {
        match self.inner.listeners.lock() {
            Ok(mut listeners) => listeners.push(Box::new(listener)),
            Err(_) => log::error!("a settings listener could not be registered"),
        }
    }

    /// Replace the settings, write them down, and tell everybody what moved.
    ///
    /// Returns the settings **as they were stored**, which is not always what was passed in:
    /// a value out of range has been clamped by then, and the settings window renders what
    /// comes back rather than what it sent.
    ///
    /// # Errors
    ///
    /// The file could not be written. The new values are in force either way — the error is
    /// about persistence, not about the change.
    pub fn set(&self, next: Settings) -> Result<Settings, std::io::Error> {
        let next = next.validated();

        let change = {
            let Ok(mut current) = self.inner.current.write() else {
                log::error!("the settings lock is poisoned; this change was not applied");
                return Ok(next);
            };
            if *current == next {
                return Ok(next);
            }
            let change = Change::between(&current, &next);
            *current = next.clone();
            change
        };

        let written = match self.inner.path.as_deref() {
            Some(path) => next.save(path),
            None => Ok(()),
        };

        self.notify(&change);

        written.map(|()| next)
    }

    /// Tell every listener what moved.
    fn notify(&self, change: &Change) {
        let Ok(listeners) = self.inner.listeners.lock() else {
            log::error!("the settings listeners could not be reached; nothing was re-applied");
            return;
        };
        for listener in listeners.iter() {
            listener(change);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Change, SettingsStore};
    use crate::settings::{CleanupLevel, DictionaryEntry, Language, Settings};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_listener_is_told_what_moved_and_not_what_did_not() {
        let store = SettingsStore::new(None, Settings::default());
        let seen: Arc<Mutex<Vec<Change>>> = Arc::new(Mutex::new(Vec::new()));

        let recorder = Arc::clone(&seen);
        store.on_change(move |change| {
            recorder
                .lock()
                .expect("an uncontended recorder")
                .push(change.clone());
        });

        let mut next = store.get();
        next.cleanup.strictness = CleanupLevel::Strict;
        store.set(next).expect("a store with no file always writes");

        let changes = seen.lock().expect("read the recorder");
        assert_eq!(changes.len(), 1, "one change, one notification");
        let change = &changes[0];
        assert!(change.cleanup, "the cleanup level is what moved");
        assert!(
            !change.hotkey,
            "the hotkey listener must not be re-installed"
        );
        assert!(!change.capture, "the microphone must not be re-opened");
        assert!(!change.dictionary);
        assert!(!change.language);
        assert_eq!(change.settings.cleanup.strictness, CleanupLevel::Strict);

        // And the store now answers with the new value.
        assert_eq!(store.get().cleanup.strictness, CleanupLevel::Strict);
    }

    #[test]
    fn a_document_that_says_the_same_thing_notifies_nobody() {
        let store = SettingsStore::new(None, Settings::default());
        let calls = Arc::new(AtomicUsize::new(0));

        let counter = Arc::clone(&calls);
        store.on_change(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        // The settings window sends the whole document on every keystroke. One that changed
        // nothing must not re-install a keyboard hook.
        store.set(store.get()).expect("no file to write");
        store.set(Settings::default()).expect("no file to write");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_value_out_of_range_is_clamped_on_the_way_in_and_the_listener_sees_the_clamped_one() {
        let store = SettingsStore::new(None, Settings::default());
        let seen: Arc<Mutex<Option<Change>>> = Arc::new(Mutex::new(None));

        let recorder = Arc::clone(&seen);
        store.on_change(move |change| {
            *recorder.lock().expect("an uncontended recorder") = Some(change.clone());
        });

        let mut next = store.get();
        next.capture.cap_secs = 6_000;
        let stored = store.set(next).expect("no file to write");

        assert_eq!(
            stored.capture.cap_secs, 300,
            "the caller is told what was stored"
        );
        assert_eq!(store.get().capture.cap_secs, 300);
        let change = seen
            .lock()
            .expect("read")
            .clone()
            .expect("a change arrived");
        assert!(change.capture);
        assert_eq!(change.settings.capture.cap_secs, 300);
        assert_eq!(change.settings.cap_ms(), 300_000);
    }

    #[test]
    fn every_flag_is_raised_by_its_own_field_and_by_nothing_else() {
        let base = Settings::default();

        let mut hotkey = base.clone();
        hotkey.hotkey.second_key = true;
        assert!(Change::between(&base, &hotkey).hotkey);

        let mut capture = base.clone();
        capture.capture.device = Some("a microphone".to_owned());
        assert!(Change::between(&base, &capture).capture);

        let mut dictionary = base.clone();
        dictionary.dictionary.push(DictionaryEntry {
            canonical: "cron".to_owned(),
            ..DictionaryEntry::default()
        });
        let change = Change::between(&base, &dictionary);
        assert!(change.dictionary);
        assert!(
            !change.capture,
            "a dictionary entry must not re-open the microphone"
        );

        let mut language = base.clone();
        language.ui.language = Language::Tr;
        assert!(Change::between(&base, &language).language);

        let mut autostart = base.clone();
        autostart.ui.autostart = true;
        assert!(Change::between(&base, &autostart).autostart);

        // The delay the panel reads when it next opens raises no flag at all: it is written
        // to disk and read when the panel opens, and nothing running has to be told.
        let mut delay = base.clone();
        delay.cleanup.auto_transfer_ms = 3_000;
        let quiet = Change::between(&base, &delay);
        assert!(!quiet.hotkey);
        assert!(!quiet.capture);
        assert!(!quiet.cleanup);
        assert!(!quiet.dictionary);
        assert!(!quiet.engine);
        assert!(!quiet.language);
        assert!(!quiet.autostart);
    }

    #[test]
    fn a_store_with_a_file_behind_it_writes_every_change_through() {
        let directory = std::env::temp_dir().join(format!(
            "dile-store-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("settings.json");

        let store = SettingsStore::open(path.clone());
        assert_eq!(store.get(), Settings::default());
        assert!(!path.exists(), "reading a missing file writes nothing");

        let mut next = store.get();
        next.ui.language = Language::Tr;
        store.set(next).expect("write the settings");

        assert_eq!(Settings::load(&path).ui.language, Language::Tr);
        assert_eq!(
            SettingsStore::open(path.clone()).get().ui.language,
            Language::Tr
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}
