//! What the settings window is allowed to ask for, and nothing else.
//!
//! Nine commands, and the list is the whole of the window's power. `capabilities/settings.json`
//! grants it no plugin permission at all — no dialog, no shell, no filesystem, no autostart —
//! because everything it needs is one of these, and a command is a function this application
//! wrote rather than a capability somebody else's crate defines. A webview that could open a
//! native dialog could open one that looks like this application asking a question; a webview
//! that could write the registry could write more of it than the one value a switch means.
//!
//! **Every error is a locale key, not a sentence.** A command that returned English prose
//! would put a hard-coded string in a product whose own test suite fails on one
//! (`crates/dile-app/tests/i18n.rs`). [`CommandError`] carries the key the window looks up and
//! an optional detail for the console, which is where a developer reads and a user does not.
//!
//! **The blocking one is `async`.** A synchronous Tauri command runs on the main thread, and
//! [`capture_hotkey`] waits up to ten seconds for a key press: run there, it would freeze every
//! window in the process, including the one that asked.

use std::collections::BTreeMap;
use std::time::Duration;

use dile_core::normalizer;
use serde::Serialize;
use tauri::{AppHandle, State};

use super::{Settings, SettingsStore, window};
use crate::engine::{EngineClient, EngineSnapshot, models, state};
use crate::ui::Ui;

/// How long [`capture_hotkey`] waits for the user to press something.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// A failure the settings window has to render.
///
/// `key` is a locale key; `detail` is for the console. Nothing here is a sentence a user
/// reads, because every visible word in this product comes from `locales/`.
#[derive(Clone, Debug, Serialize)]
pub struct CommandError {
    /// The locale key of the sentence to show.
    pub key: &'static str,
    /// What actually went wrong, in English, for a bug report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl CommandError {
    /// An error with nothing to add beyond its key.
    const fn plain(key: &'static str) -> Self {
        CommandError { key, detail: None }
    }

    /// An error with the underlying failure attached.
    fn detailed(key: &'static str, detail: &impl std::fmt::Display) -> Self {
        CommandError {
            key,
            detail: Some(detail.to_string()),
        }
    }
}

/// The catalogue, in the language the application is in.
#[derive(Clone, Debug, Serialize)]
pub struct StringsPayload {
    /// The primary subtag of the language these strings are in: `en` or `tr`.
    pub locale: String,
    /// Every key, with the chosen language on top of English.
    pub strings: BTreeMap<String, String>,
}

/// One input device, as the microphone list shows it.
#[derive(Clone, Debug, Serialize)]
pub struct DeviceRow {
    /// The stable handle stored in the settings.
    pub id: String,
    /// The name the operating system gives it.
    pub name: String,
    /// Whether this is the host's default input.
    pub is_default: bool,
}

/// Everything the Engine group shows.
#[derive(Clone, Debug, Serialize)]
pub struct EngineStatus {
    /// What the engine is doing: the state, the tier, the model, the download progress.
    pub engine: EngineSnapshot,
    /// The tier this machine decided on, as `engine.json` records it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_tier: Option<&'static str>,
    /// What the first-run probe saw, when this machine has been probed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<state::ProbeResult>,
    /// When the tier was decided, in seconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<u64>,
    /// The model file each tier runs, so the read-only model line can name one.
    pub models: BTreeMap<&'static str, ModelRow>,
}

/// One model, for the read-only model line.
#[derive(Clone, Debug, Serialize)]
pub struct ModelRow {
    /// The file name on disk.
    pub file: &'static str,
    /// Its size in whole megabytes.
    pub size_mb: u64,
    /// Whether the weights are already here.
    pub present: bool,
}

/// The strings the window renders itself with.
#[tauri::command]
pub fn get_strings(ui: State<'_, Ui>) -> StringsPayload {
    let strings = ui.strings();
    StringsPayload {
        locale: strings.locale().to_owned(),
        strings: strings.all(),
    }
}

/// The settings as they are now.
#[tauri::command]
pub fn get_settings(store: State<'_, SettingsStore>) -> Settings {
    store.get()
}

/// Replace the settings.
///
/// The whole document, every time: the window has no idea which control the user touched and
/// [`super::store::Change`] works that out by comparing. What comes back is what was
/// **stored**, which is not always what was sent — a value out of range has been clamped by
/// then, and the window renders the answer rather than its own question.
///
/// # Errors
///
/// The chord does not parse, is the permanently excluded one, or has no modifier; two
/// dictionary entries claim the same canonical spelling; the file could not be written.
#[tauri::command]
pub fn set_settings(
    settings: Settings,
    store: State<'_, SettingsStore>,
) -> Result<Settings, CommandError> {
    check_chord(&settings.hotkey.chord)?;
    check_dictionary(&settings)?;

    store
        .set(settings)
        .map_err(|error| CommandError::detailed("settings.error.save", &error))
}

/// The microphones this machine has.
///
/// # Errors
///
/// The audio host would not enumerate its devices.
#[tauri::command]
pub fn list_input_devices() -> Result<Vec<DeviceRow>, CommandError> {
    let devices = dile_capture::Capture::devices()
        .map_err(|error| CommandError::detailed("settings.error.devices", &error))?;
    Ok(devices
        .into_iter()
        .map(|device| DeviceRow {
            id: device.id,
            name: device.name,
            is_default: device.is_default,
        })
        .collect())
}

/// What the engine is doing, what the probe found, and which weights are on disk.
#[tauri::command]
pub fn engine_status(app: AppHandle, engine: State<'_, EngineClient>) -> EngineStatus {
    let record = state::tier_path(&app)
        .ok()
        .and_then(|path| state::read_tier(&path));

    let model_dir = models::directory(&app).ok();
    let mut rows = BTreeMap::new();
    for tier in [
        dile_engine_proto::Device::Vulkan,
        dile_engine_proto::Device::Cpu,
    ] {
        let spec = models::for_tier(tier);
        rows.insert(
            tier.as_str(),
            ModelRow {
                file: spec.file,
                size_mb: spec.size_mb(),
                present: model_dir
                    .as_ref()
                    .is_some_and(|directory| directory.join(spec.file).is_file()),
            },
        );
    }

    EngineStatus {
        engine: engine.status(),
        decided_tier: record.as_ref().map(|record| record.tier.as_str()),
        decided_at: record.as_ref().map(|record| record.decided_at),
        probe: record.map(|record| record.probe_result),
        models: rows,
    }
}

/// Throw this machine's tier decision away and probe the device again.
#[tauri::command]
pub fn rerun_probe(engine: State<'_, EngineClient>) {
    engine.reprobe();
}

/// Wait for the user to press the chord they want, and give it back as a string.
///
/// Ten seconds, then [`CommandError`] with the timeout key. The hook installed here never
/// blocks a key, so pressing something while this is waiting still reaches the application
/// underneath — including the chord that is currently registered.
///
/// # Errors
///
/// Nothing was pressed before the deadline; the chord is the permanently excluded one or has
/// no modifier; the operating system refused the hook.
#[tauri::command]
pub async fn capture_hotkey() -> Result<String, CommandError> {
    let captured =
        tauri::async_runtime::spawn_blocking(|| dile_hotkey::next_chord(CAPTURE_TIMEOUT))
            .await
            .map_err(|error| CommandError::detailed("settings.error.chord.unreadable", &error))?
            .map_err(|error| CommandError::detailed("settings.error.chord.unreadable", &error))?;

    match captured {
        dile_hotkey::Capture::TimedOut => Err(CommandError::plain("settings.error.chord.timeout")),
        dile_hotkey::Capture::Chord(chord) => {
            let written = chord.to_string();
            check_chord(&written)?;
            log::info!("the settings window captured the chord {written}");
            Ok(written)
        }
    }
}

/// Open the folder the model weights live in.
///
/// The one thing the settings window does that leaves the application, and it is a folder
/// rather than a file: a person looking here is either checking that a gigabyte arrived or
/// deleting it to start the download again.
///
/// # Errors
///
/// The platform would not name the directory, or the file manager would not start.
#[tauri::command]
pub fn open_models_dir(app: AppHandle) -> Result<(), CommandError> {
    let directory = models::directory(&app)
        .map_err(|error| CommandError::detailed("settings.error.models", &error))?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| CommandError::detailed("settings.error.models", &error))?;

    // The shell, by hand, rather than a plugin: one `Command` against a directory this
    // application owns is a smaller surface than a permission a webview could point
    // anywhere. `explorer` answers with a non-zero exit code even when it worked, so the
    // status is deliberately not read.
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(&directory)
        .spawn()
        .map_err(|error| CommandError::detailed("settings.error.models", &error))?;

    log::info!("opened the model directory {}", directory.display());
    Ok(())
}

/// Hide the settings window. What Esc does.
#[tauri::command]
pub fn close_settings(app: AppHandle) {
    window::hide(&app);
}

/// The chord rules that are product decisions rather than parsing.
fn check_chord(chord: &str) -> Result<(), CommandError> {
    let parsed: dile_hotkey::Chord = chord
        .parse()
        .map_err(|error| CommandError::detailed("settings.error.chord.unreadable", &error))?;

    // `docs/PROJECT.md` §3: excluded permanently. The IME swallows it before a low-level
    // hook ever sees it, so a person who set it would have a product that never hears them.
    if parsed
        == super::EXCLUDED_CHORD
            .parse()
            .unwrap_or_else(|_| dile_hotkey::Chord::ctrl_alt_space())
    {
        return Err(CommandError::plain("settings.error.chord.excluded"));
    }

    // A bare key as a global hotkey takes that key away from every application on the
    // machine, for as long as Dile is running.
    if !parsed.has_modifier() {
        return Err(CommandError::plain("settings.error.chord.nomodifier"));
    }

    Ok(())
}

/// Two entries may not claim the same term.
///
/// Compared the way the dictionary itself matches — case-insensitively under Turkish casing —
/// so `Cron` and `cron` are the same claim and the second one is refused where it was typed
/// rather than silently losing to the first at dictation time.
fn check_dictionary(settings: &Settings) -> Result<(), CommandError> {
    let mut seen = std::collections::BTreeSet::new();
    for entry in &settings.dictionary {
        let canonical = entry.canonical.trim();
        if canonical.is_empty() {
            return Err(CommandError::plain("settings.error.dictionary.empty"));
        }
        if !seen.insert(normalizer::to_lower(canonical)) {
            return Err(CommandError::plain("settings.error.dictionary.duplicate"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_chord, check_dictionary};
    use crate::settings::{DictionaryEntry, Settings};

    #[test]
    fn the_excluded_chord_is_refused_however_it_is_spelled() {
        for spelling in ["Ctrl+Space", "ctrl + space", "Control+Space"] {
            let refused = check_chord(spelling).expect_err(spelling);
            assert_eq!(refused.key, "settings.error.chord.excluded");
        }
        // And the default is not.
        assert!(check_chord("Ctrl+Alt+Space").is_ok());
    }

    #[test]
    fn a_chord_with_no_modifier_is_refused_and_says_which_rule_it_broke() {
        let refused = check_chord("F9").expect_err("a bare key is not a global hotkey");
        assert_eq!(refused.key, "settings.error.chord.nomodifier");

        let unreadable = check_chord("Ctrl+Enter").expect_err("Enter is outside the vocabulary");
        assert_eq!(unreadable.key, "settings.error.chord.unreadable");
        assert!(unreadable.detail.is_some(), "a developer needs the reason");
    }

    #[test]
    fn the_same_term_twice_is_refused_where_it_was_typed() {
        let mut settings = Settings {
            dictionary: vec![
                DictionaryEntry {
                    canonical: "cron".to_owned(),
                    ..DictionaryEntry::default()
                },
                DictionaryEntry {
                    // Turkish casing: the dictionary matches these as one term, so the
                    // settings must refuse them as one claim.
                    canonical: "CRON".to_owned(),
                    ..DictionaryEntry::default()
                },
            ],
            ..Settings::default()
        };
        let refused = check_dictionary(&settings).expect_err("two claims on one term");
        assert_eq!(refused.key, "settings.error.dictionary.duplicate");

        settings.dictionary[1].canonical = "keşfet".to_owned();
        assert!(check_dictionary(&settings).is_ok());

        settings.dictionary[1].canonical = "  ".to_owned();
        assert_eq!(
            check_dictionary(&settings).expect_err("an empty term").key,
            "settings.error.dictionary.empty"
        );
    }
}
