//! "Press the new chord": one key press, read once, and the hook comes straight back down.
//!
//! The settings window of `docs/PROJECT.md` WP5 lets a user change the trigger, and the only
//! honest way to ask for a chord is to let them press it. Typing one into a text field asks a
//! person to know that their keyboard calls the key `OEM_3`; pressing it asks them to press
//! it.
//!
//! **This is not a second listener.** [`next_chord`] installs its own observe-only hook,
//! waits for one key-down that completes a chord, and drops the hook before it returns. It
//! never blocks a key — a capture that swallowed the chord would swallow it for the whole
//! machine while a settings window sat open — and the application's real listener keeps
//! running underneath it. The most recently installed low-level hook is called first, so the
//! chord the running listener blocks is still seen here before that listener ever gets it.
//!
//! **Only the completed chord counts.** Modifiers held on their own produce nothing: the
//! capture ends on the first key-down whose key is one [`MainKey`] can name, with whatever
//! modifiers were held at that moment. A release is never a chord, and neither is a timeout —
//! which is [`Capture::TimedOut`] rather than an error, because nobody pressing nothing has
//! done anything wrong.

use std::time::{Duration, Instant};

use handy_keys::{KeyboardListener, Modifiers};

use crate::error::Error;
use crate::keys::{Chord, MainKey, ModifierFamily};

/// What one capture produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// The user pressed this.
    Chord(Chord),
    /// Nothing was pressed before the deadline.
    TimedOut,
}

/// The modifier families held, as this crate names them.
///
/// `handy-keys` reports the sided flags; a chord is written in families, so both sides of
/// each are folded into one. The same rule the state machine and the blocking set follow.
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

/// Wait for the user to press one chord.
///
/// Returns as soon as a key-down completes a chord, or [`Capture::TimedOut`] when `timeout`
/// passes with nothing pressed. The hook is removed either way.
///
/// # Errors
///
/// [`Error::Hook`] if the operating system refused the hook.
pub fn next_chord(timeout: Duration) -> Result<Capture, Error> {
    let keyboard = KeyboardListener::new().map_err(|error| Error::Hook(error.to_string()))?;
    let deadline = Instant::now() + timeout;

    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Capture::TimedOut);
        }

        match keyboard.recv_timeout(left) {
            Ok(event) => {
                // A modifier on its own is not a chord: the capture is waiting for the key
                // that completes one, and the modifier state travels on that key's event.
                if !event.is_key_down || event.changed_modifier.is_some() {
                    continue;
                }
                let Some(key) = event.key else {
                    continue;
                };
                let Ok(main) = key.to_string().parse::<MainKey>() else {
                    // A key outside this crate's small vocabulary — Enter, a media key, a
                    // mouse button. Ignored rather than returned: the user is still pressing.
                    continue;
                };
                return Ok(Capture::Chord(Chord::new(&families(event.modifiers), main)));
            }
            Err(handy_keys::Error::Timeout) => return Ok(Capture::TimedOut),
            Err(error) => return Err(Error::Hook(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::families;
    use crate::keys::ModifierFamily;
    use handy_keys::Modifiers;

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
}
