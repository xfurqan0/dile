//! What the user gets to change about the trigger, and the numbers that come with it.

use crate::keys::{Chord, ModifierOnly};

/// How a press is interpreted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Hold to talk: the recording lives exactly as long as the press.
    ///
    /// The default of `docs/PROJECT.md` §3, and the one the product is named after — hold a
    /// key, speak, let go.
    #[default]
    Hold,
    /// Tap to start, tap to stop, with a long press still behaving as hold-to-talk.
    Toggle,
}

/// The default press-length threshold, in milliseconds.
///
/// Below it a press is an accident, not a dictation. §3: "an accidental tap never dictates".
pub const DEFAULT_PRESS_THRESHOLD_MS: u32 = 250;

/// The default point at which a press in [`Mode::Toggle`] stops being a tap, in milliseconds.
pub const DEFAULT_HOLD_TAKEOVER_MS: u32 = 800;

/// The safety ceiling on a single recording, in milliseconds.
///
/// This is the **maximum** of the recording cap of §7 (a setting, default 60 s, maximum
/// 300 s), not the setting itself. The cap the user chose is the capture crate's business:
/// it owns the buffer and stops cleanly at the limit. This number exists for one case only —
/// a key-up that never arrived — so it sits at the ceiling rather than at the default.
pub const DEFAULT_MAX_HOLD_MS: u32 = 300_000;

/// Everything the machine needs to know about the user's trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HotkeyConfig {
    /// Hold to talk, or tap to toggle.
    pub mode: Mode,
    /// The main trigger. Defaults to `Ctrl+Alt+Space`.
    pub primary: Chord,
    /// The optional modifier-only second trigger, off by default.
    ///
    /// Switching it on claims that key: a chord that needs the same physical key can no
    /// longer be typed as dictation, because the machine reads the lone press first. That
    /// is why it ships off, and why the key §3 suggests is the right Ctrl — the side almost
    /// nothing else is bound to.
    pub second_key: Option<ModifierOnly>,
    /// Below this many milliseconds a press is a tap, and a tap does not dictate.
    ///
    /// **Consulted in [`Mode::Hold`] and for the second key only.** A toggle user taps fast
    /// on purpose, so applying it there would make the first tap do nothing — see
    /// [`Mode::Toggle`] and the module documentation of `crate::state`.
    pub press_threshold_ms: u32,
    /// In [`Mode::Toggle`], the point at which a press stops latching and becomes
    /// hold-to-talk instead. Ignored in [`Mode::Hold`].
    pub hold_takeover_ms: u32,
    /// The safety ceiling that ends a recording whose key-up never arrived.
    pub max_hold_ms: u32,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Hold,
            primary: Chord::ctrl_alt_space(),
            second_key: None,
            press_threshold_ms: DEFAULT_PRESS_THRESHOLD_MS,
            hold_takeover_ms: DEFAULT_HOLD_TAKEOVER_MS,
            max_hold_ms: DEFAULT_MAX_HOLD_MS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_HOLD_TAKEOVER_MS, DEFAULT_MAX_HOLD_MS, DEFAULT_PRESS_THRESHOLD_MS, HotkeyConfig,
        Mode,
    };
    use crate::keys::{Chord, MainKey, ModifierFamily};

    #[test]
    fn the_defaults_are_the_ones_the_spec_decided() {
        let config = HotkeyConfig::default();
        assert_eq!(config.mode, Mode::Hold);
        assert_eq!(config.primary, Chord::ctrl_alt_space());
        assert_eq!(config.primary.key(), MainKey::Space);
        assert!(config.primary.requires(ModifierFamily::Ctrl));
        assert!(config.primary.requires(ModifierFamily::Alt));
        assert_eq!(config.second_key, None);
        assert_eq!(config.press_threshold_ms, DEFAULT_PRESS_THRESHOLD_MS);
        assert_eq!(config.hold_takeover_ms, DEFAULT_HOLD_TAKEOVER_MS);
        assert_eq!(config.max_hold_ms, DEFAULT_MAX_HOLD_MS);
    }

    #[test]
    fn the_safety_ceiling_is_the_maximum_of_the_recording_cap() {
        assert_eq!(DEFAULT_MAX_HOLD_MS, 300 * 1000);
    }
}
