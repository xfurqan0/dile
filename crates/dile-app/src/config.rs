//! Everything the user will eventually get to change, in one place and with no file behind
//! it yet.
//!
//! **Nothing here is persisted.** WP5 owns the settings window and the settings file, and
//! this struct is the shape it will read into: one value per decision `docs/PROJECT.md`
//! calls a setting, each with the reasoning that picked the default next to it. Holding them
//! together now — rather than leaving three crates' `Default`s scattered through `session.rs`
//! — is what makes that package a file reader and a form, instead of a hunt for constants.
//!
//! The two halves come from the crates that own them: [`HotkeyConfig`] from `dile-hotkey`
//! and [`CaptureConfig`] from `dile-capture`. This module re-states only what WP2 decided to
//! ship as the default, and the test at the bottom is what stops a crate's default from
//! drifting away from the decision without anybody noticing.

use dile_capture::CaptureConfig;
use dile_hotkey::{HotkeyConfig, Mode};

/// The recording cap WP2 ships with, in seconds.
///
/// `docs/PROJECT.md` §7: "A setting: default 60 s, maximum 300 s." The maximum belongs to
/// `dile-capture`, which clamps to it; this is the number the user starts with.
pub const DEFAULT_CAP_SECS: u32 = 60;

/// How much audio from before the key went down is kept, in milliseconds.
///
/// First-word protection. A push-to-talk key reaches the application after the first syllable
/// has begun — key travel, the low-level hook and the event loop are each a few milliseconds
/// — and half a second covers the hand as well as the wire.
pub const DEFAULT_PRE_ROLL_MS: u32 = 500;

/// How often the level meter produces a reading, in milliseconds.
///
/// WP2's acceptance criterion is "level events arrive at ≥ 20 Hz", and 50 ms is exactly that.
pub const DEFAULT_LEVEL_INTERVAL_MS: u32 = 50;

/// The whole of Dile's configuration, as WP2 leaves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    /// The trigger: `Ctrl+Alt+Space`, hold to talk, no second key.
    ///
    /// **Settings, from WP5:** hold or toggle, the chord itself, and whether the optional
    /// second key (`Right Ctrl`, modifier-only) is on. It ships off because switching it on
    /// claims that key — see `dile_hotkey::HotkeyConfig::second_key`.
    pub hotkey: HotkeyConfig,
    /// The microphone: the host default, half a second of pre-roll, a 60 s cap, 20 Hz meter.
    ///
    /// **Settings, from WP5:** the input device, the cap, and — further out — the pre-roll.
    /// The meter interval is not a setting: it is the panel's frame rate, and a user has no
    /// reason to have an opinion about it.
    pub capture: CaptureConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            hotkey: HotkeyConfig {
                mode: Mode::Hold,
                second_key: None,
                ..HotkeyConfig::default()
            },
            capture: CaptureConfig {
                device: None,
                pre_roll_ms: DEFAULT_PRE_ROLL_MS,
                cap_secs: DEFAULT_CAP_SECS,
                level_interval_ms: DEFAULT_LEVEL_INTERVAL_MS,
            },
        }
    }
}

impl AppConfig {
    /// The recording cap in milliseconds, which is the unit the panel is told it in.
    #[must_use]
    pub const fn cap_ms(&self) -> u64 {
        self.capture.cap_secs as u64 * 1_000
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, DEFAULT_CAP_SECS, DEFAULT_LEVEL_INTERVAL_MS, DEFAULT_PRE_ROLL_MS};
    use dile_hotkey::{Chord, Mode};

    #[test]
    fn the_shipped_defaults_are_the_ones_wp2_decided() {
        let config = AppConfig::default();

        assert_eq!(config.hotkey.mode, Mode::Hold);
        assert_eq!(config.hotkey.primary, Chord::ctrl_alt_space());
        assert_eq!(config.hotkey.second_key, None);

        assert_eq!(config.capture.device, None);
        assert_eq!(config.capture.cap_secs, DEFAULT_CAP_SECS);
        assert_eq!(config.capture.pre_roll_ms, DEFAULT_PRE_ROLL_MS);
        assert_eq!(config.cap_ms(), 60_000);
    }

    #[test]
    fn the_meter_runs_at_the_rate_the_acceptance_criterion_asks_for() {
        // "Level events arrive at ≥ 20 Hz" (docs/PROJECT.md WP2). The shipped interval is
        // what decides that, and the capture crate must not clamp it on the way in.
        let shipped = AppConfig::default().capture.clamped();
        assert_eq!(shipped.level_interval_ms, DEFAULT_LEVEL_INTERVAL_MS);

        let hz = 1000 / shipped.level_interval_ms;
        assert!(hz >= 20, "{hz} Hz is under the acceptance criterion");
    }
}
