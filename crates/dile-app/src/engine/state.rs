//! What the engine is doing, and the file that remembers which tier this machine is on.
//!
//! Two things live here because they are two halves of one answer. [`EngineState`] is the
//! moment-to-moment report the tray and the panel read; [`TierRecord`] is the decision that
//! outlives the process, so that the probe of `docs/PROJECT.md` §3 runs **once per machine**
//! rather than once per start-up — a Vulkan probe costs a cold model load, and paying that
//! on every launch would be a worse product than the one the probe exists to protect.
//!
//! **`engine.json` is a stop-gap with a successor already named.** WP5 owns the settings
//! file and the settings window, and the tier is one of the things it puts on a form ("the
//! tier stays visible and switchable in settings", §3). When that file exists this record
//! folds into it; until then a machine that has decided needs somewhere to say so, and one
//! small file with one small shape is easier to migrate than a decision taken again every
//! morning.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dile_engine_proto::Device;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// The file the tier decision is kept in, inside the application's config directory.
const TIER_FILE: &str = "engine.json";

/// What the engine is doing right now.
///
/// Every one of these reaches the panel as `dile://engine`. They are not tray states: the
/// tray has its own five-value vocabulary about the *session*, and mapping one onto the
/// other is [`crate::engine`]'s job rather than something a payload should decide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineState {
    /// No weights on disk, and the person said not now. The hotkey still records.
    NoModel,
    /// Fetching weights. The percentage is of the file, not of the session.
    Downloading {
        /// 0–100.
        percent: u8,
    },
    /// Running the first-run probe transcription of `docs/PROJECT.md` §3.
    Probing,
    /// A model is loaded and a dictation would be transcribed.
    Ready {
        /// Which tier it is loaded on.
        tier: Device,
    },
    /// A dictation is being transcribed.
    Busy,
    /// The engine process went down. A respawn is in progress.
    Crashed,
    /// The engine process went down too many times in a row and is not being respawned.
    Failed,
}

impl EngineState {
    /// The name this state travels under in the `dile://engine` event.
    ///
    /// Stable, because WP5's panel switches on these strings: they are an interface, not a
    /// debugging convenience.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            EngineState::NoModel => "no-model",
            EngineState::Downloading { .. } => "downloading",
            EngineState::Probing => "probing",
            EngineState::Ready { .. } => "ready",
            EngineState::Busy => "busy",
            EngineState::Crashed => "crashed",
            EngineState::Failed => "failed",
        }
    }

    /// The tier, when this state has one.
    #[must_use]
    pub const fn tier(self) -> Option<Device> {
        match self {
            EngineState::Ready { tier } => Some(tier),
            _ => None,
        }
    }

    /// The download percentage, when this state has one.
    #[must_use]
    pub const fn percent(self) -> Option<u8> {
        match self {
            EngineState::Downloading { percent } => Some(percent),
            _ => None,
        }
    }
}

/// The payload of `dile://engine`.
#[derive(Clone, Debug, Serialize)]
pub struct EnginePayload {
    /// One of [`EngineState::name`].
    pub state: &'static str,
    /// The tier, when the state has one.
    pub tier: Option<&'static str>,
    /// The model file this machine is set up to use, when one has been chosen.
    pub model: Option<String>,
    /// Download progress, 0–100, when something is being downloaded.
    pub progress: Option<u8>,
}

impl EnginePayload {
    /// The payload for a state, with the model the machine is set up to use beside it.
    #[must_use]
    pub fn new(state: EngineState, model: Option<&str>) -> Self {
        EnginePayload {
            state: state.name(),
            tier: state.tier().map(Device::as_str),
            model: model.map(str::to_string),
            progress: state.percent(),
        }
    }
}

/// What the first-run probe saw, in fields rather than in a sentence.
///
/// A sentence would be shorter to write and worse to have: this is read back by the next
/// start-up, quoted in bug reports, and — from WP5 — shown on a settings page that has to
/// render it in the user's language. A record with `passed: false` and `load_ms: 118_000`
/// says "the driver took two minutes to load the model and then timed out" to anybody who
/// looks; the same thing in English prose says it to half of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    /// Whether the device produced enough of the sentence to be trusted.
    pub passed: bool,
    /// The backend the runtime reported, which is not always the one that was asked for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device: String,
    /// How long the model took to load, in milliseconds. The cold Vulkan number is the
    /// interesting one: it includes the driver compiling its shader cache.
    pub load_ms: u64,
    /// How long the probe transcription took, in milliseconds.
    pub probe_ms: u64,
    /// How many of the expected words came back.
    pub words: usize,
    /// How many there were to find.
    pub expected_words: usize,
    /// The failure, as the error that caused it describes itself. Absent when it passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProbeResult {
    /// A probe that never ran, because something stopped it before the device was asked.
    #[must_use]
    pub fn refused(error: &impl std::fmt::Display) -> Self {
        ProbeResult {
            expected_words: crate::engine::probe::WORDS.len(),
            error: Some(error.to_string()),
            ..ProbeResult::default()
        }
    }
}

/// The tier this machine decided on, and what the probe saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierRecord {
    /// The tier to use from now on.
    pub tier: Device,
    /// When it was decided, in seconds since the Unix epoch.
    ///
    /// A number rather than a formatted date, because formatting one would mean a calendar
    /// crate for a field nothing reads back as a date. WP5 shows it on the settings page and
    /// can format it there, where a locale is already in hand.
    pub decided_at: u64,
    /// What the probe saw.
    pub probe_result: ProbeResult,
}

impl TierRecord {
    /// A record of a decision taken now.
    #[must_use]
    pub fn taken(tier: Device, probe_result: ProbeResult) -> Self {
        TierRecord {
            tier,
            decided_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_secs())
                .unwrap_or_default(),
            probe_result,
        }
    }
}

/// Where the tier record lives: `<app config>/engine.json`.
pub fn tier_path(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    Ok(app.path().app_config_dir()?.join(TIER_FILE))
}

/// Read the tier record, or `None` when this machine has not decided yet.
///
/// A file that will not parse is treated as no decision rather than as an error: the worst
/// it costs is one probe, and refusing to start over a damaged 80-byte file would be the
/// wrong trade every time.
#[must_use]
pub fn read_tier(path: &Path) -> Option<TierRecord> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(record) => Some(record),
        Err(error) => {
            log::warn!(
                "{} did not parse, so the tier is decided again: {error}",
                path.display()
            );
            None
        }
    }
}

/// Write the tier record, creating the directory if it is not there.
///
/// # Errors
///
/// The directory could not be created, the file could not be written, or the record would
/// not serialize.
pub fn write_tier(path: &Path, record: &TierRecord) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::{EnginePayload, EngineState, ProbeResult, TierRecord, read_tier, write_tier};
    use crate::engine::probe::ProbeError;
    use dile_engine_proto::Device;
    use std::fs;

    #[test]
    fn every_state_has_a_distinct_name_and_only_the_right_ones_carry_a_tier() {
        let states = [
            EngineState::NoModel,
            EngineState::Downloading { percent: 40 },
            EngineState::Probing,
            EngineState::Ready {
                tier: Device::Vulkan,
            },
            EngineState::Busy,
            EngineState::Crashed,
            EngineState::Failed,
        ];
        let mut names: Vec<&str> = states.iter().map(|state| state.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two engine states share a name");

        assert_eq!(
            EngineState::Ready { tier: Device::Cpu }.tier(),
            Some(Device::Cpu)
        );
        assert_eq!(EngineState::Busy.tier(), None);
        assert_eq!(EngineState::Downloading { percent: 40 }.percent(), Some(40));
        assert_eq!(EngineState::Probing.percent(), None);
    }

    #[test]
    fn the_payload_is_the_shape_wp5_will_read() {
        let ready = serde_json::to_value(EnginePayload::new(
            EngineState::Ready {
                tier: Device::Vulkan,
            },
            Some("ggml-large-v3-q5_0.bin"),
        ))
        .expect("serialize");
        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["tier"], "vulkan");
        assert_eq!(ready["model"], "ggml-large-v3-q5_0.bin");
        assert!(ready["progress"].is_null());

        let downloading = serde_json::to_value(EnginePayload::new(
            EngineState::Downloading { percent: 7 },
            None,
        ))
        .expect("serialize");
        assert_eq!(downloading["state"], "downloading");
        assert_eq!(downloading["progress"], 7);
        assert!(downloading["tier"].is_null());
    }

    #[test]
    fn a_tier_record_survives_the_round_trip_and_a_damaged_file_is_no_decision() {
        let directory = std::env::temp_dir().join(format!(
            "dile-tier-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = directory.join("engine.json");

        let record = TierRecord::taken(
            Device::Cpu,
            ProbeResult {
                passed: false,
                device: "cpu".to_string(),
                load_ms: 9_120,
                probe_ms: 4_400,
                words: 1,
                expected_words: 7,
                error: Some(ProbeError::WrongWords {
                    matched: 1,
                    expected: 7,
                })
                .map(|error| error.to_string()),
            },
        );
        write_tier(&path, &record).expect("write the record");
        assert_eq!(read_tier(&path), Some(record));

        // Not a decision, and not a panic either: the probe simply runs again.
        fs::write(&path, "{ this is not json").expect("damage the file");
        assert_eq!(read_tier(&path), None);

        // And a machine that has never decided has no file at all.
        fs::remove_file(&path).expect("remove");
        assert_eq!(read_tier(&path), None);
        let _ = fs::remove_dir_all(&directory);
    }
}
