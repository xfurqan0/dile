//! Everything the user gets to change, and the file it lives in.
//!
//! WP0 put these values in `config.rs` with no file behind them and a note saying WP5 would
//! be the file reader and the form. This is that struct: what `<app config>/settings.json`
//! holds, how it is read, and what it is clamped to. **Who watches it for changes is not
//! here** — `dile-app`'s `settings::store` is the tray's business, and the command line has
//! no use for it: it reads the file once and exits.
//!
//! **Every field is `#[serde(default)]`, and that is the migration policy.** A settings file
//! written by an older build is missing whatever was added since; a file written by a newer
//! one carries fields this build has never heard of. Both must load, because the alternative
//! is a person losing their dictionary to a downgrade. So a missing field takes the shipped
//! default, an unknown field is ignored, and `version` exists to be read by a future
//! migration rather than to reject anything today.
//!
//! **A value out of range is clamped, never rejected.** [`Settings::validated`] moves a cap
//! of 600 s to 300 and says so in the log. A settings file that refuses to load is a tray
//! application that will not start, and no bad number is worth that.
//!
//! **The write is atomic.** [`Settings::save`] writes a temporary file beside the real one
//! and renames it over the top, because the alternative — truncate, then write — loses the
//! whole file if the machine goes down in between, and this one holds the dictionary.
//!
//! **`engine.json` stays where it is.** The tier record is written by the first-run probe on
//! the engine's own thread and read before any window exists; folding it in here would mean
//! the probe writing the user's dictionary back out on a path that has nothing to do with
//! it. `engine.tier_override` is the settings half of that decision: what the *user* said,
//! against what the probe *found*. [`crate::tier`] is the record itself.

use std::fs;
use std::path::Path;

use dile_capture::{CaptureConfig, MAX_CAP_SECS, MIN_CAP_SECS};
use dile_core::cleanup::Strictness;
use dile_core::dictionary::{Dictionary, Entry};
use dile_engine_proto::Device;
use dile_hotkey::{Chord, HotkeyConfig, Mode, ModifierOnly};
use serde::{Deserialize, Serialize};

/// The file the settings live in, inside the application's config directory.
///
/// The same directory as `engine.json`, which is where a user looking for one will look for
/// the other.
pub const FILE: &str = "settings.json";

/// The schema version this build writes.
///
/// Read by a future migration and by nothing today. It is here from the first release so
/// that the first migration has a number to branch on instead of guessing from which fields
/// are present.
pub const VERSION: u32 = 1;

/// The default chord, as it is written down.
///
/// `docs/PROJECT.md` §7: `Ctrl+Alt+Space`. It is not an IME toggle, not reserved by Windows,
/// and no mainstream editor binds it.
pub const DEFAULT_CHORD: &str = "Ctrl+Alt+Space";

/// The chord that is excluded permanently, whatever a settings file says.
///
/// `docs/PROJECT.md` §3: the IME swallows it before a low-level hook ever sees it, so a user
/// who set it would have a product that never hears them.
pub const EXCLUDED_CHORD: &str = "Ctrl+Space";

/// The recording cap the product ships with, in seconds.
pub const DEFAULT_CAP_SECS: u32 = 60;

/// How much audio from before the key went down is kept, in milliseconds.
///
/// First-word protection. A push-to-talk key reaches the application after the first
/// syllable has begun — key travel, the low-level hook and the event loop are each a few
/// milliseconds — and half a second covers the hand as well as the wire.
pub const DEFAULT_PRE_ROLL_MS: u32 = 500;

/// The longest pre-roll a settings file may ask for, in milliseconds.
pub const MAX_PRE_ROLL_MS: u32 = 5_000;

/// How often the level meter produces a reading, in milliseconds.
///
/// Not a setting: it is the panel's frame rate, and a user has no reason to have an opinion
/// about it. WP2's acceptance criterion is "level events arrive at ≥ 20 Hz", and 50 ms is
/// exactly that.
pub const LEVEL_INTERVAL_MS: u32 = 50;

/// How long the panel waits before transferring by itself, in milliseconds.
///
/// `docs/PROJECT.md` §3: "auto-transferred after 2.5 s unless the user presses ✗ or starts
/// editing".
///
/// **1.5 s was the designed number and 2.5 s is the measured one.** The maintainer's hand
/// test of 2026-09-13 — the one WP5b was waiting for — passed on every target it was run
/// against and asked for exactly one change: the countdown was too fast to read a sentence
/// in before it fired. This is a default, not a rule: a `settings.json` that already carries
/// a number keeps it, and only a new installation starts here.
pub const DEFAULT_AUTO_TRANSFER_MS: u32 = 2_500;

/// The shortest auto-transfer delay the settings accept, in milliseconds.
///
/// Half a second is already too fast to read a sentence in; below it the countdown is a
/// flicker and the review the panel exists for has been removed by a number field.
pub const MIN_AUTO_TRANSFER_MS: u32 = 500;

/// The longest auto-transfer delay the settings accept, in milliseconds.
///
/// Five seconds. Past that, switching auto-transfer off is what the user actually means.
pub const MAX_AUTO_TRANSFER_MS: u32 = 5_000;

/// The language Dile dictates in.
///
/// `docs/PROJECT.md` §3 gives the engine a language hint on every request rather than
/// letting it autodetect. Turkish is the product's first language and the one every M0
/// number was measured in. Not a setting in v1: the UI language is, and this is not.
pub const DICTATION_LANGUAGE: &str = "tr";

/// How a press is interpreted. The settings half of [`dile_hotkey::Mode`].
///
/// Written again here rather than serialized from the hotkey crate, because the stored
/// spelling of a setting is this file's contract with a settings file on somebody's disk,
/// and a library crate must be free to rename its own variants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyMode {
    /// Hold to talk: the recording lives exactly as long as the press.
    #[default]
    Hold,
    /// Tap to start, tap to stop.
    Toggle,
}

impl From<HotkeyMode> for Mode {
    fn from(mode: HotkeyMode) -> Self {
        match mode {
            HotkeyMode::Hold => Mode::Hold,
            HotkeyMode::Toggle => Mode::Toggle,
        }
    }
}

/// Which cleanup rules run. The settings half of [`dile_core::cleanup::Strictness`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupLevel {
    /// Fillers and whole-segment hallucinations only.
    Light,
    /// The default: light, plus casing, punctuation and dictionary spelling.
    #[default]
    Medium,
    /// Medium, plus repeat collapsing and clause-edge particles.
    Strict,
}

impl From<CleanupLevel> for Strictness {
    fn from(level: CleanupLevel) -> Self {
        match level {
            CleanupLevel::Light => Strictness::Light,
            CleanupLevel::Medium => Strictness::Medium,
            CleanupLevel::Strict => Strictness::Strict,
        }
    }
}

/// Which language the interface is in.
///
/// EN + TR only in v1 (`docs/PROJECT.md` §3, UI languages), plus the one that is not a
/// language: follow the operating system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Whatever Windows is set to, falling back to English.
    #[default]
    Auto,
    /// Turkish.
    Tr,
    /// English.
    En,
}

impl Language {
    /// The locale tag to load, or `None` for "ask the operating system".
    #[must_use]
    pub const fn tag(self) -> Option<&'static str> {
        match self {
            Language::Auto => None,
            Language::Tr => Some("tr"),
            Language::En => Some("en"),
        }
    }
}

/// The trigger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeySettings {
    /// The chord, as [`dile_hotkey::Chord`] writes it: `Ctrl+Alt+Space`.
    pub chord: String,
    /// Hold to talk, or tap to toggle.
    pub mode: HotkeyMode,
    /// Whether the optional second trigger — the right Ctrl, on its own — is armed.
    ///
    /// A `bool` rather than a key name because §3 names the key: switching it on claims that
    /// key for dictation, and the only key worth claiming is the side almost nothing else is
    /// bound to.
    pub second_key: bool,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        HotkeySettings {
            chord: DEFAULT_CHORD.to_owned(),
            mode: HotkeyMode::Hold,
            second_key: false,
        }
    }
}

/// The microphone and the recording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    /// How long one recording may run, in seconds. 1–300.
    pub cap_secs: u32,
    /// How much audio from before the key went down is kept, in milliseconds.
    pub pre_roll_ms: u32,
    /// The input device id, or `None` for the host default.
    pub device: Option<String>,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        CaptureSettings {
            cap_secs: DEFAULT_CAP_SECS,
            pre_roll_ms: DEFAULT_PRE_ROLL_MS,
            device: None,
        }
    }
}

/// The Turkish layer and what happens to a finished dictation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupSettings {
    /// Which rules run.
    pub strictness: CleanupLevel,
    /// Whether the panel transfers by itself after [`CleanupSettings::auto_transfer_ms`].
    pub auto_transfer: bool,
    /// How long it waits first, in milliseconds. 500–5000.
    pub auto_transfer_ms: u32,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        CleanupSettings {
            strictness: CleanupLevel::Medium,
            auto_transfer: true,
            auto_transfer_ms: DEFAULT_AUTO_TRANSFER_MS,
        }
    }
}

/// What the user said about the engine, as against what the probe found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    /// The tier to use whatever the probe decided, or `None` to follow the probe.
    ///
    /// `docs/PROJECT.md` §3: "The tier stays visible and switchable in settings." Setting it
    /// writes the tier into `engine.json` and restarts the engine on it **without probing**
    /// — a person who has overridden the answer is not asking for the question again.
    pub tier_override: Option<Device>,
}

/// One dictionary entry, as the settings file stores it.
///
/// The same three fields as [`dile_core::dictionary::Entry`], written again because this is
/// a file format: `Entry` is free to grow a field that has no business appearing in
/// somebody's `settings.json` the next time it is saved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictionaryEntry {
    /// The spelling the user wants, exactly as it must appear in the transcript.
    pub canonical: String,
    /// The forms the engine produces instead.
    pub variants: Vec<String>,
    /// Keep this term at the front of the engine prompt when the budget is tight.
    pub pinned: bool,
}

/// The window and the operating system.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Which language the interface is in.
    pub language: Language,
    /// Whether Dile starts with Windows.
    pub autostart: bool,
    /// Where the panel sits on each monitor, keyed by the monitor's name.
    ///
    /// Empty until WP5b, which owns the panel and is the only thing that can know where a
    /// person dragged it. It is declared now because the settings file is written by this
    /// package and read by that one, and a field added later is a field missing from every
    /// file already on disk.
    pub panel_position: std::collections::BTreeMap<String, PanelPosition>,
}

/// A remembered panel position on one monitor, in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelPosition {
    /// Distance from the left edge of the monitor.
    pub x: i32,
    /// Distance from the top edge of the monitor.
    pub y: i32,
}

/// The whole of Dile's configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The schema version. See [`VERSION`].
    pub version: u32,
    /// The trigger.
    pub hotkey: HotkeySettings,
    /// The microphone and the recording.
    pub capture: CaptureSettings,
    /// The Turkish layer and the transfer.
    pub cleanup: CleanupSettings,
    /// The engine tier.
    pub engine: EngineSettings,
    /// The user's own terms, which also become the engine's initial prompt.
    pub dictionary: Vec<DictionaryEntry>,
    /// The interface.
    pub ui: UiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: VERSION,
            hotkey: HotkeySettings::default(),
            capture: CaptureSettings::default(),
            cleanup: CleanupSettings::default(),
            engine: EngineSettings::default(),
            dictionary: Vec::new(),
            ui: UiSettings::default(),
        }
    }
}

impl Settings {
    /// Read the settings, or the shipped defaults when there is no file yet.
    ///
    /// **A damaged file is the defaults, not an error.** The whole product is a tray icon and
    /// a hotkey; refusing to start over a file somebody hand-edited would be the wrong trade
    /// every time. The failure is logged, and the file is left alone rather than overwritten,
    /// so a person who broke their dictionary by hand can still open it in an editor.
    #[must_use]
    pub fn load(path: &Path) -> Settings {
        let Ok(text) = fs::read_to_string(path) else {
            log::info!(
                "no settings file at {}; the defaults are in use",
                path.display()
            );
            return Settings::default();
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => {
                log::info!(
                    "settings loaded from {}: version {}, {} dictionary entries",
                    path.display(),
                    settings.version,
                    settings.dictionary.len()
                );
                settings.validated()
            }
            Err(error) => {
                log::warn!(
                    "{} did not parse, so the defaults are in use and the file is left alone: {error}",
                    path.display()
                );
                Settings::default()
            }
        }
    }

    /// Write the settings, creating the directory if it is not there.
    ///
    /// Temporary file and rename, never a truncate in place: this file holds the dictionary,
    /// and a machine that goes down mid-write must lose the change rather than the file.
    ///
    /// # Errors
    ///
    /// The directory could not be created, the file could not be written or renamed, or the
    /// settings would not serialize.
    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text)?;
        // `fs::rename` replaces an existing destination on every platform this product
        // targets, which is the whole point: the reader sees the old file or the new one.
        fs::rename(&temporary, path)
    }

    /// The settings as they will actually be used, with every out-of-range value clamped.
    ///
    /// Each clamp says so in the log, because a number that silently became a different
    /// number is how a bug report starts with "I set it to 600".
    #[must_use]
    pub fn validated(mut self) -> Settings {
        let cap = self.capture.cap_secs.clamp(MIN_CAP_SECS, MAX_CAP_SECS);
        if cap != self.capture.cap_secs {
            log::warn!(
                "recording cap of {} s is outside {MIN_CAP_SECS}..={MAX_CAP_SECS} s and was clamped to {cap} s",
                self.capture.cap_secs
            );
            self.capture.cap_secs = cap;
        }

        let pre_roll = self.capture.pre_roll_ms.min(MAX_PRE_ROLL_MS);
        if pre_roll != self.capture.pre_roll_ms {
            log::warn!(
                "pre-roll of {} ms is longer than the {MAX_PRE_ROLL_MS} ms maximum and was clamped",
                self.capture.pre_roll_ms
            );
            self.capture.pre_roll_ms = pre_roll;
        }

        let delay = self
            .cleanup
            .auto_transfer_ms
            .clamp(MIN_AUTO_TRANSFER_MS, MAX_AUTO_TRANSFER_MS);
        if delay != self.cleanup.auto_transfer_ms {
            log::warn!(
                "auto-transfer delay of {} ms is outside {MIN_AUTO_TRANSFER_MS}..={MAX_AUTO_TRANSFER_MS} ms and was clamped to {delay} ms",
                self.cleanup.auto_transfer_ms
            );
            self.cleanup.auto_transfer_ms = delay;
        }

        if self.hotkey.chord.parse::<Chord>().is_err() {
            log::warn!(
                "the stored chord {:?} is not one this build can read, so the default is in use",
                self.hotkey.chord
            );
            self.hotkey.chord = DEFAULT_CHORD.to_owned();
        }

        // A dictionary entry with no canonical spelling has nothing to correct anything to
        // and would put an empty term in the engine's prompt.
        let before = self.dictionary.len();
        self.dictionary
            .retain(|entry| !entry.canonical.trim().is_empty());
        if self.dictionary.len() != before {
            log::warn!(
                "{} dictionary entries had no canonical spelling and were dropped",
                before - self.dictionary.len()
            );
        }

        self
    }

    /// The chord, parsed. Falls back to the default, which [`Settings::validated`] has
    /// already put in place for anything that was stored wrong.
    #[must_use]
    pub fn chord(&self) -> Chord {
        self.hotkey
            .chord
            .parse()
            .unwrap_or_else(|_| Chord::ctrl_alt_space())
    }

    /// The trigger, as `dile-hotkey` wants it.
    #[must_use]
    pub fn hotkey_config(&self) -> HotkeyConfig {
        HotkeyConfig {
            mode: self.hotkey.mode.into(),
            primary: self.chord(),
            second_key: self.hotkey.second_key.then_some(ModifierOnly::RIGHT_CTRL),
            ..HotkeyConfig::default()
        }
    }

    /// The microphone, as `dile-capture` wants it.
    #[must_use]
    pub fn capture_config(&self) -> CaptureConfig {
        CaptureConfig {
            device: self.capture.device.clone(),
            pre_roll_ms: self.capture.pre_roll_ms,
            cap_secs: self.capture.cap_secs,
            level_interval_ms: LEVEL_INTERVAL_MS,
        }
    }

    /// The recording cap in milliseconds, which is the unit the panel is told it in.
    #[must_use]
    pub const fn cap_ms(&self) -> u64 {
        self.capture.cap_secs as u64 * 1_000
    }

    /// Which cleanup rules run.
    #[must_use]
    pub fn strictness(&self) -> Strictness {
        self.cleanup.strictness.into()
    }

    /// The user's terms, as `dile-core` wants them.
    #[must_use]
    pub fn dictionary(&self) -> Dictionary {
        Dictionary::from_entries(self.dictionary.iter().map(|entry| Entry {
            canonical: entry.canonical.clone(),
            variants: entry.variants.clone(),
            pinned: entry.pinned,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CleanupLevel, DEFAULT_AUTO_TRANSFER_MS, DEFAULT_CAP_SECS, DEFAULT_CHORD,
        DEFAULT_PRE_ROLL_MS, DictionaryEntry, HotkeyMode, LEVEL_INTERVAL_MS, Language,
        MAX_AUTO_TRANSFER_MS, MAX_PRE_ROLL_MS, Settings, VERSION,
    };
    use dile_core::cleanup::Strictness;
    use dile_engine_proto::Device;
    use dile_hotkey::{Chord, Mode, ModifierOnly};

    fn temporary_directory(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "dile-settings-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn the_shipped_defaults_are_the_ones_the_decisions_named() {
        let settings = Settings::default();

        assert_eq!(settings.version, VERSION);
        assert_eq!(settings.hotkey.chord, DEFAULT_CHORD);
        assert_eq!(settings.chord(), Chord::ctrl_alt_space());
        assert_eq!(settings.hotkey.mode, HotkeyMode::Hold);
        assert!(!settings.hotkey.second_key);

        assert_eq!(settings.capture.cap_secs, DEFAULT_CAP_SECS);
        assert_eq!(settings.capture.pre_roll_ms, DEFAULT_PRE_ROLL_MS);
        assert_eq!(settings.capture.device, None);
        assert_eq!(settings.cap_ms(), 60_000);

        assert_eq!(settings.cleanup.strictness, CleanupLevel::Medium);
        assert_eq!(settings.strictness(), Strictness::Medium);
        assert!(settings.cleanup.auto_transfer);
        assert_eq!(settings.cleanup.auto_transfer_ms, DEFAULT_AUTO_TRANSFER_MS);

        assert_eq!(settings.engine.tier_override, None);
        assert_eq!(settings.ui.language, Language::Auto);
        assert!(!settings.ui.autostart);

        // "The shipped dictionary is empty" (docs/PROJECT.md WP4): every entry is a claim
        // about one person's vocabulary, so there is no prompt until a person writes one.
        assert!(settings.dictionary.is_empty());
        assert!(settings.dictionary().is_empty());
        assert_eq!(settings.dictionary().prompt(), None);

        // Not a setting, and the acceptance criterion of WP2 is why.
        let capture = settings.capture_config().clamped();
        assert_eq!(capture.level_interval_ms, LEVEL_INTERVAL_MS);
        assert!(1000 / capture.level_interval_ms >= 20);
    }

    #[test]
    fn the_two_crates_get_the_configuration_they_expect() {
        let mut settings = Settings::default();
        settings.hotkey.mode = HotkeyMode::Toggle;
        settings.hotkey.second_key = true;
        settings.hotkey.chord = "Shift+F5".to_owned();
        settings.capture.device = Some("a device id".to_owned());

        let hotkey = settings.hotkey_config();
        assert_eq!(hotkey.mode, Mode::Toggle);
        assert_eq!(hotkey.second_key, Some(ModifierOnly::RIGHT_CTRL));
        assert_eq!(
            hotkey.primary,
            "Shift+F5".parse::<Chord>().expect("a chord")
        );

        let capture = settings.capture_config();
        assert_eq!(capture.device.as_deref(), Some("a device id"));
        assert_eq!(capture.cap_secs, DEFAULT_CAP_SECS);
    }

    #[test]
    fn a_settings_file_survives_the_round_trip() {
        let directory = temporary_directory("round-trip");
        let path = directory.join("settings.json");

        let mut settings = Settings::default();
        settings.capture.cap_secs = 120;
        settings.cleanup.strictness = CleanupLevel::Strict;
        settings.engine.tier_override = Some(Device::Cpu);
        settings.ui.language = Language::Tr;
        settings.ui.autostart = true;
        settings.dictionary = vec![DictionaryEntry {
            // The diacritics are the point of this product; a round trip that folded them
            // would be the one bug the dictionary exists to prevent.
            canonical: "Kubernetes".to_owned(),
            variants: vec!["kubernetis".to_owned(), "küberneteş".to_owned()],
            pinned: true,
        }];

        settings.save(&path).expect("write the settings");
        assert_eq!(Settings::load(&path), settings);

        let written = std::fs::read_to_string(&path).expect("read it back");
        assert!(
            written.contains("küberneteş"),
            "a diacritic was lost on the way out"
        );
        assert!(
            !directory.join("settings.json.tmp").exists(),
            "the temporary file was left behind"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_file_from_an_older_build_loads_and_an_unknown_field_is_ignored() {
        let directory = temporary_directory("partial");
        std::fs::create_dir_all(&directory).expect("create");
        let path = directory.join("settings.json");

        // Half the document is missing and one field belongs to a build that does not exist
        // yet. Both must load: the alternative is a person losing their dictionary to an
        // upgrade or a downgrade.
        std::fs::write(
            &path,
            r#"{"version":1,"capture":{"cap_secs":90},"panel_theme":"aubergine"}"#,
        )
        .expect("write a partial file");

        let settings = Settings::load(&path);
        assert_eq!(settings.capture.cap_secs, 90);
        assert_eq!(settings.capture.pre_roll_ms, DEFAULT_PRE_ROLL_MS);
        assert_eq!(settings.hotkey.chord, DEFAULT_CHORD);
        assert_eq!(settings.cleanup.strictness, CleanupLevel::Medium);
        assert!(settings.dictionary.is_empty());

        // And a file that is not JSON at all is the defaults rather than a failure to start.
        std::fs::write(&path, "{ this is not json").expect("damage the file");
        assert_eq!(Settings::load(&path), Settings::default());
        // The damaged file is left where it is, so a person can open it in an editor.
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn every_number_out_of_range_is_clamped_and_nothing_is_rejected() {
        let mut settings = Settings::default();
        settings.capture.cap_secs = 600;
        settings.capture.pre_roll_ms = 60_000;
        settings.cleanup.auto_transfer_ms = 30_000;
        settings.hotkey.chord = "Ctrl+Banana".to_owned();
        settings.dictionary = vec![
            DictionaryEntry {
                canonical: "  ".to_owned(),
                ..DictionaryEntry::default()
            },
            DictionaryEntry {
                canonical: "cron".to_owned(),
                ..DictionaryEntry::default()
            },
        ];

        let validated = settings.validated();
        assert_eq!(validated.capture.cap_secs, 300);
        assert_eq!(validated.capture.pre_roll_ms, MAX_PRE_ROLL_MS);
        assert_eq!(validated.cleanup.auto_transfer_ms, MAX_AUTO_TRANSFER_MS);
        assert_eq!(validated.hotkey.chord, DEFAULT_CHORD);
        assert_eq!(validated.dictionary.len(), 1);
        assert_eq!(validated.dictionary[0].canonical, "cron");

        // And the low end, which is the one that would divide by zero in a progress bar.
        let mut low = Settings::default();
        low.capture.cap_secs = 0;
        low.cleanup.auto_transfer_ms = 1;
        let validated = low.validated();
        assert_eq!(validated.capture.cap_secs, 1);
        assert_eq!(validated.cleanup.auto_transfer_ms, 500);
    }

    #[test]
    fn the_dictionary_reaches_the_engine_as_a_prompt() {
        let settings = Settings {
            dictionary: vec![
                DictionaryEntry {
                    canonical: "cron".to_owned(),
                    variants: vec!["kron".to_owned()],
                    pinned: false,
                },
                DictionaryEntry {
                    canonical: "keşfet".to_owned(),
                    variants: Vec::new(),
                    pinned: true,
                },
            ],
            ..Settings::default()
        };

        let dictionary = settings.dictionary();
        assert_eq!(dictionary.len(), 2);
        let prompt = dictionary.prompt().expect("two terms make a prompt");
        assert!(
            prompt.contains("keşfet"),
            "a pinned term leads the prompt: {prompt}"
        );
        assert!(prompt.contains("cron"));
        assert_eq!(dictionary.apply("kron görevi"), "cron görevi");
    }
}
