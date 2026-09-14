//! Watch the trigger work, with a timestamp on every decision.
//!
//! This is how WP2's hotkey half is checked by hand, and how `docs/PROJECT.md` §3's
//! "re-verified against IME in WP2" actually gets verified: the probe prints the keyboard
//! layout of the focused window next to every action, so switching to the Turkish layout and
//! pressing the chord answers the question on one line.
//!
//! ```text
//! cargo run -p dile-hotkey --example hotkey_probe -- --seconds 60
//! cargo run -p dile-hotkey --example hotkey_probe -- --toggle --trigger Ctrl+Alt+Space
//! ```
//!
//! What to try with the default trigger — the right Ctrl, on its own — and what should appear:
//!
//! | Press | Expected |
//! |---|---|
//! | Right Ctrl, held a second, released | `StartRecording` then `StopRecording` |
//! | Right Ctrl, tapped | `StartRecording` then `DiscardRecording` + `OpenPanelIdle` |
//! | Right Ctrl then `C` | `StartRecording` then `DiscardRecording`, and the copy still works |
//! | Left Ctrl, held | nothing at all — a lone trigger is one physical key |
//!
//! And with `--trigger Ctrl+Alt+Space`:
//!
//! | Press | Expected |
//! |---|---|
//! | `Ctrl+Alt+Space`, held a second, released | `StartRecording` then `StopRecording` |
//! | `Ctrl+Alt+Space`, tapped | `StartRecording` then `DiscardRecording` + `OpenPanelIdle` |
//! | `Ctrl+Alt+Space` on a Turkish layout | the same, with `layout=041F` on the line |
//! | `Ctrl+Space` (no Alt) | nothing at all — that chord is excluded permanently |
//!
//! **A chord trigger is swallowed while the probe runs.** The listener installs the hook in
//! blocking mode with that chord in it, so `Ctrl+Alt+Space` never reaches the focused window —
//! that is the behaviour being checked, and a space that still lands in the editor is the
//! failure. A lone-modifier trigger is never blocked, so `Ctrl+C` keeps working throughout.
//!
//! It exits on its own after `--seconds` so that an unattended run cannot leave a global
//! keyboard hook installed. Ctrl+C also works, but it is a hard exit: the hook is then
//! removed by Windows at process teardown rather than by the listener's `Drop`.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("hotkey_probe needs a Windows keyboard hook; there is no adapter for this OS yet.");
}

#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use dile_hotkey::{HotkeyConfig, HotkeyListener, Mode};

    let options = Options::parse(std::env::args().skip(1))?;

    let config = HotkeyConfig {
        mode: if options.toggle {
            Mode::Toggle
        } else {
            Mode::Hold
        },
        trigger: options.trigger,
        ..HotkeyConfig::default()
    };

    println!("dile-hotkey {} probe", dile_hotkey::version());
    println!(
        "  mode              {:?}\n  trigger           {} ({})",
        config.mode,
        config.trigger,
        if config.trigger.is_lone_key() {
            "one key, never swallowed"
        } else {
            "a chord, swallowed while this runs"
        }
    );
    println!(
        "  press threshold   {} ms\n  hold takeover     {} ms (toggle only)\n  safety ceiling    {} ms",
        config.press_threshold_ms, config.hold_takeover_ms, config.max_hold_ms
    );
    println!("  layout now        {}", describe_layout());
    println!("  running for       {} s\n", options.seconds);

    let listener = HotkeyListener::spawn(config)?;
    println!("hook installed. press the trigger.\n");

    let started = Instant::now();
    let deadline = started + Duration::from_secs(options.seconds);
    let mut seen = 0_u32;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match listener
            .actions()
            .recv_timeout(remaining.min(Duration::from_millis(200)))
        {
            Ok(emitted) => {
                seen += 1;
                println!(
                    "[{:>8.3}s] {:<17} layout={}",
                    emitted.at.saturating_duration_since(started).as_secs_f64(),
                    format!("{:?}", emitted.action),
                    describe_layout()
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("the listener thread ended early");
                break;
            }
        }
    }

    println!(
        "\n{seen} action(s) in {} s. removing the hook.",
        options.seconds
    );
    drop(listener);
    Ok(())
}

/// The arguments, and nothing clever about them.
#[cfg(target_os = "windows")]
struct Options {
    seconds: u64,
    toggle: bool,
    trigger: dile_hotkey::Trigger,
}

#[cfg(target_os = "windows")]
impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            seconds: 30,
            toggle: false,
            // The shipped default, so that running the probe with no arguments checks what
            // a user actually has.
            trigger: dile_hotkey::Trigger::RIGHT_CTRL,
        };
        let mut args = args.peekable();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--seconds" => {
                    let value = args.next().ok_or("--seconds needs a number")?;
                    options.seconds = value
                        .parse()
                        .map_err(|_| format!("--seconds {value}: not a number"))?;
                }
                "--toggle" => options.toggle = true,
                "--trigger" => {
                    let value = args.next().ok_or("--trigger needs a key or a chord")?;
                    options.trigger = value
                        .parse()
                        .map_err(|error| format!("--trigger {value}: {error}"))?;
                }
                "--help" | "-h" => {
                    println!("hotkey_probe [--seconds N] [--toggle] [--trigger RightCtrl]");
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(options)
    }
}

/// The keyboard layout of the window that currently has focus, as its hex language id.
///
/// `0409` is US English, `041F` is Turkish, `041F` with a Q or F layout alike — the language
/// id is what decides whether an IME sits between the keyboard and the application, which is
/// the question §3 asks about `Ctrl+Space` and answers about `Ctrl+Alt+Space`.
#[cfg(target_os = "windows")]
fn describe_layout() -> String {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    // SAFETY: three read-only Win32 calls that take no pointer from this program and can
    // only fail by returning a null handle, which the arithmetic below tolerates.
    let layout = unsafe {
        let window = GetForegroundWindow();
        let thread = GetWindowThreadProcessId(window, None);
        GetKeyboardLayout(thread)
    };
    // The low word of an HKL is the language identifier; the high word is the physical
    // layout, which is not what decides whether an IME is in the way.
    let language = (layout.0 as usize) & 0xFFFF;
    match language {
        0x041F => format!("{language:04X} (Turkish)"),
        0x0409 => format!("{language:04X} (US English)"),
        0 => "unknown".to_owned(),
        other => format!("{other:04X}"),
    }
}
