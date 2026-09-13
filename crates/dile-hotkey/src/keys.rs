//! The key vocabulary the state machine speaks.
//!
//! Deliberately small. The machine only ever needs to name the keys a trigger is built
//! from — every other key on the keyboard reaches it as [`Event::OtherKeyDown`], with no
//! identity attached. Keeping the vocabulary this narrow is what lets `state.rs` stay free
//! of virtual-key codes, scan codes and platform enums.
//!
//! [`Event::OtherKeyDown`]: crate::Event::OtherKeyDown

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

#[cfg(test)]
mod tests {
    use super::{Chord, MainKey, ModifierFamily, ModifierKey, ModifierOnly};

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
    fn the_second_key_keeps_its_side() {
        assert_eq!(ModifierOnly::RIGHT_CTRL.key(), ModifierKey::CtrlRight);
        assert_ne!(
            ModifierOnly::RIGHT_CTRL,
            ModifierOnly::new(ModifierKey::CtrlLeft)
        );
    }
}
