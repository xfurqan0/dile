//! The decision layer: key events in, application actions out, no operating system in sight.
//!
//! Everything that is hard about a push-to-talk trigger is a timing question, and timing
//! questions are exactly what a keyboard hook is worst at testing. So the clock is an
//! argument here: every event carries the [`Instant`] it happened at, the machine never
//! reads a clock of its own, and the twelve scenarios of WP2 run in microseconds as
//! ordinary unit tests. `listener.rs` is then allowed to be thin, because it holds no rules.
//!
//! ## The rules, and where they come from
//!
//! `docs/PROJECT.md` §3, row "Hotkey", plus the 2026-09-09 "Interaction model decided" entry.
//!
//! **Recording starts at the press, always.** Not at the threshold, not at the release.
//! A user who waits for a beep before speaking is a user the product has already annoyed, so
//! the first syllable has to be inside the buffer before anyone knows whether the press will
//! turn out to be real. The capture crate prepends a pre-roll on top of that, for the
//! milliseconds between the intention and the keypress. The consequence is that
//! [`Action::DiscardRecording`] is a normal outcome rather than an error path: a press that
//! turns out to be a tap throws its recording away.
//!
//! **An accidental tap never dictates.** Under `press_threshold_ms` the recording is
//! discarded and the panel opens idle instead — the user sees that the hotkey works without
//! a stray sentence landing in their editor.
//!
//! **The asymmetry in [`Mode::Toggle`] is deliberate.** `press_threshold_ms` is not
//! consulted there. A toggle user taps fast on purpose — that is what they chose toggle for
//! — so a 40 ms tap starts a recording that stays latched until the next tap, where in
//! [`Mode::Hold`] the same 40 ms would be discarded. The number that matters in toggle mode
//! is `hold_takeover_ms`: hold the chord past it and the press behaves as hold-to-talk,
//! ending on release. Both idioms therefore live on one key without a setting to explain.
//!
//! **The second key is hold-semantics only, in both modes.** It is a lone modifier, so the
//! machine cannot tell a dictation from the opening of `Ctrl+C` until something else
//! happens. It resolves that by starting anyway and withdrawing: any other key going down
//! during the hold means the press was a modifier chord, so the recording is discarded and
//! that press is ignored until the key comes back up.

use std::time::{Duration, Instant};

use crate::config::{HotkeyConfig, Mode};
use crate::keys::{Key, ModifierFamily, ModifierKey};

/// One of the three keys the review panel claims while it is on screen.
///
/// They are neither a trigger nor a chord: they belong to a window that is already showing,
/// and they mean nothing when it is not. `docs/PROJECT.md` §3 names all three — Enter
/// transfers, Esc cancels, `Ctrl+C` copies — and the reason they are read here rather than
/// in the webview is that the panel never takes focus, so a press is on its way to somebody
/// else's window by the time this crate sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKey {
    /// Enter, with no modifier held.
    Transfer,
    /// Esc.
    Cancel,
    /// `Ctrl+C`.
    Copy,
}

impl PanelKey {
    /// What the application does when this key arrives while the panel is up.
    #[must_use]
    pub const fn action(self) -> Action {
        match self {
            PanelKey::Transfer => Action::PanelTransfer,
            PanelKey::Cancel => Action::PanelCancel,
            PanelKey::Copy => Action::PanelCopy,
        }
    }
}

/// Something that happened to a key.
///
/// The OS adapter is responsible for the split: keys that belong to a configured trigger
/// arrive as [`Event::Down`] and [`Event::Up`] with their identity, and every other key on
/// the keyboard — and every mouse button — arrives as [`Event::OtherKeyDown`] with none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A key belonging to a configured trigger went down.
    Down(Key, Instant),
    /// A key belonging to a configured trigger went up.
    Up(Key, Instant),
    /// One of the review panel's three keys went down.
    ///
    /// The adapter sends these whenever they are pressed; whether they mean anything is
    /// [`HotkeyMachine::set_panel_keys`]'s answer, so that rule lives in this module with
    /// every other rule rather than in the hook.
    Panel(PanelKey, Instant),
    /// Any other key, or a mouse button, went down.
    OtherKeyDown(Instant),
    /// Time passed with nothing pressed.
    ///
    /// The app feeds these so the safety timeout can fire while the user is holding a key
    /// and producing no events at all. Roughly four a second is plenty; the machine does
    /// nothing with a tick that is not overdue.
    Tick(Instant),
}

/// What the application must do, in the order it is handed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Show the review panel in its idle state, with nothing in it.
    ///
    /// Only ever follows a [`Action::DiscardRecording`]: it is the feedback that says "the
    /// hotkey works, that press was too short to be a sentence".
    OpenPanelIdle,
    /// Begin capturing audio now.
    StartRecording,
    /// Stop capturing and send what was captured on to transcription.
    StopRecording,
    /// Stop capturing and throw the audio away. Nothing reaches transcription.
    DiscardRecording,
    /// The panel's Enter: transfer what is in it into the window it was aimed at.
    PanelTransfer,
    /// The panel's Esc: close it and paste nothing.
    PanelCancel,
    /// The panel's `Ctrl+C`: put the text on the clipboard and do not paste it.
    PanelCopy,
}

/// The actions one event produced — never more than two.
///
/// Two is not an arbitrary bound. The only pairs the rules can produce are a discarded tap
/// followed by an idle panel, and a second key withdrawing in the same instant the primary
/// chord completes; every other transition emits one action or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actions {
    // The unused tail is never read: `as_slice` stops at `len`. A filler variant costs
    // nothing and keeps the whole type `Copy` with a plain slice accessor.
    buf: [Action; 2],
    len: usize,
}

impl Actions {
    const fn none() -> Self {
        Self {
            buf: [Action::OpenPanelIdle; 2],
            len: 0,
        }
    }

    const fn one(action: Action) -> Self {
        Self {
            buf: [action, action],
            len: 1,
        }
    }

    const fn two(first: Action, second: Action) -> Self {
        Self {
            buf: [first, second],
            len: 2,
        }
    }

    /// The actions, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[Action] {
        &self.buf[..self.len]
    }

    /// Whether this event produced nothing at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn extend(&mut self, other: Self) {
        for &action in other.as_slice() {
            debug_assert!(
                self.len < self.buf.len(),
                "more than two actions in one event"
            );
            if self.len < self.buf.len() {
                self.buf[self.len] = action;
                self.len += 1;
            }
        }
    }
}

impl Default for Actions {
    fn default() -> Self {
        Self::none()
    }
}

/// Which trigger a live recording belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// The primary chord.
    Primary,
    /// The modifier-only second key.
    Second,
}

/// What the machine is currently doing, when it is doing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Active {
    /// A trigger is held down and recording; `since` is when the press began.
    Held { trigger: Trigger, since: Instant },
    /// Toggle mode: recording continues with nothing held, until the next tap.
    Latched,
}

/// The set of modifier keys currently down, one bit per physical key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HeldModifiers(u8);

impl HeldModifiers {
    fn insert(&mut self, key: ModifierKey) {
        self.0 |= key.bit();
    }

    fn remove(&mut self, key: ModifierKey) {
        self.0 &= !key.bit();
    }

    const fn contains(self, key: ModifierKey) -> bool {
        self.0 & key.bit() != 0
    }

    const fn has_family(self, family: ModifierFamily) -> bool {
        self.0 & family.bits() != 0
    }

    fn clear(&mut self) {
        self.0 = 0;
    }
}

/// The push-to-talk trigger, as a state machine.
///
/// Feed it [`Event`]s in the order they happen and perform the [`Action`]s it returns. It
/// reads no clock, touches no device and allocates nothing, so a test can drive a whole
/// dictation through it in a few microseconds.
///
/// ```
/// use std::time::{Duration, Instant};
/// use dile_hotkey::{Action, Event, HotkeyConfig, HotkeyMachine, Key, MainKey, ModifierKey};
///
/// let mut machine = HotkeyMachine::new(HotkeyConfig::default());
/// let t0 = Instant::now();
///
/// machine.on_event(Event::Down(Key::Modifier(ModifierKey::CtrlLeft), t0));
/// machine.on_event(Event::Down(Key::Modifier(ModifierKey::AltLeft), t0));
/// let started = machine.on_event(Event::Down(Key::Main(MainKey::Space), t0));
/// assert_eq!(started.as_slice(), &[Action::StartRecording]);
///
/// let released = t0 + Duration::from_millis(1_400);
/// let stopped = machine.on_event(Event::Up(Key::Main(MainKey::Space), released));
/// assert_eq!(stopped.as_slice(), &[Action::StopRecording]);
/// ```
#[derive(Debug, Clone)]
pub struct HotkeyMachine {
    config: HotkeyConfig,
    held: HeldModifiers,
    main_held: bool,
    active: Option<Active>,
    /// Ignore the primary chord until it is broken. Set when a recording ended while the
    /// chord was still held, so the rest of that press cannot start a new one.
    suppress_primary: bool,
    /// Ignore the second key until it comes back up. Set when the press turned out to be a
    /// modifier chord, or when the safety timeout ended it.
    suppress_second: bool,
    /// When the live recording began — which is not the same as when the current press
    /// began, because a toggle latch outlives the press that started it.
    recording_since: Option<Instant>,
    /// Whether the review panel is on screen and therefore owns Enter, Esc and `Ctrl+C`.
    panel_keys: bool,
}

impl HotkeyMachine {
    /// A machine in its idle state.
    #[must_use]
    pub fn new(config: HotkeyConfig) -> Self {
        Self {
            config,
            held: HeldModifiers::default(),
            main_held: false,
            active: None,
            suppress_primary: false,
            suppress_second: false,
            recording_since: None,
            panel_keys: false,
        }
    }

    /// The configuration this machine was built with.
    #[must_use]
    pub const fn config(&self) -> &HotkeyConfig {
        &self.config
    }

    /// Whether audio is being captured right now.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.recording_since.is_some()
    }

    /// Whether Enter, Esc and `Ctrl+C` currently belong to the review panel.
    #[must_use]
    pub const fn panel_keys(&self) -> bool {
        self.panel_keys
    }

    /// Hand the panel's three keys to the panel, or give them back to the machine.
    ///
    /// Switched on while the panel is on screen and off the moment it hides. It is a mode
    /// rather than a state of its own because the panel is not a phase of a dictation: it is
    /// up during a recording, during transcription and after both, and the keys mean the same
    /// thing in all three.
    pub const fn set_panel_keys(&mut self, enabled: bool) {
        self.panel_keys = enabled;
    }

    /// Feed one event and get back what the application must do.
    pub fn on_event(&mut self, event: Event) -> Actions {
        match event {
            Event::Down(key, now) => self.on_down(key, now),
            Event::Up(key, now) => self.on_up(key, now),
            Event::Panel(key, _) => self.on_panel_key(key),
            Event::OtherKeyDown(_) => self.withdraw_second_key(),
            Event::Tick(now) => self.on_tick(now),
        }
    }

    /// Start a recording the way a toggle tap would, with no key pressed.
    ///
    /// The panel's **re-record** button. In [`Mode::Toggle`] the recording has to begin at
    /// once — there is no key to hold — and it has to begin *latched*, so that the next tap
    /// of the chord stops it exactly as if the user had started it themselves. In
    /// [`Mode::Hold`] there is nothing to do: a hold-to-talk recording lives as long as a
    /// press, and one nobody is pressing would have no end. The application hides the panel
    /// either way and the next press starts fresh.
    pub fn rerecord(&mut self, now: Instant) -> Actions {
        if self.config.mode != Mode::Toggle || self.active.is_some() {
            return Actions::none();
        }
        self.active = Some(Active::Latched);
        self.recording_since = Some(now);
        Actions::one(Action::StartRecording)
    }

    /// Forget every key the machine believes is held, because it can no longer be sure.
    ///
    /// The app calls this when the window manager took focus away mid-press — a UAC prompt,
    /// `Win+L`, a full-screen game grabbing input — since the key-up for a press that began
    /// before the switch may never arrive.
    ///
    /// A press in progress is ended **exactly as a release at this instant would end it**,
    /// which keeps the short-press rule intact: a press that had not yet reached
    /// `press_threshold_ms` is discarded rather than dictated. A toggle latch is deliberately
    /// left alone — it does not depend on any key being held, and killing it here would
    /// break the ordinary toggle flow of tapping to start and then switching to the window
    /// you mean to dictate into.
    pub fn reset(&mut self, now: Instant) -> Actions {
        let out = self.end_active_press(now);
        self.held.clear();
        self.main_held = false;
        self.suppress_primary = false;
        self.suppress_second = false;
        out
    }

    fn on_down(&mut self, key: Key, now: Instant) -> Actions {
        let was_chord = self.chord_satisfied();
        let is_second = self.is_second_key(key);

        match key {
            Key::Modifier(modifier) => {
                if self.held.contains(modifier) {
                    // Key auto-repeat, or a duplicate the OS sent. Not a new press.
                    return Actions::none();
                }
                self.held.insert(modifier);
            }
            Key::Main(main) => {
                if main.normalized() != self.config.primary.key() {
                    // A named key that belongs to no trigger. The adapter should have sent
                    // `OtherKeyDown`; treat it as one rather than trusting it blindly.
                    return self.withdraw_second_key();
                }
                if self.main_held {
                    return Actions::none();
                }
                self.main_held = true;
            }
        }

        let mut out = Actions::none();

        // Anything other than the second key itself going down during a second-key hold
        // means the press was the start of `Ctrl+C`, not of a sentence.
        if !is_second {
            out.extend(self.withdraw_second_key());
        }

        if !was_chord && self.chord_satisfied() {
            out.extend(self.on_chord_down(now));
        } else if is_second {
            out.extend(self.on_second_key_down(now));
        }

        out
    }

    fn on_up(&mut self, key: Key, now: Instant) -> Actions {
        let was_chord = self.chord_satisfied();
        let is_second = self.is_second_key(key);

        match key {
            Key::Modifier(modifier) => {
                if !self.held.contains(modifier) {
                    // An up with no down in front of it: the press began before this
                    // machine was listening, or a reset already forgot it.
                    return Actions::none();
                }
                self.held.remove(modifier);
            }
            Key::Main(main) => {
                if main.normalized() != self.config.primary.key() || !self.main_held {
                    return Actions::none();
                }
                self.main_held = false;
            }
        }

        let mut out = Actions::none();

        // At most one of these two can own the release: they require different values of
        // `self.active`, so the two-action bound of `Actions` holds.
        if is_second {
            out.extend(self.on_second_key_up(now));
        }
        if was_chord && !self.chord_satisfied() {
            out.extend(self.on_chord_up(now));
        }

        out
    }

    fn on_chord_down(&mut self, now: Instant) -> Actions {
        if self.suppress_primary {
            return Actions::none();
        }
        match self.active {
            // The other trigger owns the recording. Nothing starts twice.
            Some(Active::Held { .. }) => Actions::none(),
            // Toggle mode, second tap. The press ends the recording; the release that
            // follows is swallowed, so a slow second tap is still one tap.
            Some(Active::Latched) => {
                self.active = None;
                self.recording_since = None;
                self.suppress_primary = true;
                Actions::one(Action::StopRecording)
            }
            None => {
                self.active = Some(Active::Held {
                    trigger: Trigger::Primary,
                    since: now,
                });
                self.recording_since = Some(now);
                Actions::one(Action::StartRecording)
            }
        }
    }

    fn on_second_key_down(&mut self, now: Instant) -> Actions {
        if self.suppress_second || self.active.is_some() {
            return Actions::none();
        }
        self.active = Some(Active::Held {
            trigger: Trigger::Second,
            since: now,
        });
        self.recording_since = Some(now);
        Actions::one(Action::StartRecording)
    }

    fn on_chord_up(&mut self, now: Instant) -> Actions {
        if self.suppress_primary {
            self.suppress_primary = false;
            return Actions::none();
        }
        match self.active {
            Some(Active::Held {
                trigger: Trigger::Primary,
                ..
            }) => self.end_active_press(now),
            _ => Actions::none(),
        }
    }

    fn on_second_key_up(&mut self, now: Instant) -> Actions {
        if self.suppress_second {
            self.suppress_second = false;
            return Actions::none();
        }
        match self.active {
            Some(Active::Held {
                trigger: Trigger::Second,
                ..
            }) => self.end_active_press(now),
            _ => Actions::none(),
        }
    }

    /// The one place a press in progress ends, whether the key came up or a reset said so.
    fn end_active_press(&mut self, now: Instant) -> Actions {
        let Some(Active::Held { trigger, since }) = self.active else {
            return Actions::none();
        };
        let held_for = now.saturating_duration_since(since);

        // The second key is hold-to-talk in both modes: it is a lone modifier, and latching
        // a recording to a key a hand rests on would be a trap rather than a feature.
        if trigger == Trigger::Second || self.config.mode == Mode::Hold {
            self.active = None;
            self.recording_since = None;
            return if held_for < self.press_threshold() {
                Actions::two(Action::DiscardRecording, Action::OpenPanelIdle)
            } else {
                Actions::one(Action::StopRecording)
            };
        }

        if held_for >= self.hold_takeover() {
            // Held long enough that the user clearly meant hold-to-talk.
            self.active = None;
            self.recording_since = None;
            Actions::one(Action::StopRecording)
        } else {
            // A tap. The recording carries on with nothing held, until the next tap.
            self.active = Some(Active::Latched);
            Actions::none()
        }
    }

    /// One of the panel's keys went down.
    ///
    /// It is still a key on the keyboard, so it withdraws a second-key press exactly as any
    /// other would: Esc pressed during a right-Ctrl hold is a person cancelling, not a
    /// person dictating. The panel action comes after the withdrawal, which is the pair the
    /// two-action bound of [`Actions`] allows for.
    ///
    /// With the panel down the key is nothing but another key, which is what makes the mode
    /// safe: the machine cannot emit a panel action while there is no panel to act on.
    fn on_panel_key(&mut self, key: PanelKey) -> Actions {
        let mut out = self.withdraw_second_key();
        if self.panel_keys {
            out.extend(Actions::one(key.action()));
        }
        out
    }

    /// The second key's press was a modifier chord after all.
    fn withdraw_second_key(&mut self) -> Actions {
        if let Some(Active::Held {
            trigger: Trigger::Second,
            ..
        }) = self.active
        {
            self.active = None;
            self.recording_since = None;
            self.suppress_second = true;
            Actions::one(Action::DiscardRecording)
        } else {
            Actions::none()
        }
    }

    fn on_tick(&mut self, now: Instant) -> Actions {
        let Some(started) = self.recording_since else {
            return Actions::none();
        };
        if now.saturating_duration_since(started) < self.max_hold() {
            return Actions::none();
        }

        // The recording ends, but the key may still be down. Suppress its trigger so the
        // rest of that press — auto-repeat included — cannot start another one.
        match self.active {
            Some(Active::Held {
                trigger: Trigger::Primary,
                ..
            }) => self.suppress_primary = true,
            Some(Active::Held {
                trigger: Trigger::Second,
                ..
            }) => self.suppress_second = true,
            _ => {}
        }
        self.active = None;
        self.recording_since = None;
        Actions::one(Action::StopRecording)
    }

    fn chord_satisfied(&self) -> bool {
        self.main_held
            && ModifierFamily::ALL.iter().all(|&family| {
                !self.config.primary.requires(family) || self.held.has_family(family)
            })
    }

    fn is_second_key(&self, key: Key) -> bool {
        match (key, self.config.second_key) {
            (Key::Modifier(modifier), Some(second)) => modifier == second.key(),
            _ => false,
        }
    }

    fn press_threshold(&self) -> Duration {
        Duration::from_millis(u64::from(self.config.press_threshold_ms))
    }

    fn hold_takeover(&self) -> Duration {
        Duration::from_millis(u64::from(self.config.hold_takeover_ms))
    }

    fn max_hold(&self) -> Duration {
        Duration::from_millis(u64::from(self.config.max_hold_ms))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Action, Event, HotkeyMachine, PanelKey};
    use crate::config::{HotkeyConfig, Mode};
    use crate::keys::{Chord, Key, MainKey, ModifierFamily, ModifierKey, ModifierOnly};

    const CTRL_L: Key = Key::Modifier(ModifierKey::CtrlLeft);
    const CTRL_R: Key = Key::Modifier(ModifierKey::CtrlRight);
    const ALT_L: Key = Key::Modifier(ModifierKey::AltLeft);
    const ALT_R: Key = Key::Modifier(ModifierKey::AltRight);
    const SHIFT_L: Key = Key::Modifier(ModifierKey::ShiftLeft);
    const SPACE: Key = Key::Main(MainKey::Space);

    /// A machine plus a clock that only moves when a test says so.
    struct Harness {
        machine: HotkeyMachine,
        base: Instant,
    }

    impl Harness {
        fn new(config: HotkeyConfig) -> Self {
            Self {
                machine: HotkeyMachine::new(config),
                base: Instant::now(),
            }
        }

        fn hold_mode() -> Self {
            Self::new(HotkeyConfig::default())
        }

        fn toggle_mode() -> Self {
            Self::new(HotkeyConfig {
                mode: Mode::Toggle,
                ..HotkeyConfig::default()
            })
        }

        fn with_second_key() -> Self {
            Self::new(HotkeyConfig {
                second_key: Some(ModifierOnly::RIGHT_CTRL),
                ..HotkeyConfig::default()
            })
        }

        fn at(&self, ms: u64) -> Instant {
            self.base + Duration::from_millis(ms)
        }

        fn down(&mut self, key: Key, ms: u64) -> Vec<Action> {
            let now = self.at(ms);
            self.machine
                .on_event(Event::Down(key, now))
                .as_slice()
                .to_vec()
        }

        fn up(&mut self, key: Key, ms: u64) -> Vec<Action> {
            let now = self.at(ms);
            self.machine
                .on_event(Event::Up(key, now))
                .as_slice()
                .to_vec()
        }

        fn other_key(&mut self, ms: u64) -> Vec<Action> {
            let now = self.at(ms);
            self.machine
                .on_event(Event::OtherKeyDown(now))
                .as_slice()
                .to_vec()
        }

        fn panel_key(&mut self, key: PanelKey, ms: u64) -> Vec<Action> {
            let now = self.at(ms);
            self.machine
                .on_event(Event::Panel(key, now))
                .as_slice()
                .to_vec()
        }

        fn rerecord(&mut self, ms: u64) -> Vec<Action> {
            let now = self.at(ms);
            self.machine.rerecord(now).as_slice().to_vec()
        }

        fn tick(&mut self, ms: u64) -> Vec<Action> {
            let now = self.at(ms);
            self.machine.on_event(Event::Tick(now)).as_slice().to_vec()
        }

        fn reset(&mut self, ms: u64) -> Vec<Action> {
            let now = self.at(ms);
            self.machine.reset(now).as_slice().to_vec()
        }

        /// Press the whole default chord, returning only what the last key produced.
        fn press_chord(&mut self, ms: u64) -> Vec<Action> {
            assert!(self.down(CTRL_L, ms).is_empty());
            assert!(self.down(ALT_L, ms).is_empty());
            self.down(SPACE, ms)
        }

        /// Release the chord by lifting its main key, which is what a hand actually does.
        fn release_chord(&mut self, ms: u64) -> Vec<Action> {
            let out = self.up(SPACE, ms);
            assert!(self.up(ALT_L, ms).is_empty());
            assert!(self.up(CTRL_L, ms).is_empty());
            out
        }
    }

    // ---- hold mode -------------------------------------------------------------------

    #[test]
    fn hold_a_tap_under_the_threshold_discards_and_opens_the_panel_idle() {
        let mut h = Harness::hold_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert_eq!(
            h.release_chord(120),
            vec![Action::DiscardRecording, Action::OpenPanelIdle],
            "an accidental tap never dictates"
        );
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn hold_a_real_press_records_from_the_press_and_stops_on_release() {
        let mut h = Harness::hold_mode();
        assert_eq!(
            h.press_chord(0),
            vec![Action::StartRecording],
            "recording begins at the press so the first word is never lost"
        );
        assert!(h.machine.is_recording());
        assert_eq!(h.release_chord(2_400), vec![Action::StopRecording]);
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn hold_the_press_length_is_measured_to_the_release_not_to_the_last_key_down() {
        let mut h = Harness::hold_mode();
        // Space first, then the modifiers: the press only counts from the moment the chord
        // is complete, which is the last key down.
        assert!(h.down(SPACE, 0).is_empty());
        assert!(h.down(CTRL_L, 400).is_empty());
        assert_eq!(h.down(ALT_L, 500), vec![Action::StartRecording]);
        assert_eq!(
            h.up(SPACE, 600),
            vec![Action::DiscardRecording, Action::OpenPanelIdle],
            "100 ms of chord is a tap even though a key was down for 600 ms"
        );
    }

    // ---- toggle mode -----------------------------------------------------------------

    #[test]
    fn toggle_a_tap_starts_and_the_next_tap_stops() {
        let mut h = Harness::toggle_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert!(
            h.release_chord(40).is_empty(),
            "a 40 ms tap is under press_threshold_ms and still latches: \
             toggle users tap fast, so the threshold is a hold-mode rule only"
        );
        assert!(h.machine.is_recording(), "the latch outlives the press");

        assert_eq!(h.press_chord(5_000), vec![Action::StopRecording]);
        assert!(
            h.release_chord(5_090).is_empty(),
            "the release of the stopping tap is swallowed, not read as a third tap"
        );
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn toggle_a_press_past_the_takeover_behaves_as_hold_to_talk() {
        let mut h = Harness::toggle_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert_eq!(
            h.release_chord(1_500),
            vec![Action::StopRecording],
            "held past hold_takeover_ms, so the release ends it instead of latching"
        );
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn toggle_a_slow_second_tap_is_still_one_tap() {
        let mut h = Harness::toggle_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert!(h.release_chord(100).is_empty());
        // The second tap is held for two seconds — past the takeover — but the recording
        // was already latched, so it stops on the press and the long release does nothing.
        assert_eq!(h.press_chord(9_000), vec![Action::StopRecording]);
        assert!(h.release_chord(11_000).is_empty());
        assert!(!h.machine.is_recording());
    }

    // ---- the second key --------------------------------------------------------------

    #[test]
    fn second_key_alone_is_a_clean_hold_to_talk() {
        let mut h = Harness::with_second_key();
        assert_eq!(h.down(CTRL_R, 0), vec![Action::StartRecording]);
        assert_eq!(h.up(CTRL_R, 3_000), vec![Action::StopRecording]);
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn second_key_tapped_under_the_threshold_discards_like_the_chord_does() {
        let mut h = Harness::with_second_key();
        assert_eq!(h.down(CTRL_R, 0), vec![Action::StartRecording]);
        assert_eq!(
            h.up(CTRL_R, 90),
            vec![Action::DiscardRecording, Action::OpenPanelIdle]
        );
    }

    #[test]
    fn second_key_that_turns_into_ctrl_c_discards_and_is_ignored_until_released() {
        let mut h = Harness::with_second_key();
        assert_eq!(h.down(CTRL_R, 0), vec![Action::StartRecording]);
        assert_eq!(
            h.other_key(80),
            vec![Action::DiscardRecording],
            "the C of Ctrl+C: the press was a modifier chord, not a sentence"
        );
        assert!(!h.machine.is_recording());

        assert!(
            h.other_key(200).is_empty(),
            "Ctrl+V right after Ctrl+C, same held Ctrl: still nothing"
        );
        assert!(
            h.up(CTRL_R, 900).is_empty(),
            "the release of a withdrawn press emits nothing, not even a stop"
        );
        // And the key works again on the next press.
        assert_eq!(h.down(CTRL_R, 1_000), vec![Action::StartRecording]);
    }

    #[test]
    fn second_key_does_not_double_start_while_the_chord_is_recording() {
        let mut h = Harness::with_second_key();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert!(
            h.down(CTRL_R, 300).is_empty(),
            "the other trigger's events are ignored while one recording is live"
        );
        assert!(h.up(CTRL_R, 400).is_empty());
        assert_eq!(h.release_chord(2_000), vec![Action::StopRecording]);
    }

    #[test]
    fn second_key_is_hold_semantics_even_in_toggle_mode() {
        let mut h = Harness::new(HotkeyConfig {
            mode: Mode::Toggle,
            second_key: Some(ModifierOnly::RIGHT_CTRL),
            ..HotkeyConfig::default()
        });
        assert_eq!(h.down(CTRL_R, 0), vec![Action::StartRecording]);
        assert_eq!(
            h.up(CTRL_R, 100),
            vec![Action::DiscardRecording, Action::OpenPanelIdle],
            "a lone modifier never latches, whatever the mode"
        );
    }

    // ---- robustness ------------------------------------------------------------------

    #[test]
    fn auto_repeat_does_not_restart_the_recording() {
        let mut h = Harness::hold_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        // Windows repeats WM_KEYDOWN for a held space bar roughly every 30 ms.
        for repeat in 1..=10 {
            assert!(
                h.down(SPACE, 500 + repeat * 30).is_empty(),
                "repeat {repeat} restarted the recording"
            );
            assert!(h.down(CTRL_L, 500 + repeat * 30).is_empty());
        }
        assert_eq!(h.release_chord(2_000), vec![Action::StopRecording]);
    }

    #[test]
    fn an_up_without_a_down_is_ignored() {
        let mut h = Harness::with_second_key();
        assert!(h.up(SPACE, 0).is_empty());
        assert!(h.up(CTRL_L, 10).is_empty());
        assert!(h.up(CTRL_R, 20).is_empty());
        assert!(!h.machine.is_recording());
        // And the machine is still usable afterwards.
        assert_eq!(h.press_chord(100), vec![Action::StartRecording]);
    }

    #[test]
    fn reset_on_focus_loss_ends_a_live_recording() {
        let mut h = Harness::hold_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert_eq!(
            h.reset(4_000),
            vec![Action::StopRecording],
            "focus went away mid-sentence; what was said is kept"
        );
        assert!(!h.machine.is_recording());
        // The key-up that focus loss swallowed arrives at nothing.
        assert!(h.up(SPACE, 4_100).is_empty());
        // The trigger still works.
        assert_eq!(h.press_chord(5_000), vec![Action::StartRecording]);
    }

    #[test]
    fn reset_during_a_press_too_short_to_be_a_sentence_discards_it() {
        let mut h = Harness::hold_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert_eq!(
            h.reset(100),
            vec![Action::DiscardRecording, Action::OpenPanelIdle],
            "a reset ends the press exactly as a release at that instant would"
        );
    }

    #[test]
    fn reset_leaves_a_toggle_latch_alone() {
        let mut h = Harness::toggle_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert!(h.release_chord(80).is_empty());
        assert!(
            h.reset(200).is_empty(),
            "tap to start, then switch to the window you mean to dictate into"
        );
        assert!(h.machine.is_recording());
        assert_eq!(h.press_chord(6_000), vec![Action::StopRecording]);
    }

    #[test]
    fn the_safety_timeout_stops_a_press_whose_release_never_came() {
        let mut h = Harness::hold_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert!(
            h.tick(120_000).is_empty(),
            "two minutes in, still recording"
        );
        assert_eq!(
            h.tick(300_000),
            vec![Action::StopRecording],
            "max_hold_ms is the ceiling of the recording cap of §7"
        );
        assert!(!h.machine.is_recording());
        assert!(h.tick(400_000).is_empty(), "and it fires only once");

        // The key is still physically down; auto-repeat must not start a second recording.
        assert!(h.down(SPACE, 400_100).is_empty());
        assert!(h.up(SPACE, 500_000).is_empty());
        // Once the chord is broken and pressed again, the trigger is live.
        assert_eq!(h.press_chord(600_000), vec![Action::StartRecording]);
    }

    #[test]
    fn the_safety_timeout_also_ends_a_stuck_second_key() {
        let mut h = Harness::with_second_key();
        assert_eq!(h.down(CTRL_R, 0), vec![Action::StartRecording]);
        assert_eq!(h.tick(300_001), vec![Action::StopRecording]);
        assert!(h.up(CTRL_R, 310_000).is_empty());
        assert_eq!(h.down(CTRL_R, 320_000), vec![Action::StartRecording]);
    }

    #[test]
    fn the_safety_timeout_reaches_a_toggle_latch_too() {
        let mut h = Harness::toggle_mode();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert!(h.release_chord(60).is_empty());
        assert_eq!(
            h.tick(300_000),
            vec![Action::StopRecording],
            "a forgotten latch is exactly what the ceiling is for"
        );
    }

    // ---- chord matching ---------------------------------------------------------------

    #[test]
    fn modifier_order_and_side_do_not_matter() {
        // Six orders, both sides, all of them the same chord.
        let orders: [[Key; 3]; 6] = [
            [CTRL_L, ALT_L, SPACE],
            [ALT_L, CTRL_L, SPACE],
            [CTRL_R, SPACE, ALT_R],
            [SPACE, ALT_R, CTRL_R],
            [ALT_R, SPACE, CTRL_L],
            [SPACE, CTRL_R, ALT_L],
        ];
        for order in orders {
            let mut h = Harness::hold_mode();
            let mut actions = Vec::new();
            let mut when = 0_u64;
            for key in order {
                actions = h.down(key, when);
                when += 10;
            }
            assert_eq!(
                actions,
                vec![Action::StartRecording],
                "{order:?} should complete the chord on its last key"
            );
            // Lifting any one of the three ends it.
            assert_eq!(h.up(order[0], 2_000), vec![Action::StopRecording]);
        }
    }

    #[test]
    fn ctrl_space_without_alt_never_triggers() {
        let mut h = Harness::hold_mode();
        assert!(h.down(CTRL_L, 0).is_empty());
        assert!(
            h.down(SPACE, 50).is_empty(),
            "Ctrl+Space is excluded permanently: the IME swallows it and dikte owns it"
        );
        assert!(h.up(SPACE, 2_000).is_empty());
        assert!(h.up(CTRL_L, 2_010).is_empty());
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn a_modifier_the_chord_does_not_ask_for_does_not_block_it() {
        // Ctrl+Alt+Shift+Space still contains Ctrl+Alt+Space. The chord is a requirement,
        // not an exact match — a user holding Shift for the next capital letter still
        // dictates.
        let mut h = Harness::hold_mode();
        assert!(h.down(SHIFT_L, 0).is_empty());
        assert!(h.down(CTRL_L, 10).is_empty());
        assert!(h.down(ALT_L, 20).is_empty());
        assert_eq!(h.down(SPACE, 30), vec![Action::StartRecording]);
        assert_eq!(h.up(SPACE, 2_000), vec![Action::StopRecording]);
    }

    #[test]
    fn a_chord_on_a_letter_key_works_the_same_way() {
        let mut h = Harness::new(HotkeyConfig {
            primary: Chord::new(
                &[ModifierFamily::Ctrl, ModifierFamily::Shift],
                MainKey::Letter('D'),
            ),
            ..HotkeyConfig::default()
        });
        let d = Key::Main(MainKey::Letter('d'));
        assert!(h.down(CTRL_L, 0).is_empty());
        assert!(h.down(SHIFT_L, 10).is_empty());
        assert_eq!(h.down(d, 20), vec![Action::StartRecording]);
        assert_eq!(h.up(d, 1_500), vec![Action::StopRecording]);
    }

    #[test]
    fn a_named_key_that_belongs_to_no_trigger_is_treated_as_any_other_key() {
        let mut h = Harness::with_second_key();
        assert_eq!(h.down(CTRL_R, 0), vec![Action::StartRecording]);
        assert_eq!(
            h.down(Key::Main(MainKey::Letter('c')), 50),
            vec![Action::DiscardRecording],
            "the adapter should have sent OtherKeyDown; the machine is not fooled either way"
        );
    }

    // ---- the panel's three keys -------------------------------------------------------

    #[test]
    fn the_panel_keys_mean_nothing_until_the_panel_is_up() {
        let mut h = Harness::hold_mode();
        assert!(!h.machine.panel_keys());

        for key in [PanelKey::Transfer, PanelKey::Cancel, PanelKey::Copy] {
            assert!(
                h.panel_key(key, 0).is_empty(),
                "{key:?} reached the machine with no panel to act on"
            );
        }

        h.machine.set_panel_keys(true);
        assert_eq!(
            h.panel_key(PanelKey::Transfer, 10),
            vec![Action::PanelTransfer]
        );
        assert_eq!(h.panel_key(PanelKey::Cancel, 20), vec![Action::PanelCancel]);
        assert_eq!(h.panel_key(PanelKey::Copy, 30), vec![Action::PanelCopy]);

        h.machine.set_panel_keys(false);
        assert!(h.panel_key(PanelKey::Cancel, 40).is_empty());
    }

    #[test]
    fn a_panel_key_withdraws_a_second_key_press_the_way_any_other_key_does() {
        let mut h = Harness::with_second_key();
        h.machine.set_panel_keys(true);

        assert_eq!(h.down(CTRL_R, 0), vec![Action::StartRecording]);
        assert_eq!(
            h.panel_key(PanelKey::Copy, 60),
            vec![Action::DiscardRecording, Action::PanelCopy],
            "the C of Ctrl+C is still a key going down during a lone-modifier hold"
        );
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn a_panel_key_leaves_a_chord_recording_alone() {
        let mut h = Harness::hold_mode();
        h.machine.set_panel_keys(true);

        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert_eq!(
            h.panel_key(PanelKey::Cancel, 400),
            vec![Action::PanelCancel],
            "Esc during a recording is the panel's cancel, not a withdrawal"
        );
        assert!(
            h.machine.is_recording(),
            "the session owns what cancel means"
        );
        assert_eq!(h.release_chord(2_000), vec![Action::StopRecording]);
    }

    // ---- re-record ---------------------------------------------------------------------

    #[test]
    fn re_record_starts_a_latched_recording_in_toggle_mode_and_the_next_tap_stops_it() {
        let mut h = Harness::toggle_mode();
        assert_eq!(h.rerecord(0), vec![Action::StartRecording]);
        assert!(h.machine.is_recording());

        // Exactly as if the user had tapped: the next tap of the chord ends it.
        assert_eq!(h.press_chord(3_000), vec![Action::StopRecording]);
        assert!(h.release_chord(3_060).is_empty());
        assert!(!h.machine.is_recording());
    }

    #[test]
    fn re_record_does_nothing_in_hold_mode_or_while_something_is_already_recording() {
        let mut h = Harness::hold_mode();
        assert!(
            h.rerecord(0).is_empty(),
            "a hold-to-talk recording nobody is pressing would have no end"
        );
        assert!(!h.machine.is_recording());

        let mut toggling = Harness::toggle_mode();
        assert_eq!(toggling.press_chord(0), vec![Action::StartRecording]);
        assert!(toggling.release_chord(60).is_empty());
        assert!(toggling.rerecord(500).is_empty(), "nothing starts twice");
    }

    #[test]
    fn other_keys_do_not_disturb_a_chord_recording() {
        let mut h = Harness::with_second_key();
        assert_eq!(h.press_chord(0), vec![Action::StartRecording]);
        assert!(
            h.other_key(500).is_empty(),
            "the withdrawal rule belongs to the lone modifier, not to an explicit chord"
        );
        assert_eq!(h.release_chord(2_000), vec![Action::StopRecording]);
    }
}
