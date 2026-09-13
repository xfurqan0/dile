//! The Windows adapter: virtual-key codes in, [`Action`]s out.
//!
//! Thin on purpose. Every rule lives in [`crate::state`]; this module owns three jobs and no
//! judgement — install a global hook, translate what it reports into [`Event`]s, and keep
//! the clock ticking so the safety ceiling of [`crate::config::DEFAULT_MAX_HOLD_MS`] can
//! fire during a press that produces no events at all.
//!
//! ## The hook itself is not here
//!
//! `handy-keys` owns the `WH_KEYBOARD_LL` callback, and that is the point of choosing it.
//! `docs/PROJECT.md` §3 picked it over `rdev` because before
//! `tauri-plugin-global-shortcut` 2.3.2 the Windows "Released" event in the alternatives was
//! a 50 ms poll that burned a core; `handy-keys` reports the release from the hook, and its
//! callback does nothing but a channel send. A low-level hook runs on the thread that
//! installed it and Windows removes it if it does not return within `LowLevelHooksTimeout`,
//! so nothing that can block or allocate may live inside it. This module therefore does its
//! own work — the state machine included — on a thread of its own, one channel away.
//!
//! ## Two channels, one thread
//!
//! ```text
//!   OS hook thread  ──KeyEvent──▶  this crate's thread  ──Emitted──▶  the application
//!    (handy-keys)                  (translate + decide)                      │
//!                                          ▲                                 │
//!                                          └──────────Command────────────────┘
//! ```
//!
//! The command channel exists for one message, [`HotkeyListener::reset`], because focus loss
//! is something only the application can see.
//!
//! ## Swallowing the chord: the primary one, and never the second key
//!
//! The hook is installed **blocking**, with exactly one hotkey in the blocking set: the
//! primary chord. `Ctrl+Alt+Space` therefore does not reach the focused window, which is the
//! point of it — a space landing in the editor the user is dictating into is a bug, not a
//! side effect. What a swallowed Alt chord needs on Windows comes with `handy-keys`: it
//! injects a menu mask, so releasing Alt after a blocked key does not read as a lone tap and
//! arm the window's menu bar.
//!
//! **The second key is never in that set**, and the asymmetry is deliberate. It is a bare
//! modifier — the right Ctrl — and swallowing it would take `Ctrl+C` away from every
//! application on the machine. A lone modifier also has nothing to swallow: it means nothing
//! until another key joins it, which is exactly the case [`crate::state`] resolves by
//! starting a recording and withdrawing it.
//!
//! Blocking is a system-wide behaviour, so the set is built from the configured chord and
//! nothing else. A chord whose key this build of `handy-keys` cannot name is not blocked and
//! the hook is installed observe-only, because a trigger that reports is worth more than one
//! that refuses to start.
//!
//! ## The panel's three keys, added and taken away while the hook runs
//!
//! WP5b puts a window on screen that never takes focus, so Enter, Esc and `Ctrl+C` have to be
//! read from the hook — and **blocked**, because an Enter that also reaches the editor behind
//! the panel would put a newline in the document the user is dictating into, which is the
//! same bug as the swallowed space of WP2.
//!
//! [`HotkeyListener::panel_keys`] is how the application arms them, and nothing is reinstalled
//! to do it: `handy-keys` takes the blocking set as an `Arc<Mutex<HashSet<Hotkey>>>` and reads
//! it inside the hook on every event, so adding three entries takes effect on the next key
//! press. The listener keeps its handle on that set for exactly this.
//!
//! **The three entries are spelled so that only the bare key is taken.** A hotkey with no
//! modifiers matches only an event with no modifiers, so `Shift+Enter` is not blocked and
//! still reaches the panel as a newline; `Ctrl+C` is the compound Ctrl, so either side of the
//! keyboard counts and `Ctrl+Shift+C` does not. The same three definitions decide what is
//! blocked and what is reported, so the hook and the machine cannot disagree about which key
//! belongs to the panel.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use handy_keys::{
    BlockingHotkeys, Hotkey as OsHotkey, Key as OsKey, KeyEvent as OsKeyEvent, KeyboardListener,
    Modifiers,
};

use crate::config::HotkeyConfig;
use crate::error::Error;
use crate::keys::{Key, MainKey, ModifierFamily, ModifierKey};
use crate::state::{Action, Actions, Event, HotkeyMachine, PanelKey};

/// How long the thread waits for a key event before feeding the machine a tick.
///
/// It is only the resolution of the safety ceiling — a stuck key is caught within a quarter
/// second of five minutes — so there is no reason to spin faster. Every real decision is
/// driven by an event, not by this.
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// One action, and the moment the machine decided on it.
///
/// The timestamp is the event's own, not the moment the application read it, so a queue that
/// backed up behind a slow UI thread does not corrupt the record of when the user pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emitted {
    /// What to do.
    pub action: Action,
    /// When the key event that caused it happened.
    pub at: Instant,
}

/// A message from the application to the listener thread.
enum Command {
    /// Focus was lost; forget every key believed to be held.
    Reset,
    /// The review panel appeared or went away; Enter, Esc and `Ctrl+C` change hands.
    PanelKeys(bool),
    /// The panel's re-record button was pressed.
    Rerecord,
}

/// A live global hotkey listener.
///
/// Dropping it takes the hook down and joins both threads.
///
/// ```no_run
/// use std::time::Duration;
/// use dile_hotkey::{HotkeyConfig, HotkeyListener};
///
/// let listener = HotkeyListener::spawn(HotkeyConfig::default())?;
/// while let Ok(emitted) = listener.actions().recv_timeout(Duration::from_secs(1)) {
///     println!("{:?}", emitted.action);
/// }
/// # Ok::<(), dile_hotkey::Error>(())
/// ```
#[derive(Debug)]
pub struct HotkeyListener {
    actions: Receiver<Emitted>,
    commands: Sender<Command>,
    running: Arc<AtomicBool>,
    /// The set the hook reads on every key event, or `None` when the hook is observe-only.
    ///
    /// Held so that [`HotkeyListener::panel_keys`] can add and remove the panel's three
    /// entries without taking the hook down and putting another one up — a reinstall would
    /// drop every key held at that moment, which during a dictation is the dictation.
    blocking: Option<BlockingHotkeys>,
    thread: Option<JoinHandle<()>>,
}

impl HotkeyListener {
    /// Install the global hook and start deciding.
    ///
    /// # Errors
    ///
    /// [`Error::Hook`] if the operating system refused the hook, [`Error::Thread`] if the
    /// thread could not be spawned.
    pub fn spawn(config: HotkeyConfig) -> Result<Self, Error> {
        let (action_tx, action_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        // The hook must be installed on the thread that will pump it, so the constructor
        // runs over there and reports back before `spawn` returns.
        let (ready_tx, ready_rx) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));

        // Built here rather than on the listener thread so that an unnameable chord is a
        // decision taken once, in the open, instead of a branch inside the hook's setup.
        let blocking = blocking_set(&config);
        // The handle keeps a second reference: the hook owns the set, and the application
        // adds the panel's keys to it while that hook is running.
        let shared = blocking.clone();

        let thread_running = Arc::clone(&running);
        let thread = thread::Builder::new()
            .name("dile-hotkey".to_owned())
            .spawn(move || {
                let installed = match blocking {
                    Some(hotkeys) => KeyboardListener::new_with_blocking(hotkeys),
                    None => KeyboardListener::new(),
                };
                match installed {
                    Ok(keyboard) => {
                        if ready_tx.send(Ok(())).is_err() {
                            return;
                        }
                        run(config, &keyboard, &action_tx, &command_rx, &thread_running);
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(Error::Hook(error.to_string())));
                    }
                }
            })
            .map_err(|error| Error::Thread(error.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                actions: action_rx,
                commands: command_tx,
                running,
                blocking: shared,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::NotRunning)
            }
        }
    }

    /// The stream of actions. Use the ordinary [`Receiver`] API on it.
    #[must_use]
    pub const fn actions(&self) -> &Receiver<Emitted> {
        &self.actions
    }

    /// Tell the machine to forget every key it believes is held.
    ///
    /// The application calls this when the window manager took focus away mid-press, since
    /// the key-up may never arrive. Any resulting action comes back through
    /// [`HotkeyListener::actions`] like every other.
    ///
    /// # Errors
    ///
    /// [`Error::NotRunning`] if the listener thread has already ended.
    pub fn reset(&self) -> Result<(), Error> {
        self.commands
            .send(Command::Reset)
            .map_err(|_| Error::NotRunning)
    }

    /// Hand Enter, Esc and `Ctrl+C` to the review panel, or give them back to the machine.
    ///
    /// Two things happen, and both have to: the three keys are added to the hook's blocking
    /// set so they never reach the window behind the panel, and the state machine is told to
    /// turn them into [`Action::PanelTransfer`], [`Action::PanelCancel`] and
    /// [`Action::PanelCopy`]. Call it with `false` the moment the panel hides — while it is
    /// `true`, Esc does nothing anywhere else on the machine.
    ///
    /// A listener whose hook is observe-only still reports the keys; it simply cannot stop
    /// them, which is the same trade `blocking_set` makes for an unnameable chord.
    ///
    /// # Errors
    ///
    /// [`Error::NotRunning`] if the listener thread has already ended.
    pub fn panel_keys(&self, enabled: bool) -> Result<(), Error> {
        if let Some(blocking) = self.blocking.as_ref() {
            match blocking.lock() {
                Ok(mut hotkeys) => {
                    for hotkey in panel_hotkeys() {
                        if enabled {
                            hotkeys.insert(hotkey);
                        } else {
                            hotkeys.remove(&hotkey);
                        }
                    }
                }
                // The hook is still reading whatever the set held before. Reporting is
                // unaffected, so the panel works and the keys also reach the window behind
                // it — a worse product than intended, not a broken one.
                Err(_) => {
                    log::warn!("the blocking set is poisoned; the panel keys are not blocked")
                }
            }
        }
        self.commands
            .send(Command::PanelKeys(enabled))
            .map_err(|_| Error::NotRunning)
    }

    /// Start a recording again from the panel, with no key pressed.
    ///
    /// Only does anything in [`crate::Mode::Toggle`]; see [`HotkeyMachine::rerecord`]. Any
    /// resulting action comes back through [`HotkeyListener::actions`] like every other.
    ///
    /// # Errors
    ///
    /// [`Error::NotRunning`] if the listener thread has already ended.
    pub fn rerecord(&self) -> Result<(), Error> {
        self.commands
            .send(Command::Rerecord)
            .map_err(|_| Error::NotRunning)
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // The loop wakes on its own every TICK_INTERVAL, so this join is short. Dropping
        // the `KeyboardListener` inside it is what actually removes the hook.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The set of hotkeys the hook swallows: the primary chord, or nothing.
///
/// `None` means observe-only, and there are two ways to get there: a chord whose key this
/// build of `handy-keys` cannot name, and a chord `handy-keys` rejects as empty. Neither can
/// happen with the shipped default; both are a reason to keep reporting rather than to fail
/// to start.
///
/// The second key is deliberately absent. See the module documentation: swallowing a bare
/// right Ctrl would take `Ctrl+C` away from the whole machine.
fn blocking_set(config: &HotkeyConfig) -> Option<BlockingHotkeys> {
    let primary = primary_hotkey(config)?;
    let mut hotkeys = HashSet::with_capacity(1);
    hotkeys.insert(primary);
    Some(Arc::new(Mutex::new(hotkeys)))
}

/// The primary chord in the hook's own vocabulary.
fn primary_hotkey(config: &HotkeyConfig) -> Option<OsHotkey> {
    let key = to_os_key(config.primary.key())?;
    let modifiers = ModifierFamily::ALL
        .iter()
        .filter(|family| config.primary.requires(**family))
        .fold(Modifiers::empty(), |held, family| {
            held | os_modifier(*family)
        });
    OsHotkey::new(modifiers, key).ok()
}

/// The three keys the review panel claims, with the modifier state each one requires.
///
/// One table, read twice — to decide what the hook swallows and to decide what this module
/// reports — so that a key can never be blocked without also being reported, which would be
/// a key that disappears from the machine and arrives nowhere.
///
/// `Modifiers::empty()` is not "any modifier": `handy_keys::Modifiers::matches` requires the
/// event to hold nothing from a group the hotkey does not name, so a bare Enter is taken and
/// `Shift+Enter` is left alone for the panel to turn into a newline. `CTRL` is the compound
/// flag, so either Ctrl counts and `Ctrl+Shift+C` does not.
const PANEL_KEYS: [(Modifiers, OsKey, PanelKey); 3] = [
    (Modifiers::empty(), OsKey::Return, PanelKey::Transfer),
    (Modifiers::empty(), OsKey::Escape, PanelKey::Cancel),
    (Modifiers::CTRL, OsKey::C, PanelKey::Copy),
];

/// [`PANEL_KEYS`] in the form the hook's blocking set holds.
fn panel_hotkeys() -> Vec<OsHotkey> {
    PANEL_KEYS
        .iter()
        .filter_map(|&(modifiers, key, _)| OsHotkey::new(modifiers, key).ok())
        .collect()
}

/// Which of the panel's keys this event is, if it is one of them.
fn panel_key_of(event: &OsKeyEvent) -> Option<PanelKey> {
    let pressed = event.key?;
    PANEL_KEYS
        .iter()
        .find(|&&(modifiers, key, _)| key == pressed && modifiers.matches(event.modifiers))
        .map(|&(_, _, panel)| panel)
}

/// A modifier family as the side-agnostic flag pair `handy-keys` matches with.
///
/// The compound flags rather than one side: `docs/PROJECT.md` §3 writes the chord in
/// families, so a user holding the right Ctrl triggers the same hotkey as one holding the
/// left, and the hook has to agree with the state machine about that or the chord would be
/// decided in one place and swallowed in another.
const fn os_modifier(family: ModifierFamily) -> Modifiers {
    match family {
        ModifierFamily::Ctrl => Modifiers::CTRL,
        ModifierFamily::Alt => Modifiers::OPT,
        ModifierFamily::Shift => Modifiers::SHIFT,
        ModifierFamily::Meta => Modifiers::CMD,
    }
}

/// The listener thread: translate, decide, forward, repeat.
fn run(
    config: HotkeyConfig,
    keyboard: &KeyboardListener,
    actions: &Sender<Emitted>,
    commands: &Receiver<Command>,
    running: &AtomicBool,
) {
    // Resolved once: the state machine speaks `MainKey`, the hook speaks `handy_keys::Key`,
    // and a chord's key never changes while a listener is alive. A key this build of
    // `handy-keys` cannot name leaves the chord unmatchable, which is the honest outcome —
    // the second key, if configured, still works.
    let chord_key = to_os_key(config.primary.key());
    let mut machine = HotkeyMachine::new(config);

    while running.load(Ordering::SeqCst) {
        loop {
            match commands.try_recv() {
                Ok(Command::Reset) => {
                    let now = Instant::now();
                    forward(actions, machine.reset(now), now);
                }
                Ok(Command::PanelKeys(enabled)) => machine.set_panel_keys(enabled),
                Ok(Command::Rerecord) => {
                    let now = Instant::now();
                    forward(actions, machine.rerecord(now), now);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        match keyboard.recv_timeout(TICK_INTERVAL) {
            Ok(event) => {
                let now = Instant::now();
                if let Some(translated) = translate(&event, chord_key, now) {
                    forward(actions, machine.on_event(translated), now);
                }
            }
            Err(handy_keys::Error::Timeout) => {
                let now = Instant::now();
                forward(actions, machine.on_event(Event::Tick(now)), now);
            }
            // The hook is gone. Nothing this thread does can bring it back.
            Err(_) => return,
        }
    }
}

fn forward(actions: &Sender<Emitted>, produced: Actions, at: Instant) {
    for &action in produced.as_slice() {
        // A receiver that hung up means the application is shutting down; the loop notices
        // through `running` on its next pass.
        let _ = actions.send(Emitted { action, at });
    }
}

/// Turn one OS key event into the machine's vocabulary.
///
/// Returns `None` for events the machine has nothing to say about — the release of a key it
/// never named, and the modifier-state echoes the hook sends that changed nothing.
fn translate(event: &OsKeyEvent, chord_key: Option<OsKey>, now: Instant) -> Option<Event> {
    if let Some(changed) = event.changed_modifier {
        return Some(match map_modifier(changed) {
            Some(modifier) => {
                if event.is_key_down {
                    Event::Down(Key::Modifier(modifier), now)
                } else {
                    Event::Up(Key::Modifier(modifier), now)
                }
            }
            // A modifier with no name in this crate's vocabulary — `Fn` on a laptop. It is
            // still "some other key" for the purpose of withdrawing a second-key press.
            None if event.is_key_down => Event::OtherKeyDown(now),
            None => return None,
        });
    }

    let key = event.key?;
    if Some(key) == chord_key {
        let named = Key::Main(from_os_key(key)?);
        return Some(if event.is_key_down {
            Event::Down(named, now)
        } else {
            Event::Up(named, now)
        });
    }

    // The panel's keys, after the chord and before everything else: a user who binds their
    // chord to one of them has asked for a trigger, and a trigger outranks a window button.
    // Whether the panel is up is not decided here — `HotkeyMachine` owns that, so the rule
    // is unit-testable and the hook stays a translator.
    if event.is_key_down
        && let Some(panel) = panel_key_of(event)
    {
        return Some(Event::Panel(panel, now));
    }

    // Everything else on the keyboard, and every mouse button. Only the press matters: it is
    // what turns a lone Right Ctrl into the opening of `Ctrl+C`.
    event.is_key_down.then_some(Event::OtherKeyDown(now))
}

/// Every sided modifier `handy-keys` reports, in this crate's terms.
///
/// `OPT` is the crate's macOS-flavoured name for Alt; on Windows it is `VK_LMENU` and
/// `VK_RMENU`, and `CMD` is the Windows key.
const MODIFIERS: [(Modifiers, ModifierKey); 8] = [
    (Modifiers::CTRL_LEFT, ModifierKey::CtrlLeft),
    (Modifiers::CTRL_RIGHT, ModifierKey::CtrlRight),
    (Modifiers::OPT_LEFT, ModifierKey::AltLeft),
    (Modifiers::OPT_RIGHT, ModifierKey::AltRight),
    (Modifiers::SHIFT_LEFT, ModifierKey::ShiftLeft),
    (Modifiers::SHIFT_RIGHT, ModifierKey::ShiftRight),
    (Modifiers::CMD_LEFT, ModifierKey::MetaLeft),
    (Modifiers::CMD_RIGHT, ModifierKey::MetaRight),
];

fn map_modifier(changed: Modifiers) -> Option<ModifierKey> {
    MODIFIERS
        .iter()
        .find(|(flag, _)| *flag == changed)
        .map(|&(_, key)| key)
}

/// The chord's key, in the hook's vocabulary.
///
/// Through `handy-keys`' own string form rather than through sixty match arms: parsing a
/// hotkey from a string is the crate's advertised API (`"Ctrl+Alt+Space".parse()`), so its
/// spelling of a key is a contract rather than an accident, and one table beats two.
fn to_os_key(key: MainKey) -> Option<OsKey> {
    let name = match key {
        MainKey::Space => "space".to_owned(),
        MainKey::Letter(letter) => letter.to_string(),
        MainKey::Digit(digit) => digit.to_string(),
        MainKey::Function(number) => format!("f{number}"),
    };
    name.parse().ok()
}

fn from_os_key(key: OsKey) -> Option<MainKey> {
    if key == OsKey::Space {
        return Some(MainKey::Space);
    }
    let name = key.to_string().to_lowercase();
    // `F` on its own is the letter, not a function key with no number: the digits have to
    // parse before this branch may claim the name.
    if let Some(digits) = name.strip_prefix('f')
        && let Ok(number) = digits.parse::<u8>()
    {
        return MainKey::function(number);
    }
    let mut characters = name.chars();
    let (Some(single), None) = (characters.next(), characters.next()) else {
        return None;
    };
    match single.to_digit(10) {
        Some(digit) => u8::try_from(digit).ok().and_then(MainKey::digit),
        None => MainKey::letter(single),
    }
}

#[cfg(test)]
mod tests {
    use handy_keys::{Key as OsKey, KeyEvent as OsKeyEvent, Modifiers};
    use std::time::Instant;

    use super::{
        blocking_set, from_os_key, map_modifier, panel_hotkeys, panel_key_of, primary_hotkey,
        to_os_key, translate,
    };
    use crate::config::HotkeyConfig;
    use crate::keys::{Key, MainKey, ModifierKey, ModifierOnly};
    use crate::state::{Event, PanelKey};

    fn modifier_event(changed: Modifiers, is_key_down: bool) -> OsKeyEvent {
        OsKeyEvent {
            modifiers: changed,
            key: None,
            is_key_down,
            changed_modifier: Some(changed),
        }
    }

    fn key_event(key: OsKey, is_key_down: bool) -> OsKeyEvent {
        OsKeyEvent {
            modifiers: Modifiers::empty(),
            key: Some(key),
            is_key_down,
            changed_modifier: None,
        }
    }

    #[test]
    fn every_sided_modifier_the_hook_reports_has_a_name_here() {
        for (flag, expected) in super::MODIFIERS {
            assert_eq!(map_modifier(flag), Some(expected));
        }
        // A compound flag is not a single physical key and must not be mistaken for one.
        assert_eq!(map_modifier(Modifiers::CTRL), None);
        assert_eq!(map_modifier(Modifiers::FN), None);
    }

    #[test]
    fn the_keys_a_chord_can_be_built_from_survive_the_round_trip() {
        let keys = [
            MainKey::Space,
            MainKey::Letter('d'),
            MainKey::Letter('f'),
            MainKey::Letter('z'),
            MainKey::Digit(0),
            MainKey::Digit(9),
            MainKey::Function(1),
            MainKey::Function(12),
            MainKey::Function(24),
        ];
        for key in keys {
            let os_key = to_os_key(key).unwrap_or_else(|| panic!("{key:?} has no OS spelling"));
            assert_eq!(
                from_os_key(os_key),
                Some(key),
                "{key:?} did not survive the round trip"
            );
        }
    }

    #[test]
    fn the_default_chord_key_is_the_space_bar_the_hook_reports() {
        assert_eq!(to_os_key(MainKey::Space), Some(OsKey::Space));
    }

    #[test]
    fn a_key_outside_the_chord_arrives_without_an_identity() {
        let now = Instant::now();
        let chord_key = to_os_key(MainKey::Space);

        assert_eq!(
            translate(&key_event(OsKey::C, true), chord_key, now),
            Some(Event::OtherKeyDown(now)),
            "the C of Ctrl+C carries no name into the machine"
        );
        assert_eq!(
            translate(&key_event(OsKey::C, false), chord_key, now),
            None,
            "and its release is nothing at all"
        );
    }

    #[test]
    fn the_chord_key_keeps_its_identity_in_both_directions() {
        let now = Instant::now();
        let chord_key = to_os_key(MainKey::Space);

        assert_eq!(
            translate(&key_event(OsKey::Space, true), chord_key, now),
            Some(Event::Down(Key::Main(MainKey::Space), now))
        );
        assert_eq!(
            translate(&key_event(OsKey::Space, false), chord_key, now),
            Some(Event::Up(Key::Main(MainKey::Space), now))
        );
    }

    #[test]
    fn right_ctrl_is_distinguishable_from_left_ctrl() {
        let now = Instant::now();
        let chord_key = to_os_key(MainKey::Space);

        assert_eq!(
            translate(&modifier_event(Modifiers::CTRL_RIGHT, true), chord_key, now),
            Some(Event::Down(Key::Modifier(ModifierKey::CtrlRight), now)),
            "the second key of §3 is a side, so the side has to survive translation"
        );
        assert_eq!(
            translate(&modifier_event(Modifiers::CTRL_LEFT, false), chord_key, now),
            Some(Event::Up(Key::Modifier(ModifierKey::CtrlLeft), now))
        );
        assert_eq!(
            translate(&modifier_event(Modifiers::OPT_RIGHT, true), chord_key, now),
            Some(Event::Down(Key::Modifier(ModifierKey::AltRight), now)),
            "AltGr on a Turkish layout is the right Alt"
        );
    }

    #[test]
    fn an_unnamed_modifier_still_counts_as_some_other_key() {
        let now = Instant::now();
        let chord_key = to_os_key(MainKey::Space);

        assert_eq!(
            translate(&modifier_event(Modifiers::FN, true), chord_key, now),
            Some(Event::OtherKeyDown(now))
        );
        assert_eq!(
            translate(&modifier_event(Modifiers::FN, false), chord_key, now),
            None
        );
    }

    #[test]
    fn the_hook_swallows_the_primary_chord_on_either_side_of_the_keyboard() {
        let hotkey = primary_hotkey(&HotkeyConfig::default()).expect("the default chord");

        assert_eq!(hotkey.key, Some(OsKey::Space));
        // The compound flags, so that either Ctrl and either Alt match — the same rule the
        // state machine follows. `matches` is what the hook calls on every key event.
        assert!(
            hotkey
                .modifiers
                .matches(Modifiers::CTRL_RIGHT | Modifiers::OPT_LEFT)
        );
        assert!(
            hotkey
                .modifiers
                .matches(Modifiers::CTRL_LEFT | Modifiers::OPT_RIGHT)
        );
        // Ctrl+Space is the chord §3 excludes permanently, and it must not be swallowed
        // either — the IME owns it.
        assert!(!hotkey.modifiers.matches(Modifiers::CTRL_LEFT));
    }

    #[test]
    fn the_panel_takes_the_bare_key_and_leaves_the_chorded_one() {
        let now = Instant::now();
        let chord_key = to_os_key(MainKey::Space);

        let bare = |key| OsKeyEvent {
            modifiers: Modifiers::empty(),
            key: Some(key),
            is_key_down: true,
            changed_modifier: None,
        };
        assert_eq!(panel_key_of(&bare(OsKey::Return)), Some(PanelKey::Transfer));
        assert_eq!(panel_key_of(&bare(OsKey::Escape)), Some(PanelKey::Cancel));
        assert_eq!(panel_key_of(&bare(OsKey::C)), None, "C alone is just C");

        let with = |modifiers, key| OsKeyEvent {
            modifiers,
            key: Some(key),
            is_key_down: true,
            changed_modifier: None,
        };
        assert_eq!(
            panel_key_of(&with(Modifiers::CTRL_LEFT, OsKey::C)),
            Some(PanelKey::Copy)
        );
        assert_eq!(
            panel_key_of(&with(Modifiers::CTRL_RIGHT, OsKey::C)),
            Some(PanelKey::Copy),
            "either Ctrl copies, the way the chord matches either side"
        );
        assert_eq!(
            panel_key_of(&with(
                Modifiers::CTRL_LEFT | Modifiers::SHIFT_LEFT,
                OsKey::C
            )),
            None,
            "Ctrl+Shift+C belongs to whatever the user is working in"
        );
        assert_eq!(
            panel_key_of(&with(Modifiers::SHIFT_LEFT, OsKey::Return)),
            None,
            "Shift+Enter has to reach the panel as a newline"
        );

        // And the translation agrees with the table in both directions.
        assert_eq!(
            translate(&bare(OsKey::Escape), chord_key, now),
            Some(Event::Panel(PanelKey::Cancel, now))
        );
        assert_eq!(
            translate(&key_event(OsKey::Escape, false), chord_key, now),
            None,
            "only the press matters; a release is nothing"
        );
    }

    #[test]
    fn what_the_hook_swallows_for_the_panel_is_what_the_panel_is_told_about() {
        let hotkeys = panel_hotkeys();
        assert_eq!(
            hotkeys.len(),
            3,
            "one entry per key, or one of them escapes"
        );

        for hotkey in hotkeys {
            let event = OsKeyEvent {
                modifiers: hotkey.modifiers,
                key: hotkey.key,
                is_key_down: true,
                changed_modifier: None,
            };
            assert!(
                panel_key_of(&event).is_some(),
                "{hotkey:?} would be blocked and never reported"
            );
        }
    }

    #[test]
    fn the_second_key_is_never_in_the_blocking_set() {
        let config = HotkeyConfig {
            second_key: Some(ModifierOnly::RIGHT_CTRL),
            ..HotkeyConfig::default()
        };
        let blocking = blocking_set(&config).expect("the default chord is blockable");
        let hotkeys = blocking.lock().expect("an uncontended set");

        // One entry, and it is the chord. A bare right Ctrl in here would swallow Ctrl+C
        // for every application on the machine.
        assert_eq!(hotkeys.len(), 1);
        assert!(hotkeys.iter().all(|hotkey| hotkey.key.is_some()));
    }
}
