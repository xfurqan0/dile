//! "Press the new trigger": one press, read once, and the hook comes straight back down.
//!
//! The settings window of `docs/PROJECT.md` WP5 lets a user change the trigger, and the only
//! honest way to ask for one is to let them press it. Typing one into a text field asks a
//! person to know that their keyboard calls the key `OEM_3`; pressing it asks them to press
//! it.
//!
//! **This is not a second listener.** [`next_trigger`] installs its own observe-only hook,
//! waits for one press it can name, and drops the hook before it returns. It never blocks a
//! key — a capture that swallowed the trigger would swallow it for the whole machine while a
//! settings window sat open — and the application's real listener keeps running underneath
//! it. The most recently installed low-level hook is called first, so a chord the running
//! listener blocks is still seen here before that listener ever gets it.
//!
//! ## Two shapes, and the rule that tells them apart
//!
//! A [`Trigger`] is either a chord or one modifier key held on its own, so this capture has to
//! answer a question the chord-only version never asked: is the Ctrl that just went down the
//! beginning of `Ctrl+Alt+Space`, or the whole trigger?
//!
//! * **A chord ends the capture on the key that completes it.** The first key-down whose key
//!   one [`MainKey`] can name is the answer, with whatever modifiers were held at that moment.
//! * **A lone modifier is decided on the release**, by one rule: *the modifier that went down
//!   is the next thing that goes up, with nothing else pressed in between*. So `Ctrl` down and
//!   `Ctrl` up is the right Ctrl; `Ctrl` down, `Alt` down, `Ctrl` up is nothing at all, and the
//!   capture keeps waiting — a release inside a chord in progress is not a lone key, and
//!   guessing that it was would hand a person `LeftCtrl` when they were reaching for a chord.
//!
//! Nothing outside those two is a trigger. A key outside this crate's small vocabulary —
//! Enter, a media key, a mouse button — is not an answer, but it *is* something pressed in
//! between, so it ends any lone-key candidacy and the capture waits for the next press. A
//! timeout is [`Capture::TimedOut`] rather than an error, because nobody pressing nothing has
//! done anything wrong.
//!
//! **The decision is a pure function.** [`InProgress::on`] takes one [`KeyStep`] and answers,
//! which is what lets every rule above be a unit test rather than a hand check with a
//! keyboard. The hook's only job is to turn a `handy-keys` event into that struct.

use std::time::{Duration, Instant};

use handy_keys::{KeyEvent as OsKeyEvent, KeyboardListener, Modifiers};

use crate::error::Error;
use crate::keys::{Chord, MainKey, ModifierFamily, ModifierKey, ModifierOnly, Trigger};
use crate::listener::map_modifier;

/// What one capture produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// The user pressed this.
    Trigger(Trigger),
    /// Nothing was pressed before the deadline.
    TimedOut,
}

/// The non-modifier key one event carries.
///
/// Three cases rather than an `Option`, because "no key at all" and "a key with no name here"
/// mean opposite things to a lone-key candidate: one is a modifier-state echo that changed
/// nothing, the other is somebody pressing Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pressed {
    /// No key: a modifier event, or one of the state echoes the hook sends.
    Nothing,
    /// A key outside this crate's vocabulary — Enter, a media key, a mouse button.
    Unnamed,
    /// A key a chord can end with.
    Named(MainKey),
}

/// One key event, reduced to what the decision depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyStep {
    /// Whether the key went down. A release is the only thing that can name a lone modifier.
    is_down: bool,
    /// The physical modifier whose state this event changed, when it changed a named one.
    changed_modifier: Option<ModifierKey>,
    /// The non-modifier key this event carries.
    key: Pressed,
    /// Every modifier held at the moment of the event, which is what a chord is built from.
    modifiers: Modifiers,
}

/// A capture that has not decided yet.
///
/// One value, because there is only one thing worth remembering between events: the modifier
/// that might still turn out to have been pressed on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct InProgress {
    /// The modifier that went down with nothing else pressed since, if there is one.
    alone: Option<ModifierKey>,
}

impl InProgress {
    /// Feed one event. `Some` is the trigger the user pressed; `None` means keep waiting.
    fn on(&mut self, event: KeyStep) -> Option<Trigger> {
        if !event.is_down {
            // The only question a release answers. `take` is deliberate: a release that was
            // not the candidate's ends the candidacy, because the modifier that is still held
            // was pressed before something else and is no longer alone.
            let alone = self.alone.take()?;
            return (event.changed_modifier == Some(alone))
                .then_some(Trigger::Key(ModifierOnly::new(alone)));
        }

        match (event.changed_modifier, event.key) {
            // The key that completes a chord, with the modifiers held at that instant. It
            // ends the capture whatever was being tracked: a chord is an answer on its own.
            (_, Pressed::Named(main)) => {
                return Some(Trigger::Chord(Chord::new(&families(event.modifiers), main)));
            }
            (Some(modifier), _) => {
                self.alone = match self.alone {
                    // Auto-repeat of the candidate: the same press, not a second key.
                    Some(held) if held == modifier => Some(held),
                    // A second modifier joined the first, so neither is alone any more.
                    Some(_) => None,
                    None => Some(modifier),
                };
            }
            // A key this crate cannot name. Not a chord it can report, but certainly
            // something pressed in between.
            (None, Pressed::Unnamed) => self.alone = None,
            // A modifier-state echo that changed nothing named. Ignored entirely, so that a
            // lone press survives one.
            (None, Pressed::Nothing) => {}
        }
        None
    }
}

/// One `handy-keys` event in the terms [`InProgress::on`] decides with.
fn step_of(event: &OsKeyEvent) -> KeyStep {
    KeyStep {
        is_down: event.is_key_down,
        changed_modifier: event.changed_modifier.and_then(map_modifier),
        key: match event.key {
            // The same parse the chord path has always used: `handy-keys`' own spelling of a
            // key is its advertised API, so one table decides what this crate can name.
            Some(key) => key
                .to_string()
                .parse::<MainKey>()
                .map_or(Pressed::Unnamed, Pressed::Named),
            None => Pressed::Nothing,
        },
        modifiers: event.modifiers,
    }
}

/// The modifier families held, as this crate names them.
///
/// `handy-keys` reports the sided flags; a chord is written in families, so both sides of
/// each are folded into one. The same rule the state machine and the blocking set follow — and
/// the one place a lone modifier differs, since its side is its identity.
fn families(held: Modifiers) -> Vec<ModifierFamily> {
    let mut found = Vec::new();
    for family in ModifierFamily::ALL {
        let sides = match family {
            ModifierFamily::Ctrl => Modifiers::CTRL,
            ModifierFamily::Alt => Modifiers::OPT,
            ModifierFamily::Shift => Modifiers::SHIFT,
            ModifierFamily::Meta => Modifiers::CMD,
        };
        if held.intersects(sides) {
            found.push(family);
        }
    }
    found
}

/// Wait for the user to press one trigger.
///
/// Returns as soon as a press names one — a chord on the key that completes it, a lone
/// modifier on its release — or [`Capture::TimedOut`] when `timeout` passes with nothing
/// pressed. The hook is removed either way.
///
/// # Errors
///
/// [`Error::Hook`] if the operating system refused the hook.
pub fn next_trigger(timeout: Duration) -> Result<Capture, Error> {
    let keyboard = KeyboardListener::new().map_err(|error| Error::Hook(error.to_string()))?;
    let deadline = Instant::now() + timeout;
    let mut capture = InProgress::default();

    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Capture::TimedOut);
        }

        match keyboard.recv_timeout(left) {
            Ok(event) => {
                if let Some(trigger) = capture.on(step_of(&event)) {
                    return Ok(Capture::Trigger(trigger));
                }
            }
            Err(handy_keys::Error::Timeout) => return Ok(Capture::TimedOut),
            Err(error) => return Err(Error::Hook(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InProgress, KeyStep, Pressed, families};
    use crate::keys::{Chord, MainKey, ModifierFamily, ModifierKey, ModifierOnly, Trigger};
    use handy_keys::Modifiers;

    /// A modifier going down or coming up, with what is held at that moment.
    fn modifier(key: ModifierKey, is_down: bool, held: Modifiers) -> KeyStep {
        KeyStep {
            is_down,
            changed_modifier: Some(key),
            key: Pressed::Nothing,
            modifiers: held,
        }
    }

    /// A non-modifier key going down, with the modifiers held at that moment.
    fn main_key(key: MainKey, held: Modifiers) -> KeyStep {
        KeyStep {
            is_down: true,
            changed_modifier: None,
            key: Pressed::Named(key),
            modifiers: held,
        }
    }

    #[test]
    fn both_sides_of_a_modifier_fold_into_one_family() {
        assert_eq!(
            families(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT),
            vec![ModifierFamily::Ctrl, ModifierFamily::Alt]
        );
        assert_eq!(
            families(Modifiers::CTRL_RIGHT),
            families(Modifiers::CTRL_LEFT),
            "a chord is written in families, so the side cannot change the answer"
        );
        assert!(families(Modifiers::empty()).is_empty());
    }

    #[test]
    fn the_families_come_back_in_the_order_a_chord_is_written_in() {
        let all = families(
            Modifiers::CMD_LEFT
                | Modifiers::SHIFT_LEFT
                | Modifiers::OPT_LEFT
                | Modifiers::CTRL_LEFT,
        );
        assert_eq!(
            all,
            vec![
                ModifierFamily::Ctrl,
                ModifierFamily::Alt,
                ModifierFamily::Shift,
                ModifierFamily::Meta
            ]
        );
    }

    #[test]
    fn a_modifier_pressed_and_released_on_its_own_is_a_lone_key() {
        let mut capture = InProgress::default();
        assert_eq!(
            capture.on(modifier(
                ModifierKey::CtrlRight,
                true,
                Modifiers::CTRL_RIGHT
            )),
            None,
            "nothing is decided while the key is still down"
        );
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlRight, false, Modifiers::empty())),
            Some(Trigger::Key(ModifierOnly::RIGHT_CTRL)),
            "the shipped trigger, captured the way a person would press it"
        );
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlRight, false, Modifiers::empty())),
            None,
            "and the candidate is spent, not answered twice"
        );
    }

    #[test]
    fn a_lone_key_keeps_the_side_it_was_pressed_on() {
        for (key, expected) in [
            (ModifierKey::CtrlLeft, "LeftCtrl"),
            (ModifierKey::AltRight, "RightAlt"),
            (ModifierKey::ShiftLeft, "LeftShift"),
            (ModifierKey::MetaRight, "RightMeta"),
        ] {
            let mut capture = InProgress::default();
            assert_eq!(capture.on(modifier(key, true, Modifiers::empty())), None);
            let trigger = capture
                .on(modifier(key, false, Modifiers::empty()))
                .expect("a lone press");
            assert_eq!(trigger.to_string(), expected);
        }
    }

    #[test]
    fn the_key_that_completes_a_chord_ends_the_capture() {
        let mut capture = InProgress::default();
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlLeft, true, Modifiers::CTRL_LEFT)),
            None
        );
        assert_eq!(
            capture.on(modifier(
                ModifierKey::AltLeft,
                true,
                Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT
            )),
            None
        );
        assert_eq!(
            capture.on(main_key(
                MainKey::Space,
                Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT
            )),
            Some(Trigger::Chord(Chord::ctrl_alt_space()))
        );
    }

    #[test]
    fn ctrl_c_is_captured_as_the_chord_it_is() {
        // Whether a chord is *acceptable* is the application's rule — `Ctrl+Space` is
        // refused there, and so is a chord with no modifier. This crate reports what was
        // pressed.
        let mut capture = InProgress::default();
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlLeft, true, Modifiers::CTRL_LEFT)),
            None
        );
        let trigger = capture
            .on(main_key(MainKey::Letter('c'), Modifiers::CTRL_LEFT))
            .expect("Ctrl+C is a chord");
        assert_eq!(trigger.to_string(), "Ctrl+C");
    }

    #[test]
    fn a_release_inside_a_chord_in_progress_is_not_a_lone_key() {
        let mut capture = InProgress::default();
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlLeft, true, Modifiers::CTRL_LEFT)),
            None
        );
        assert_eq!(
            capture.on(modifier(
                ModifierKey::AltLeft,
                true,
                Modifiers::CTRL_LEFT | Modifiers::OPT_LEFT
            )),
            None
        );
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlLeft, false, Modifiers::OPT_LEFT)),
            None,
            "the Ctrl was not alone: something was pressed in between, so this is still waiting"
        );
        assert_eq!(
            capture.on(modifier(ModifierKey::AltLeft, false, Modifiers::empty())),
            None,
            "and neither was the Alt"
        );

        // The next clean press is still read.
        assert_eq!(
            capture.on(modifier(
                ModifierKey::CtrlRight,
                true,
                Modifiers::CTRL_RIGHT
            )),
            None
        );
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlRight, false, Modifiers::empty())),
            Some(Trigger::Key(ModifierOnly::RIGHT_CTRL))
        );
    }

    #[test]
    fn auto_repeat_of_the_held_modifier_is_still_one_press() {
        let mut capture = InProgress::default();
        for _ in 0..12 {
            assert_eq!(
                capture.on(modifier(
                    ModifierKey::CtrlRight,
                    true,
                    Modifiers::CTRL_RIGHT
                )),
                None
            );
        }
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlRight, false, Modifiers::empty())),
            Some(Trigger::Key(ModifierOnly::RIGHT_CTRL)),
            "a held key repeats; the finger never left it"
        );
    }

    #[test]
    fn a_key_with_no_name_here_ends_the_candidacy_and_an_echo_does_not() {
        let mut capture = InProgress::default();
        assert_eq!(
            capture.on(modifier(
                ModifierKey::CtrlRight,
                true,
                Modifiers::CTRL_RIGHT
            )),
            None
        );
        // A state echo: no key, no named modifier, nothing changed.
        assert_eq!(
            capture.on(KeyStep {
                is_down: true,
                changed_modifier: None,
                key: Pressed::Nothing,
                modifiers: Modifiers::CTRL_RIGHT,
            }),
            None
        );
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlRight, false, Modifiers::empty())),
            Some(Trigger::Key(ModifierOnly::RIGHT_CTRL)),
            "an echo must not cost a person their press"
        );

        // Enter during the hold is a key this crate cannot name, and it is still a key.
        let mut capture = InProgress::default();
        assert_eq!(
            capture.on(modifier(
                ModifierKey::CtrlRight,
                true,
                Modifiers::CTRL_RIGHT
            )),
            None
        );
        assert_eq!(
            capture.on(KeyStep {
                is_down: true,
                changed_modifier: None,
                key: Pressed::Unnamed,
                modifiers: Modifiers::CTRL_RIGHT,
            }),
            None
        );
        assert_eq!(
            capture.on(modifier(ModifierKey::CtrlRight, false, Modifiers::empty())),
            None,
            "Ctrl+Enter is not a lone right Ctrl"
        );
    }
}
