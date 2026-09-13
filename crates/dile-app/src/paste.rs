//! Where a finished dictation actually goes.
//!
//! Four decisions live here and no Win32 call does — every one of those is in
//! [`crate::win32`], which is why the tables below are ordinary unit tests on a machine with
//! no clipboard and no focused window:
//!
//! 1. **What to call the target.** `Code.exe` is "VS Code" on the panel, because "→ Code.exe"
//!    tells a person nothing they did not already know and "→ VS Code" tells them where the
//!    next 1.5 s is going to put their sentence. An application nobody has written a name for
//!    keeps its own file name, which is always better than a blank.
//! 2. **Which chord to send.** `Ctrl+V` everywhere, `Ctrl+Shift+V` in the terminal family,
//!    because a terminal reads `Ctrl+V` as a literal-next escape or as nothing at all.
//! 3. **Whether to paste at all.** The target window is captured when the recording *starts*
//!    and checked again when the text is ready. A window that has gone takes the paste with
//!    it: the text stays in the panel and the user is told, rather than a sentence landing in
//!    whatever inherited the focus.
//! 4. **What to put back.** The user's own clipboard, as text, after the paste has been taken
//!    — which is possible at all because the offer is a delayed render with a receipt rather
//!    than a write and a guess. See [`crate::win32::clipboard`] for the mechanism and for
//!    what a v1 restore does *not* cover.
//!
//! **Copy is not paste.** The panel's copy button writes the clipboard outright and restores
//! nothing: the user asked for their clipboard to hold this. `docs/PROJECT.md` §3 says so in
//! as many words, and it is the one place the two paths deliberately differ.

use std::path::Path;
use std::time::Duration;

use crate::win32::{Clipboard, ClipboardError, Hwnd, window};

/// How long the transfer waits for the target to actually take the text.
///
/// Not a delay before pasting — the chord goes out immediately, and this is how long the
/// receipt is waited on afterwards. Two seconds is long enough for a loaded Electron window
/// to get round to its paste handler and short enough that a target which never pastes does
/// not hold the user's clipboard hostage.
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(2);

/// Applications that read `Ctrl+Shift+V` as paste, by executable stem, lower-cased.
///
/// The terminal family, and it is a family rather than a guess: in all of these `Ctrl+V`
/// means something else or nothing at all, and every one of them has had `Ctrl+Shift+V` as
/// paste since it shipped. A user with a terminal that is not on this list gets `Ctrl+V`,
/// which is the right default for the other several hundred programs on a machine; a setting
/// that lets them override it is a later package's job and not a reason to guess more widely
/// now.
const SHIFT_PASTE: [&str; 7] = [
    "windowsterminal",
    "openconsole",
    "conhost",
    "mintty",
    "alacritty",
    "wezterm-gui",
    "hyper",
];

/// What to call an application on the panel, by executable stem, lower-cased.
///
/// Brands, not translations: `locales/` carries the sentence the label sits in
/// (`panel.target`, "→ {app}") and this carries the name, which is the same word in every
/// language. The list is short on purpose — it covers what a person dictating into Windows
/// is most likely to be dictating into, and everything else falls back to the file name.
const FRIENDLY_NAMES: [(&str, &str); 25] = [
    ("code", "VS Code"),
    ("cursor", "Cursor"),
    ("devenv", "Visual Studio"),
    ("idea64", "IntelliJ"),
    ("pycharm64", "PyCharm"),
    ("rider64", "Rider"),
    ("sublime_text", "Sublime"),
    ("zed", "Zed"),
    ("windowsterminal", "Windows Terminal"),
    ("openconsole", "Terminal"),
    ("conhost", "Terminal"),
    ("powershell", "PowerShell"),
    ("pwsh", "PowerShell"),
    ("wezterm-gui", "WezTerm"),
    ("alacritty", "Alacritty"),
    ("mintty", "mintty"),
    ("hyper", "Hyper"),
    ("chrome", "Chrome"),
    ("msedge", "Edge"),
    ("firefox", "Firefox"),
    ("notepad", "Notepad"),
    ("notepad++", "Notepad++"),
    ("obsidian", "Obsidian"),
    ("slack", "Slack"),
    ("discord", "Discord"),
];

/// What happened to one transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The chord was sent and the target took the text off the clipboard.
    Pasted,
    /// The chord was sent and the receipt never came back inside [`RECEIPT_TIMEOUT`].
    ///
    /// Not necessarily a failure — a target can be slow, or the user may have switched away
    /// mid-paste — but the clipboard is restored anyway, and a later paste in that window
    /// would get whatever was there before rather than the dictation.
    Unclaimed,
    /// The window the recording was aimed at is gone, or would not come back to the front.
    ///
    /// Nothing was pasted and nothing was put on the clipboard. The panel keeps the text.
    TargetGone,
    /// The clipboard itself refused.
    Refused,
}

/// The executable's name without its extension, lower-cased.
///
/// `C:\Users\…\Code.exe` becomes `code`. Lower-cased because Windows file names are, and
/// because half the programs on a machine spell their own executable differently from their
/// own installer.
#[must_use]
pub fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// What the panel calls this application.
#[must_use]
pub fn label_for(path: &Path) -> String {
    let stem = stem_of(path);
    FRIENDLY_NAMES
        .iter()
        .find(|(exe, _)| *exe == stem)
        .map_or_else(
            || {
                // The file name as it is actually spelled, not the lower-cased key: an
                // unknown program is better introduced by its own capitalisation.
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            |(_, name)| (*name).to_owned(),
        )
}

/// Whether this application wants `Ctrl+Shift+V` rather than `Ctrl+V`.
#[must_use]
pub fn wants_shift(path: &Path) -> bool {
    let stem = stem_of(path);
    SHIFT_PASTE.contains(&stem.as_str())
}

/// The clipboard, and the three things this application does with it.
#[derive(Debug)]
pub struct Paster {
    clipboard: Clipboard,
}

impl Paster {
    /// Start the clipboard agent.
    ///
    /// # Errors
    ///
    /// The agent thread or its hidden owner window could not be created. The application
    /// still runs: a dictation can be read in the panel and copied by hand.
    pub fn start() -> Result<Paster, ClipboardError> {
        Ok(Paster {
            clipboard: Clipboard::start()?,
        })
    }

    /// What the clipboard holds as text right now.
    ///
    /// **Test builds only.** The hand test below reads the clipboard twice — before a paste
    /// and after it — because "the previous clipboard is restored" is a claim about the
    /// machine rather than about this code, and the only way to check a claim about the
    /// machine is to ask the machine.
    #[cfg(test)]
    #[must_use]
    pub fn peek(&self) -> Option<String> {
        self.clipboard.text()
    }

    /// Put text on the clipboard and leave it there.
    ///
    /// The panel's copy button. No delayed render, no receipt, and **no restore**.
    ///
    /// # Errors
    ///
    /// Windows would not give up the clipboard.
    pub fn copy(&self, text: &str) -> Result<(), ClipboardError> {
        self.clipboard.set_text(text)
    }

    /// Paste `text` into `target`, then give the clipboard back.
    ///
    /// In order: read what is on the clipboard; make sure the target is there and in front;
    /// offer the text as a delayed render; send the chord; wait for the receipt; restore.
    /// The target check comes **before** the clipboard is touched, so a window that has gone
    /// costs the user nothing at all.
    pub fn transfer(&self, target: Hwnd, exe: &Path, text: &str) -> Outcome {
        if !target.exists() {
            log::warn!("the window this dictation was aimed at is gone; nothing was pasted");
            return Outcome::TargetGone;
        }
        if !target.focus() {
            log::warn!(
                "the window this dictation was aimed at would not come back to the front; nothing was pasted"
            );
            return Outcome::TargetGone;
        }

        let previous = self.clipboard.text();
        let receipt = match self.clipboard.offer(text) {
            Ok(receipt) => receipt,
            Err(error) => {
                log::error!("the clipboard would not take the dictation: {error}");
                return Outcome::Refused;
            }
        };

        let shift = wants_shift(exe);
        if !window::send_paste_chord(shift) {
            log::error!("the paste chord could not be sent");
            let _ = self.clipboard.restore(previous);
            return Outcome::Refused;
        }
        log::info!(
            "pasted into {} with {}",
            label_for(exe),
            if shift { "Ctrl+Shift+V" } else { "Ctrl+V" }
        );

        let taken = receipt.wait(RECEIPT_TIMEOUT);
        if !taken {
            log::warn!(
                "the target never asked for the text within {} s; the clipboard is being put back anyway",
                RECEIPT_TIMEOUT.as_secs()
            );
        }

        if let Err(error) = self.clipboard.restore(previous) {
            log::error!("the previous clipboard could not be restored: {error}");
        }

        if taken {
            Outcome::Pasted
        } else {
            Outcome::Unclaimed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FRIENDLY_NAMES, Outcome, Paster, SHIFT_PASTE, label_for, stem_of, wants_shift};
    use std::path::Path;

    #[test]
    fn an_executable_becomes_the_name_a_person_would_use() {
        assert_eq!(
            label_for(Path::new(
                r"C:\Users\someone\AppData\Local\Programs\Code\Code.exe"
            )),
            "VS Code"
        );
        assert_eq!(
            label_for(Path::new(
                r"C:\Program Files\WindowsApps\WindowsTerminal.exe"
            )),
            "Windows Terminal"
        );
        assert_eq!(label_for(Path::new(r"C:\Windows\notepad.exe")), "Notepad");
    }

    #[test]
    fn an_application_nobody_named_keeps_its_own_spelling() {
        // Not lower-cased: the map's keys are, so that the lookup is case-insensitive, but a
        // program Dile has never heard of introduces itself the way it spells itself.
        assert_eq!(
            label_for(Path::new(r"D:\tools\SomeEditor.exe")),
            "SomeEditor"
        );
        assert_eq!(label_for(Path::new("")), "");
        assert_eq!(stem_of(Path::new(r"C:\x\Code.exe")), "code");
    }

    #[test]
    fn the_terminal_family_gets_the_chord_a_terminal_understands() {
        for exe in SHIFT_PASTE {
            let path = format!(r"C:\bin\{exe}.exe");
            assert!(
                wants_shift(Path::new(&path)),
                "{exe} is in the family and must get Ctrl+Shift+V"
            );
        }

        // And nothing else does. Ctrl+Shift+V is "paste without formatting" in a browser and
        // in Word, which would be a different result rather than no result.
        for exe in ["code", "chrome", "msedge", "winword", "notepad", "slack"] {
            let path = format!(r"C:\bin\{exe}.exe");
            assert!(!wants_shift(Path::new(&path)), "{exe} takes Ctrl+V");
        }
    }

    #[test]
    fn every_terminal_in_the_chord_map_can_also_be_named_on_the_panel() {
        // A window whose paste chord is special enough to be listed is a window worth
        // labelling: "→ Ctrl+Shift+V into openconsole" is not a sentence anybody wants to
        // work out from "→ openconsole".
        for exe in SHIFT_PASTE {
            assert!(
                FRIENDLY_NAMES.iter().any(|(key, _)| *key == exe),
                "{exe} has a special chord and no name"
            );
        }
    }

    /// The whole paste path, against a real window, on a real clipboard.
    ///
    /// `#[ignore]` for two reasons, both of them about other people: it takes the machine's
    /// clipboard for a second, and it opens and closes a Notepad on whoever's desktop is
    /// running it. Neither is acceptable in a suite that runs on every commit, and neither is
    /// a reason for the check not to exist — this is the only test in the repository that
    /// proves a dictation actually arrives somewhere. Run it by hand:
    ///
    /// ```text
    /// cargo test -p dile-app --bin dile-app -- --ignored --nocapture notepad
    /// ```
    ///
    /// **The window is found by asking who is in front, not by the process that was
    /// started.** On Windows 11 `notepad.exe` is a stub that hands off to a packaged
    /// application, so enumerating the spawned pid's windows finds nothing at all — which is
    /// exactly the shape of problem this whole module exists to get right, and it is fitting
    /// that the test hit it first.
    #[test]
    #[ignore = "spawns Notepad and takes the machine's clipboard"]
    fn a_dictation_pasted_into_notepad_arrives_and_the_clipboard_comes_back() {
        use crate::win32::{Hwnd, window};
        use std::time::{Duration, Instant};

        let paster = Paster::start().expect("the clipboard agent starts");
        paster
            .copy("dile onceki pano")
            .expect("something to restore");
        assert_eq!(paster.peek().as_deref(), Some("dile onceki pano"));

        let mut stub = std::process::Command::new("notepad.exe")
            .spawn()
            .expect("notepad starts");

        // Its window, when it has one and has come to the front. Spawning is not showing, and
        // a paste sent at a window that does not exist yet is the race this wait loses on
        // purpose.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut found = None;
        while Instant::now() < deadline && found.is_none() {
            found = Hwnd::foreground()
                .map(|front| (front, front.process_path().unwrap_or_default()))
                .filter(|(_, exe)| stem_of(exe) == "notepad");
            if found.is_none() {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        let Some((target, exe)) = found else {
            let _ = stub.kill();
            panic!("notepad never came to the front");
        };
        println!("target: {} hwnd {:#x}", exe.display(), target.as_isize());
        let owner = window::process_id(target);
        // Long enough for the editor inside it to take the caret.
        std::thread::sleep(Duration::from_millis(1_500));

        let outcome = paster.transfer(target, &exe, "dile test");
        std::thread::sleep(Duration::from_millis(800));

        // The clipboard first, because reading the text back is going to overwrite it: the
        // only way to ask a XAML editor what it holds is to select it and copy it, the way a
        // person would. `text_of` is tried first anyway, because a classic edit control
        // answers directly and this test should keep working against one.
        let restored = paster.peek();
        let mut arrived = window::text_of(target);
        if !arrived.contains("dile test") {
            assert!(window::send_select_all_and_copy(), "the read-back chord");
            std::thread::sleep(Duration::from_millis(600));
            arrived = paster.peek().unwrap_or_default();
        }

        let _ = stub.kill();
        let _ = stub.wait();
        if let Some(owner) = owner {
            // By pid, and only by pid.
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &owner.to_string(), "/T", "/F"])
                .output();
        }

        println!("outcome: {outcome:?}");
        println!("notepad said: {arrived:?}");
        println!("clipboard after: {restored:?}");

        assert_eq!(outcome, Outcome::Pasted, "the target never took the text");
        assert!(
            arrived.contains("dile test"),
            "notepad shows {arrived:?}, which does not contain the dictation"
        );
        assert_eq!(
            restored.as_deref(),
            Some("dile onceki pano"),
            "the clipboard was not put back"
        );
    }

    #[test]
    fn the_name_map_has_no_duplicate_keys_and_every_key_is_a_stem() {
        let mut keys: Vec<&str> = FRIENDLY_NAMES.iter().map(|(key, _)| *key).collect();
        let before = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), before, "two entries claim one executable");

        for (key, name) in FRIENDLY_NAMES {
            assert_eq!(key, key.to_lowercase(), "{key} would never match");
            assert!(!key.ends_with(".exe"), "{key} is a file name, not a stem");
            assert!(!name.is_empty());
        }
    }
}
