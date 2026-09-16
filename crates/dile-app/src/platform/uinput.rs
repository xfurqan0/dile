//! The paste chord on Linux: a virtual keyboard, and the promise it deliberately does not
//! make.
//!
//! This is D-WP-L4, and it is the *optional* half of a decision `docs/PROJECT.md` §9 already
//! took. A dictation is handed over on the clipboard here, because a Wayland client is never
//! told which window has the focus and a paste therefore cannot be aimed. Synthesising
//! `Ctrl+V` was always the easy half — the trigger asks for `/dev/uinput` anyway, so the
//! chord costs no permission that has not already been asked for — and this module is that
//! easy half, behind a setting that is **off by default** and labelled experimental.
//!
//! ## What it is not
//!
//! It is not [`super::unported::window::send_paste_chord`], and the difference is the whole
//! reason there are two things. That function means *type the paste chord into the window
//! this dictation was aimed at*, which is what `paste.rs` calls after it has checked the
//! target is still there; it answers `false` on Linux and it will keep answering `false`,
//! because the window it is about cannot be named here. This one means *press `Ctrl+V` at
//! whatever holds the keyboard right now*, which is a different and smaller thing, and the
//! person who turns it on is told so in the settings window.
//!
//! ## The order this has to happen in, which is not obvious
//!
//! The card has the keyboard while it is on screen — that is [`super::linux`]'s whole subject,
//! and it is what lets the clipboard be written at all. So a chord sent while the card is up
//! goes **into the card**. The panel therefore hides it first, waits for the compositor to
//! hand the focus back to whatever had it, and only then asks for the chord. `panel.rs`'s
//! `hand_over` is where that sequence lives, with the delay it spends named there.
//!
//! ## Why the device is kept open
//!
//! Creating a uinput device is not the same as the compositor being ready to read from it:
//! the kernel announces a new input node, udev applies its rules to it, and only then does
//! libinput open it. A chord written into the gap goes nowhere, silently. So the device is
//! created once, [`DEVICE_SETTLE`] is paid once, and every later chord is immediate — and
//! **it disappears with the process**, because the kernel destroys a uinput device when the
//! last handle on it closes. There is nothing to clean up after a crash.

use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, KeyCode, KeyEvent};

/// The node whose permission decides whether any of this is possible at all.
const UINPUT: &str = "/dev/uinput";

/// What this keyboard is called, in `libinput list-devices` and in the kernel's device list.
///
/// Named rather than anonymous on purpose: a person who finds an unexplained keyboard in that
/// list should be able to tell whose it is, and a person who turned this setting on should be
/// able to find it.
const DEVICE_NAME: &str = "Dile virtual keyboard";

/// How long a newly created device is left alone before the first chord is written to it.
///
/// Paid once per process, on the first auto-paste. See the module documentation: a chord
/// written before libinput has opened the node is not slow, it is lost.
const DEVICE_SETTLE: Duration = Duration::from_millis(400);

/// How long each half of the chord is held, and the gap between the presses.
///
/// Four events with a pause between them rather than one batch, because that is what a
/// keyboard looks like: a toolkit that reads the modifier state when the key arrives has to
/// have seen the modifier first.
const CHORD_GAP: Duration = Duration::from_millis(12);

/// Why there is no chord.
///
/// Every variant is a different thing to do about it, which is why they are not one error
/// with a string in it: a missing node means the kernel module is not loaded, a refused one
/// means the udev rule, and anything else is the system's own complaint repeated verbatim.
#[derive(Debug, thiserror::Error)]
pub enum AutoPasteError {
    /// There is no `/dev/uinput` at all — no `uinput` module, or a container without it.
    #[error(
        "there is no {UINPUT} on this machine, so no key press can be synthesised; the uinput kernel module is what provides it"
    )]
    NoDevice,

    /// The node is there and this user may not write to it.
    ///
    /// The same permission the trigger needs, and the same rule opens it — which is why a
    /// machine where the trigger works is a machine where this works.
    #[error(
        "{UINPUT} is there and this user may not write to it; packaging/linux/70-dile-input.rules is the rule that opens it, and it is the same one the trigger needs"
    )]
    NoPermission,

    /// The device could not be created, for a reason that is neither of the above.
    #[error("the virtual keyboard could not be created: {0}")]
    Refused(String),

    /// The device exists and the chord would not go to it.
    #[error("the paste chord could not be written to the virtual keyboard: {0}")]
    NotWritten(String),

    /// Something else is already holding the one keyboard this process makes.
    ///
    /// A lock is poisoned when a thread panicked while holding it. Reported rather than
    /// papered over: the dictation is on the clipboard either way.
    #[error("the virtual keyboard is not usable, because a thread panicked while holding it")]
    Unusable,
}

impl AutoPasteError {
    /// The locale key of the line the card shows about this.
    ///
    /// A key rather than this error's own sentence, for the rule every visible word in this
    /// product follows: the sentence above is a diagnostic for a log, and the one on the card
    /// comes out of `locales/`. The two say the same thing at different lengths.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            AutoPasteError::NoDevice => "panel.state.autopaste.nodevice",
            AutoPasteError::NoPermission => "panel.state.autopaste.nopermission",
            AutoPasteError::Refused(_)
            | AutoPasteError::NotWritten(_)
            | AutoPasteError::Unusable => super::AUTO_PASTE_FAILED,
        }
    }
}

/// What a failure to open `/dev/uinput` actually means.
///
/// **A pure function of two facts**, for the reason `dile-hotkey`'s `decide` is one: the
/// machine that runs the tests has a working uinput and no permission problem to reproduce,
/// and the sentence a person reads is worth checking on every machine rather than on the one
/// machine that can produce it. `exists` is passed in rather than read here for the same
/// reason.
fn classify(exists: bool, error: &io::Error) -> AutoPasteError {
    if !exists {
        // Distinguished from a permission failure because the answer is different: no rule
        // will help, the module has to be loaded. `ENOENT` from the open would say the same
        // thing, but only if the node was there a moment ago and went away.
        return AutoPasteError::NoDevice;
    }
    match error.kind() {
        io::ErrorKind::PermissionDenied => AutoPasteError::NoPermission,
        // Not folded into `NoDevice`: the node was there when it was looked at, so this is a
        // device that vanished between the two, which is worth reading as what it is.
        _ => AutoPasteError::Refused(error.to_string()),
    }
}

/// A keyboard this application makes, which can press exactly two keys.
///
/// `Ctrl` and `V`, and nothing else is registered on the device — a virtual keyboard that
/// could type anything would be a larger thing to have running than the one job it is here
/// for, and the capability bits are what a person inspecting it would read.
#[derive(Debug)]
pub struct AutoPaste {
    /// `evdev`'s handle. Behind a lock because writing to it takes `&mut`, and the panel
    /// holds one of these behind an `Arc` shared by every thread that can finish a dictation.
    device: Mutex<VirtualDevice>,
}

impl AutoPaste {
    /// Create the virtual keyboard, and wait for the system to notice it.
    ///
    /// **Blocks for [`DEVICE_SETTLE`]**, once, which is why the caller keeps the result.
    ///
    /// # Errors
    ///
    /// [`AutoPasteError::NoDevice`] when there is no `/dev/uinput`,
    /// [`AutoPasteError::NoPermission`] when this user may not write to it, and
    /// [`AutoPasteError::Refused`] for anything else the kernel says.
    pub fn open() -> Result<AutoPaste, AutoPasteError> {
        let exists = Path::new(UINPUT).exists();
        let keys = AttributeSet::from_iter([KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V]);

        let device = VirtualDevice::builder()
            .map_err(|error| classify(exists, &error))?
            .name(DEVICE_NAME)
            .with_keys(&keys)
            .map_err(|error| classify(exists, &error))?
            .build()
            .map_err(|error| classify(exists, &error))?;

        log::info!("a virtual keyboard was created for auto-paste, named {DEVICE_NAME:?}");
        sleep(DEVICE_SETTLE);

        Ok(AutoPaste {
            device: Mutex::new(device),
        })
    }

    /// Press `Ctrl+V` at whatever holds the keyboard.
    ///
    /// **It does not say where that went**, and it cannot: nothing in a Wayland session will
    /// tell a client which window has the focus. A successful return means the four events
    /// reached the kernel.
    ///
    /// # Errors
    ///
    /// [`AutoPasteError::NotWritten`] when the device would not take the events, and
    /// [`AutoPasteError::Unusable`] when a thread panicked holding the lock.
    pub fn press_paste(&self) -> Result<(), AutoPasteError> {
        let mut device = self.device.lock().map_err(|_| AutoPasteError::Unusable)?;

        // Down and up, in the order a hand does it, each one its own report. The releases are
        // not optional and not a tidiness: a Ctrl left down by a program nobody can see is
        // the worst failure this module could have.
        let chord = [
            (KeyCode::KEY_LEFTCTRL, 1),
            (KeyCode::KEY_V, 1),
            (KeyCode::KEY_V, 0),
            (KeyCode::KEY_LEFTCTRL, 0),
        ];
        for (key, value) in chord {
            device
                .emit(&[*KeyEvent::new(key, value)])
                .map_err(|error| AutoPasteError::NotWritten(error.to_string()))?;
            sleep(CHORD_GAP);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoPasteError, classify};
    use std::io;

    #[test]
    fn a_machine_with_no_uinput_is_not_told_to_install_a_udev_rule() {
        // The wrong answer here costs somebody an hour: the rule cannot open a node that is
        // not there, and the sentence has to say the module rather than the rule.
        let missing = classify(
            false,
            &io::Error::new(io::ErrorKind::NotFound, "No such file or directory"),
        );
        assert!(matches!(missing, AutoPasteError::NoDevice));
        assert!(missing.to_string().contains("uinput kernel module"));

        // And the same error kind with the node present is a node that went away, not a
        // machine without one.
        let vanished = classify(
            true,
            &io::Error::new(io::ErrorKind::NotFound, "No such file or directory"),
        );
        assert!(matches!(vanished, AutoPasteError::Refused(_)));
    }

    #[test]
    fn a_refused_write_names_the_rule_that_opens_it() {
        let refused = classify(
            true,
            &io::Error::new(io::ErrorKind::PermissionDenied, "Permission denied"),
        );
        assert!(matches!(refused, AutoPasteError::NoPermission));
        let sentence = refused.to_string();
        assert!(sentence.contains("70-dile-input.rules"), "{sentence}");
        // The one thing a person needs to know next: this is the permission they may already
        // have, because the trigger needed it first.
        assert!(sentence.contains("trigger"), "{sentence}");
    }

    #[test]
    fn every_reason_carries_a_line_the_card_can_show() {
        // The card shows one line and it has to be a locale key, because a sentence typed
        // into Rust is what crates/dile-app/tests/i18n.rs fails the build on.
        for (error, key) in [
            (AutoPasteError::NoDevice, "panel.state.autopaste.nodevice"),
            (
                AutoPasteError::NoPermission,
                "panel.state.autopaste.nopermission",
            ),
            (
                AutoPasteError::Refused("mmap".to_owned()),
                "panel.state.autopaste.failed",
            ),
            (
                AutoPasteError::NotWritten("EIO".to_owned()),
                "panel.state.autopaste.failed",
            ),
            (AutoPasteError::Unusable, "panel.state.autopaste.failed"),
        ] {
            assert_eq!(error.key(), key, "{error}");
            assert!(error.key().starts_with("panel.state.autopaste."));
        }
    }
}
