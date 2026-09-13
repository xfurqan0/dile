//! The key vocabulary the state machine speaks.
//!
//! Deliberately small. The machine only ever needs to name the keys a trigger is built
//! from — every other key on the keyboard reaches it as [`Event::OtherKeyDown`], with no
//! identity attached. Keeping the vocabulary this narrow is what lets `state.rs` stay free
//! of virtual-key codes, scan codes and platform enums.
//!
//! [`Event::OtherKeyDown`]: crate::Event::OtherKeyDown

use std::fmt;
use std::str::FromStr;

/// A modifier key family, without a side.
///
/// A chord is written in families rather than in physical keys because a user pressing the
/// right Ctrl means the same thing as one pressing the left Ctrl. The second key of
/// `docs/PROJECT.md` §3 is the deliberate exception: it names a side, and [`ModifierKey`]
/// is how it does so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModifierFamily {
    /// Either Ctrl key.
    Ctrl,
    /// Either Alt key. On Windows this is `VK_MENU`; AltGr is the right Alt.
    Alt,
    /// Either Shift key.
    Shift,
    /// Either Windows / Command key.
    Meta,
}

impl ModifierFamily {
    /// Every family, in a fixed order, so that loops over them are exhaustive by
    /// construction rather than by a match the next variant would quietly fall out of.
    pub const ALL: [Self; 4] = [Self::Ctrl, Self::Alt, Self::Shift, Self::Meta];

    /// The bits in a held-modifier mask that belong to this family — both sides of it.
    pub(crate) const fn bits(self) -> u8 {
        match self {
            Self::Ctrl => ModifierKey::CtrlLeft.bit() | ModifierKey::CtrlRight.bit(),
            Self::Alt => ModifierKey::AltLeft.bit() | ModifierKey::AltRight.bit(),
            Self::Shift => ModifierKey::ShiftLeft.bit() | ModifierKey::ShiftRight.bit(),
            Self::Meta => ModifierKey::MetaLeft.bit() | ModifierKey::MetaRight.bit(),
        }
    }
}

/// One physical modifier key, side included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModifierKey {
    /// Left Ctrl.
    CtrlLeft,
    /// Right Ctrl — the key `docs/PROJECT.md` §3 offers as the optional second trigger.
    CtrlRight,
    /// Left Alt.
    AltLeft,
    /// Right Alt, which on a Turkish layout is also AltGr.
    AltRight,
    /// Left Shift.
    ShiftLeft,
    /// Right Shift.
    ShiftRight,
    /// Left Windows / Command.
    MetaLeft,
    /// Right Windows / Command.
    MetaRight,
}

impl ModifierKey {
    /// The family this key belongs to.
    #[must_use]
    pub const fn family(self) -> ModifierFamily {
        match self {
            Self::CtrlLeft | Self::CtrlRight => ModifierFamily::Ctrl,
            Self::AltLeft | Self::AltRight => ModifierFamily::Alt,
            Self::ShiftLeft | Self::ShiftRight => ModifierFamily::Shift,
            Self::MetaLeft | Self::MetaRight => ModifierFamily::Meta,
        }
    }

    /// This key's bit in a held-modifier mask.
    pub(crate) const fn bit(self) -> u8 {
        match self {
            Self::CtrlLeft => 1 << 0,
            Self::CtrlRight => 1 << 1,
            Self::AltLeft => 1 << 2,
            Self::AltRight => 1 << 3,
            Self::ShiftLeft => 1 << 4,
            Self::ShiftRight => 1 << 5,
            Self::MetaLeft => 1 << 6,
            Self::MetaRight => 1 << 7,
        }
    }
}

/// The non-modifier key that completes a chord.
///
/// Only the shapes a hotkey is plausibly built from. Anything outside this set reaches the
/// machine as [`Event::OtherKeyDown`] and needs no name.
///
/// [`Event::OtherKeyDown`]: crate::Event::OtherKeyDown
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MainKey {
    /// The space bar — the key of the default chord.
    Space,
    /// A letter key, identified by its **lower-case** ASCII character.
    ///
    /// Build it with [`MainKey::letter`], which normalises the case. A value constructed by
    /// hand with an upper-case character compares unequal to the same key from the OS
    /// adapter; [`MainKey::normalized`] repairs one.
    Letter(char),
    /// A number-row digit, `0` to `9`.
    Digit(u8),
    /// A function key, `1` to `24`.
    Function(u8),
}

impl MainKey {
    /// A letter key from an ASCII letter, in either case. `None` for anything else.
    #[must_use]
    pub fn letter(c: char) -> Option<Self> {
        c.is_ascii_alphabetic()
            .then(|| Self::Letter(c.to_ascii_lowercase()))
    }

    /// A number-row digit key. `None` above 9.
    #[must_use]
    pub fn digit(d: u8) -> Option<Self> {
        (d <= 9).then_some(Self::Digit(d))
    }

    /// A function key, F1 to F24. `None` outside that range.
    #[must_use]
    pub fn function(n: u8) -> Option<Self> {
        (1..=24).contains(&n).then_some(Self::Function(n))
    }

    /// The canonical form of this key: letters lower-cased, everything else unchanged.
    #[must_use]
    pub fn normalized(self) -> Self {
        match self {
            Self::Letter(c) => Self::Letter(c.to_ascii_lowercase()),
            other => other,
        }
    }
}

/// A key the machine can name, because it belongs to one of the configured triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A modifier, side included.
    Modifier(ModifierKey),
    /// The key that completes a chord.
    Main(MainKey),
}

/// A set of modifier families, held as a bitmask so a chord is `Copy` and comparison is a
/// single `&`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct FamilySet(u8);

impl FamilySet {
    const fn index(family: ModifierFamily) -> u8 {
        match family {
            ModifierFamily::Ctrl => 1 << 0,
            ModifierFamily::Alt => 1 << 1,
            ModifierFamily::Shift => 1 << 2,
            ModifierFamily::Meta => 1 << 3,
        }
    }

    fn insert(&mut self, family: ModifierFamily) {
        self.0 |= Self::index(family);
    }

    const fn contains(self, family: ModifierFamily) -> bool {
        self.0 & Self::index(family) != 0
    }
}

/// A chord: some modifier families plus one key, all held at once.
///
/// Order never matters and neither does the side of a modifier. `Ctrl+Alt+Space` is any
/// Ctrl, plus any Alt, plus the space bar, pressed in whatever sequence the user's fingers
/// happen to take — which is the whole reason the machine tracks a held set instead of
/// matching a sequence of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    modifiers: FamilySet,
    key: MainKey,
}

impl Chord {
    /// A chord from a list of families and a key. Repeats in the list are harmless.
    #[must_use]
    pub fn new(modifiers: &[ModifierFamily], key: MainKey) -> Self {
        let mut set = FamilySet::default();
        for &family in modifiers {
            set.insert(family);
        }
        Self {
            modifiers: set,
            key: key.normalized(),
        }
    }

    /// `Ctrl+Alt+Space`, the default of `docs/PROJECT.md` §7.
    ///
    /// It is not an IME toggle, not reserved by Windows, and no mainstream editor binds it.
    /// `Ctrl+Space` is excluded permanently: the IME swallows that chord before a low-level
    /// hook ever sees it.
    #[must_use]
    pub fn ctrl_alt_space() -> Self {
        Self::new(&[ModifierFamily::Ctrl, ModifierFamily::Alt], MainKey::Space)
    }

    /// Whether this chord needs a modifier of the given family held.
    #[must_use]
    pub const fn requires(&self, family: ModifierFamily) -> bool {
        self.modifiers.contains(family)
    }

    /// The key that completes the chord.
    #[must_use]
    pub const fn key(&self) -> MainKey {
        self.key
    }
}

impl Default for Chord {
    fn default() -> Self {
        Self::ctrl_alt_space()
    }
}

/// The optional second trigger: one modifier key, pressed on its own.
///
/// A side is part of its identity. `docs/PROJECT.md` §3 keeps it "still recommended for long
/// dictation, because holding a three-key chord for 60 s is tiring", and a lone modifier is
/// the only key a hand can rest on that long.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModifierOnly(ModifierKey);

impl ModifierOnly {
    /// Right Ctrl — the key §3 names.
    pub const RIGHT_CTRL: Self = Self(ModifierKey::CtrlRight);

    /// A second trigger on any modifier key.
    #[must_use]
    pub const fn new(key: ModifierKey) -> Self {
        Self(key)
    }

    /// The key itself.
    #[must_use]
    pub const fn key(self) -> ModifierKey {
        self.0
    }
}

/// Why a string is not a chord.
///
/// The one message per variant is a diagnostic, the way every other error in this crate is.
/// The sentence a user reads about a rejected chord is a locale key resolved in the
/// application: `docs/PROJECT.md` §3 keeps every visible word in `locales/`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChordParseError {
    /// There was nothing to parse.
    #[error("a chord cannot be empty")]
    Empty,
    /// A part of the chord is not a modifier this crate knows.
    #[error("{0:?} is not a modifier")]
    UnknownModifier(String),
    /// The last part is not a key a chord can be built from.
    #[error("{0:?} is not a key a chord can end with")]
    UnknownKey(String),
}

/// The name of one modifier family in a chord string, and the spellings accepted for it.
///
/// The written order is the order [`Chord`]'s `Display` produces, which is why it is a table
/// rather than a match: the same list decides how a chord is written and how it is read, so
/// the two cannot drift apart.
const MODIFIER_NAMES: [(ModifierFamily, &str, &[&str]); 4] = [
    (ModifierFamily::Ctrl, "Ctrl", &["ctrl", "control"]),
    (ModifierFamily::Alt, "Alt", &["alt", "opt", "option"]),
    (ModifierFamily::Shift, "Shift", &["shift"]),
    (
        ModifierFamily::Meta,
        "Meta",
        &["meta", "win", "windows", "cmd", "super"],
    ),
];

/// How the space bar is written in a chord string.
const SPACE_NAME: &str = "Space";

impl fmt::Display for MainKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MainKey::Space => f.write_str(SPACE_NAME),
            MainKey::Letter(letter) => write!(f, "{}", letter.to_ascii_uppercase()),
            MainKey::Digit(digit) => write!(f, "{digit}"),
            MainKey::Function(number) => write!(f, "F{number}"),
        }
    }
}

impl FromStr for MainKey {
    type Err = ChordParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let name = s.trim();
        let lowered = name.to_ascii_lowercase();
        if lowered == SPACE_NAME.to_ascii_lowercase() {
            return Ok(MainKey::Space);
        }
        if let Some(digits) = lowered.strip_prefix('f')
            && !digits.is_empty()
            && let Ok(number) = digits.parse::<u8>()
        {
            return MainKey::function(number)
                .ok_or_else(|| ChordParseError::UnknownKey(name.to_owned()));
        }
        let mut characters = lowered.chars();
        if let (Some(single), None) = (characters.next(), characters.next()) {
            if let Some(digit) = single.to_digit(10) {
                return u8::try_from(digit)
                    .ok()
                    .and_then(MainKey::digit)
                    .ok_or_else(|| ChordParseError::UnknownKey(name.to_owned()));
            }
            if let Some(letter) = MainKey::letter(single) {
                return Ok(letter);
            }
        }
        Err(ChordParseError::UnknownKey(name.to_owned()))
    }
}

/// How a chord is written down: `Ctrl+Alt+Space`, `Shift+F5`, `Ctrl+Meta+D`.
///
/// One spelling, in one order, whatever order the user pressed the keys in — a settings file
/// that stored `Alt+Ctrl+Space` one day and `Ctrl+Alt+Space` the next would make two
/// identical chords compare unequal as text and a "did this change?" question unanswerable.
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (family, written, _) in MODIFIER_NAMES {
            if self.requires(family) {
                write!(f, "{written}+")?;
            }
        }
        write!(f, "{}", self.key)
    }
}

/// The other direction: `"Ctrl+Alt+Space".parse()`.
///
/// Case-insensitive, whitespace-tolerant and forgiving about the names a person might type —
/// `win`, `windows`, `cmd` and `super` are all the Meta family — because this parses a
/// settings file a human may have edited. The **order never matters**: a chord is a set of
/// families plus a key, and `Display` is what decides how it is written back.
impl FromStr for Chord {
    type Err = ChordParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        let (key, modifiers) = parts.split_last().ok_or(ChordParseError::Empty)?;

        let mut families = Vec::with_capacity(modifiers.len());
        for part in modifiers {
            let lowered = part.to_ascii_lowercase();
            let found = MODIFIER_NAMES
                .iter()
                .find(|(_, _, spellings)| spellings.contains(&lowered.as_str()))
                .map(|&(family, _, _)| family)
                .ok_or_else(|| ChordParseError::UnknownModifier((*part).to_owned()))?;
            families.push(found);
        }

        Ok(Chord::new(&families, key.parse()?))
    }
}

/// Whether this chord holds no modifier at all.
///
/// A bare key as a global hotkey takes that key away from every application on the machine,
/// so the application refuses one. The rule lives there rather than here: this crate reports
/// what a chord *is*, and what is acceptable is a product decision.
impl Chord {
    /// Whether any modifier family is required.
    #[must_use]
    pub const fn has_modifier(&self) -> bool {
        self.modifiers.0 != 0
    }

    /// The modifier families this chord requires, in the order they are written.
    #[must_use]
    pub fn families(&self) -> Vec<ModifierFamily> {
        MODIFIER_NAMES
            .iter()
            .filter(|(family, _, _)| self.requires(*family))
            .map(|&(family, _, _)| family)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Chord, ChordParseError, MainKey, ModifierFamily, ModifierKey, ModifierOnly};

    #[test]
    fn every_modifier_key_owns_exactly_one_bit_of_its_family() {
        for family in ModifierFamily::ALL {
            let bits = family.bits();
            assert_eq!(bits.count_ones(), 2, "{family:?} should cover two sides");
        }
        let all: u8 = ModifierFamily::ALL
            .iter()
            .fold(0, |acc, family| acc | family.bits());
        assert_eq!(
            all,
            u8::MAX,
            "the four families should cover all eight keys"
        );
    }

    #[test]
    fn a_modifier_key_reports_its_own_family() {
        assert_eq!(ModifierKey::CtrlRight.family(), ModifierFamily::Ctrl);
        assert_eq!(ModifierKey::AltRight.family(), ModifierFamily::Alt);
        assert_eq!(ModifierKey::MetaLeft.family(), ModifierFamily::Meta);
    }

    #[test]
    fn letters_are_normalised_to_lower_case() {
        assert_eq!(MainKey::letter('D'), Some(MainKey::Letter('d')));
        assert_eq!(MainKey::letter('d'), Some(MainKey::Letter('d')));
        assert_eq!(MainKey::letter('ş'), None);
        assert_eq!(MainKey::Letter('D').normalized(), MainKey::Letter('d'));
    }

    #[test]
    fn digits_and_function_keys_reject_values_no_keyboard_has() {
        assert_eq!(MainKey::digit(9), Some(MainKey::Digit(9)));
        assert_eq!(MainKey::digit(10), None);
        assert_eq!(MainKey::function(24), Some(MainKey::Function(24)));
        assert_eq!(MainKey::function(0), None);
        assert_eq!(MainKey::function(25), None);
    }

    #[test]
    fn the_default_chord_is_ctrl_alt_space() {
        let chord = Chord::default();
        assert!(chord.requires(ModifierFamily::Ctrl));
        assert!(chord.requires(ModifierFamily::Alt));
        assert!(!chord.requires(ModifierFamily::Shift));
        assert!(!chord.requires(ModifierFamily::Meta));
        assert_eq!(chord.key(), MainKey::Space);
    }

    #[test]
    fn a_chord_normalises_the_case_of_its_key() {
        let chord = Chord::new(&[ModifierFamily::Ctrl], MainKey::Letter('D'));
        assert_eq!(chord.key(), MainKey::Letter('d'));
    }

    #[test]
    fn a_chord_is_written_in_one_order_whatever_order_it_was_typed_in() {
        assert_eq!(Chord::ctrl_alt_space().to_string(), "Ctrl+Alt+Space");
        assert_eq!(
            Chord::new(&[ModifierFamily::Alt, ModifierFamily::Ctrl], MainKey::Space).to_string(),
            "Ctrl+Alt+Space",
            "the families are written in a fixed order, not in the order they were given"
        );
        assert_eq!(
            Chord::new(&[ModifierFamily::Shift], MainKey::Function(5)).to_string(),
            "Shift+F5"
        );
        assert_eq!(
            Chord::new(&[ModifierFamily::Meta], MainKey::Letter('d')).to_string(),
            "Meta+D"
        );
        assert_eq!(
            Chord::new(&[ModifierFamily::Ctrl], MainKey::Digit(7)).to_string(),
            "Ctrl+7"
        );
    }

    #[test]
    fn every_chord_survives_being_written_down_and_read_back() {
        let chords = [
            Chord::ctrl_alt_space(),
            Chord::new(&[ModifierFamily::Ctrl], MainKey::Letter('d')),
            Chord::new(
                &[ModifierFamily::Shift, ModifierFamily::Meta],
                MainKey::Digit(0),
            ),
            Chord::new(&ModifierFamily::ALL, MainKey::Function(24)),
        ];
        for chord in chords {
            let written = chord.to_string();
            let read: Chord = written.parse().expect("a chord this crate wrote");
            assert_eq!(read, chord, "{written} did not survive the round trip");
        }
    }

    #[test]
    fn a_settings_file_a_person_edited_still_parses() {
        // Order, case and spacing are all forgiven: this string comes out of a JSON file
        // somebody may have typed into.
        let typed: Chord = " alt + CONTROL + space ".parse().expect("a human spelling");
        assert_eq!(typed, Chord::ctrl_alt_space());

        for spelling in ["Win+D", "Windows+D", "Cmd+D", "Super+D", "meta+d"] {
            assert_eq!(
                spelling.parse::<Chord>().expect(spelling),
                Chord::new(&[ModifierFamily::Meta], MainKey::Letter('d')),
                "{spelling} is the Meta family"
            );
        }
    }

    #[test]
    fn a_string_that_is_not_a_chord_says_which_part_was_wrong() {
        assert_eq!("".parse::<Chord>(), Err(ChordParseError::Empty));
        assert_eq!(
            "Hyper+D".parse::<Chord>(),
            Err(ChordParseError::UnknownModifier("Hyper".to_owned()))
        );
        assert_eq!(
            "Ctrl+Enter".parse::<Chord>(),
            Err(ChordParseError::UnknownKey("Enter".to_owned())),
            "the vocabulary is deliberately small; a key outside it is named rather than guessed"
        );
        assert_eq!(
            "Ctrl+F25".parse::<Chord>(),
            Err(ChordParseError::UnknownKey("F25".to_owned()))
        );
    }

    #[test]
    fn a_chord_knows_whether_it_has_a_modifier_at_all() {
        assert!(Chord::ctrl_alt_space().has_modifier());
        assert_eq!(
            Chord::ctrl_alt_space().families(),
            vec![ModifierFamily::Ctrl, ModifierFamily::Alt]
        );

        let bare: Chord = "F9".parse().expect("a bare function key parses");
        assert!(
            !bare.has_modifier(),
            "the application is what refuses this, and it needs to be able to see it"
        );
        assert!(bare.families().is_empty());
    }

    #[test]
    fn the_second_key_keeps_its_side() {
        assert_eq!(ModifierOnly::RIGHT_CTRL.key(), ModifierKey::CtrlRight);
        assert_ne!(
            ModifierOnly::RIGHT_CTRL,
            ModifierOnly::new(ModifierKey::CtrlLeft)
        );
    }
}
