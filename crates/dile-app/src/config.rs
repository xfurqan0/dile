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
use dile_core::cleanup::Strictness;
use dile_core::dictionary::Dictionary;
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

/// The language Dile dictates in.
///
/// `docs/PROJECT.md` §3 gives the engine a language hint on every request rather than
/// letting it autodetect: "Dile knows which language it is set to, and a hint costs
/// nothing." Turkish is the product's first language and the one every M0 number was
/// measured in. **Settings, from WP5.**
pub const DEFAULT_LANGUAGE: &str = "tr";

/// Everything about turning audio into text.
///
/// Split out from the two WP2 halves because it belongs to a different crate again and
/// because three of its four fields are the ones WP5's settings window will spend most of
/// its space on: the language, the strictness chip, and the dictionary.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// The ISO code sent with every transcription request.
    pub language: String,
    /// Which cleanup rules run on what comes back. Medium is the default of
    /// `docs/PROJECT.md` §5 and the level the panel's chip starts on.
    pub strictness: Strictness,
    /// CPU threads the engine may use; 0 leaves it to the runtime.
    ///
    /// Zero is right on the Vulkan tier, where almost nothing runs on the CPU, and it is
    /// right on the fallback tier too until somebody has measured otherwise on a real
    /// machine. It is here rather than hard-coded because the engine host takes it off the
    /// wire, and a number that travels should have one place it comes from.
    pub threads: u32,
    /// The user's terms, which become the engine's initial prompt.
    ///
    /// **Empty in every build today**, and `docs/PROJECT.md` WP4 says why: every entry is a
    /// claim about one person's vocabulary. WP5 gives it somewhere to be stored, and the
    /// 0.379 → 0.261 word-error improvement M0 measured is what it unlocks.
    pub dictionary: Dictionary,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            language: DEFAULT_LANGUAGE.to_string(),
            strictness: Strictness::Medium,
            threads: 0,
            dictionary: Dictionary::new(),
        }
    }
}

/// The whole of Dile's configuration, as WP3 leaves it.
#[derive(Clone, Debug)]
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
    /// The engine: Turkish, medium strictness, the runtime's own threading, no dictionary.
    ///
    /// **Settings, from WP5:** all four, plus the tier — which lives in `engine.json` for
    /// now because it is decided by a probe rather than typed by a person.
    pub engine: EngineConfig,
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
            engine: EngineConfig::default(),
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
    use super::{
        AppConfig, DEFAULT_CAP_SECS, DEFAULT_LANGUAGE, DEFAULT_LEVEL_INTERVAL_MS,
        DEFAULT_PRE_ROLL_MS,
    };
    use dile_core::cleanup::Strictness;
    use dile_hotkey::{Chord, Mode};

    #[test]
    fn the_engine_ships_turkish_at_medium_with_an_empty_dictionary() {
        let engine = AppConfig::default().engine;

        assert_eq!(engine.language, DEFAULT_LANGUAGE);
        assert_eq!(engine.language, "tr");
        assert_eq!(engine.strictness, Strictness::Medium);
        assert_eq!(
            engine.threads, 0,
            "the runtime decides until somebody measures"
        );

        // "The shipped dictionary is empty" (docs/PROJECT.md WP4): every entry is a claim
        // about one person's vocabulary, so there is no prompt until a person writes one.
        assert!(engine.dictionary.is_empty());
        assert_eq!(engine.dictionary.prompt(), None);
    }

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
