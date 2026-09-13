//! The decision that outlives the process: which tier this machine is on.
//!
//! The probe of `docs/PROJECT.md` §3 runs **once per machine** rather than once per
//! start-up — a Vulkan probe costs a cold model load, and paying that on every launch would
//! be a worse product than the one the probe exists to protect. So the answer is written
//! down, and this is the file it is written to.
//!
//! **It is read by two programs and written by one.** The tray runs the probe and writes
//! the record; `dile transcribe` reads it and runs on whatever the tray decided, because a
//! command line that probed on its own would either take a cold load on every invocation or
//! disagree with the application about the same machine.
//!
//! **`engine.json` is a stop-gap with a successor already named.** WP5 owns the settings
//! file, and the tier is one of the things it puts on a form ("the tier stays visible and
//! switchable in settings", §3). When that migration happens this record folds into it;
//! until then a machine that has decided needs somewhere to say so, and one small file with
//! one small shape is easier to migrate than a decision taken again every morning.
//! [`crate::settings::EngineSettings::tier_override`] is the settings half of it: what the
//! *user* said, against what the probe *found*.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use dile_engine_proto::Device;
use serde::{Deserialize, Serialize};

/// The file the tier decision is kept in, inside the application's config directory.
pub const FILE: &str = "engine.json";

/// What the first-run probe saw, in fields rather than in a sentence.
///
/// A sentence would be shorter to write and worse to have: this is read back by the next
/// start-up, quoted in bug reports, and shown on a settings page that has to render it in
/// the user's language. A record with `passed: false` and `load_ms: 118_000` says "the
/// driver took two minutes to load the model and then timed out" to anybody who looks; the
/// same thing in English prose says it to half of them.
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
    ///
    /// `expected_words` is passed in rather than read from a constant here: the clip and the
    /// words it is scored against belong to the application that owns the probe, and this
    /// crate holds the record rather than the measurement.
    #[must_use]
    pub fn refused(expected_words: usize, error: &impl std::fmt::Display) -> Self {
        ProbeResult {
            expected_words,
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
    /// crate for a field nothing reads back as a date. The settings page shows it and can
    /// format it there, where a locale is already in hand.
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

/// Read the tier record, or `None` when this machine has not decided yet.
///
/// A file that will not parse is treated as no decision rather than as an error: the worst
/// it costs is one probe, and refusing to start over a damaged 80-byte file would be the
/// wrong trade every time.
#[must_use]
pub fn read(path: &Path) -> Option<TierRecord> {
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
pub fn write(path: &Path, record: &TierRecord) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::{ProbeResult, TierRecord, read, write};
    use dile_engine_proto::Device;
    use std::fs;

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
                error: Some("only 1 of 7 words came back".to_string()),
            },
        );
        write(&path, &record).expect("write the record");
        assert_eq!(read(&path), Some(record));

        // Not a decision, and not a panic either: the probe simply runs again.
        fs::write(&path, "{ this is not json").expect("damage the file");
        assert_eq!(read(&path), None);

        // And a machine that has never decided has no file at all.
        fs::remove_file(&path).expect("remove");
        assert_eq!(read(&path), None);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_refused_probe_keeps_the_word_count_it_was_told_and_carries_the_reason() {
        let refused = ProbeResult::refused(7, &"this build has no GPU support");
        assert!(!refused.passed);
        assert_eq!(refused.words, 0);
        assert_eq!(refused.expected_words, 7);
        assert_eq!(
            refused.error.as_deref(),
            Some("this build has no GPU support")
        );
    }
}
