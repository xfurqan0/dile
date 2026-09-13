//! What the engine is doing, and where the tier decision is kept.
//!
//! [`EngineState`] is the moment-to-moment report the tray and the panel read: not a tier,
//! not a file, just what the supervisor is doing at this instant. It is the tray's alone,
//! which is why it stayed here when the rest moved.
//!
//! The decision that **outlives the process** did move: [`TierRecord`] and the two functions
//! that read and write it live in [`dile_client::tier`], because `dile transcribe` runs on
//! the tier this machine decided rather than probing again on its own. They are re-exported
//! here so that the supervisor still says `state::read_tier`, and [`tier_path`] is the one
//! line that needs an `AppHandle`.

use std::path::PathBuf;

use dile_engine_proto::Device;
use serde::Serialize;
use tauri::{AppHandle, Manager};

pub use dile_client::tier::{
    self, ProbeResult, TierRecord, read as read_tier, write as write_tier,
};

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

/// Where the tier record lives: `<app config>/engine.json`.
///
/// The same directory as `settings.json` — see [`crate::settings::path`], which names the
/// other file in it.
///
/// # Errors
///
/// The platform would not name a configuration directory.
pub fn tier_path(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    Ok(app.path().app_config_dir()?.join(tier::FILE))
}

#[cfg(test)]
mod tests {
    use super::{EnginePayload, EngineState};
    use dile_engine_proto::Device;

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
}
