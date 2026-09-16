//! The review panel: the window a dictation lands in before it lands anywhere else.
//!
//! `docs/PROJECT.md` §3, row "Panel": **Dile does not paste blind.** Every dictation is shown
//! in a 680-pixel card at the top of the screen, for long enough to read it, fix a mangled
//! proper noun or cancel it — and then it goes into whatever the user was typing in.
//!
//! ## The window never takes the focus
//!
//! That is the whole design, and everything else follows from it. The panel is created with
//! `focusable: false`, which on Windows is `WS_EX_NOACTIVATE` — `tao` sets exactly that bit
//! for that flag, so there is no need to reach for the `windows` crate to put it there, and
//! `Hwnd::is_non_activating` is how it gets read back and checked. A window that cannot be
//! activated means the editor behind it keeps the caret, keeps its selection and keeps its
//! focus ring, so the paste at the end goes where the user was already looking. The flag has
//! no equivalent under Wayland at all — `xdg-shell` has no request for "do not activate me",
//! because the compositor alone decides — which is one of the reasons `src/platform` has a
//! module that answers no rather than a second implementation.
//!
//! Three consequences:
//!
//! * **Enter, Esc and `Ctrl+C` come from the global hook**, not from the webview, because the
//!   webview never has the keyboard. `dile-hotkey`'s `panel_keys` mode both reports them and
//!   blocks them while the panel is up — a transfer whose Enter also reached the editor would
//!   paste a sentence *and* a newline.
//! * **Clicking into the text is the one thing that activates the panel.** `panel_take_focus`
//!   lifts `focusable` for as long as the user is editing and puts it back afterwards, and
//!   the target window is restored before anything is pasted.
//! * **The window is placed and resized from Rust.** `capabilities/default.json` grants the
//!   webview `core:window:allow-start-dragging` and nothing else that moves a window: a page
//!   that could show, move or focus its own window could undo the one property this design
//!   rests on.
//!
//! ## Where it appears
//!
//! Top-centre of the monitor that held the **foreground window at the moment the hotkey was
//! pressed** — not the monitor the mouse is on, and not the primary one. A person dictating
//! into an editor on the left-hand screen should not have to look right. Twenty-four pixels
//! below the work area, so it clears a taskbar docked at the top.
//!
//! Dragging it is remembered per monitor, keyed by the display's device name, in
//! `settings.ui.panel_position`. Two screens, two habits, and the one the user is dictating
//! on decides.
//!
//! ## What the webview is told
//!
//! | Event | Carries |
//! |---|---|
//! | `dile://state` | the session's own state — recording, working, nothing heard, no model |
//! | `dile://level` | the meter, twenty times a second, with elapsed and the cap |
//! | `dile://engine` | the tier and the model, for the honest working line |
//! | `dile://result` | the finished dictation: raw, cleaned, target label, strictness |
//! | `dile://panel` | the two transitions the session state cannot express: the idle hint after a tap, and *close* |
//!
//! The settings the panel needs — the chord to name in the hint, whether auto-transfer is on
//! and how long it waits — come from [`commands::panel_context`] rather than from an event,
//! because they are answers to a question rather than news.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dile_core::cleanup::{self, Config as CleanupConfig, Strictness};
use dile_hotkey::Remote;
use serde::Serialize;
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WindowEvent};

use crate::paste::{Outcome, Paster};
use crate::platform::{self, AUTO_PASTE_FAILED, AutoPaste, Delivery, Hwnd, Monitor, window};
use crate::settings::{PanelPosition, SettingsStore};
use crate::ui::Ui;

/// The window label, which is also what `capabilities/default.json` matches on.
pub const WINDOW: &str = "panel";

/// One finished dictation, or one instruction, on its way to the webview.
pub const EVENT_RESULT: &str = "dile://result";

/// The two panel transitions the session's own state cannot express.
pub const EVENT_PANEL: &str = "dile://panel";

/// The card's width, in logical pixels. The mockup's, and it does not change.
const CARD_WIDTH: f64 = 680.0;

/// The card's height in every state but *result*, in logical pixels.
const CARD_BASE: f64 = 64.0;

/// The tallest the card may grow to, in logical pixels.
///
/// Three lines of 14 px text plus the controls, which is the 2026-09-13 decision: unbounded
/// growth is a panel that covers half the screen after a minute of speech, and no growth at
/// all is a review through a keyhole. Past this the text region scrolls inside itself.
const CARD_MAX: f64 = 168.0;

/// Transparent space left around the card on every side, in logical pixels.
///
/// The window is transparent and the card is the only thing drawn in it, with rounded corners
/// and a shadow — and a shadow needs somewhere to fall. Without the gutter the window's edge
/// would clip it, and a floating strip with no shadow reads as a hole cut in the screen
/// rather than as something sitting above it. It is also why the window is wider than the
/// card: 680 is the mockup's measurement of the *card*, and it stays that whatever the window
/// around it is.
const GUTTER: f64 = 14.0;

/// The window, which is the card plus its gutter on both sides.
const WIDTH: f64 = CARD_WIDTH + GUTTER * 2.0;

/// The window's height in every state but *result*.
const BASE_HEIGHT: f64 = CARD_BASE + GUTTER * 2.0;

/// The tallest the window may grow to.
const MAX_HEIGHT: f64 = CARD_MAX + GUTTER * 2.0;

/// How far below the top of the work area the **card's own top edge** sits, in logical pixels.
const TOP_MARGIN: f64 = 24.0;

/// How long the compositor is given to hand the keyboard back after the card is hidden,
/// before an auto-paste chord is sent.
///
/// **The one number in the auto-paste path that is a guess rather than a measurement**, and
/// it cannot be anything else: there is no event a Wayland client can wait for that says
/// *somebody else has the focus now*. A client is told when it gains and loses the keyboard
/// and nothing about who has it instead, which is the same rule that makes the target window
/// unnameable in the first place (`docs/PROJECT.md` §9).
///
/// Measured on GNOME 50.4: the card's `wl_keyboard.leave` and the next surface's `enter`
/// arrive in the same frame, well inside this. Generous rather than tight because the cost of
/// being early is a chord typed into a card that is on its way out, and the cost of being
/// late is that a person sees the paste a tenth of a second after the card goes.
const FOCUS_RETURN: std::time::Duration = std::time::Duration::from_millis(180);

/// What the experimental auto-paste did, which is three different things to the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chord {
    /// The setting is off, or this platform has no way to send one. The card says *copied*.
    NotAsked,
    /// The card has been taken down and the chord was written — or was not, in which case the
    /// log says so. There is nothing left on screen to put a line on either way, and the
    /// dictation is on the clipboard in both.
    Handled,
    /// It was asked for and there is no keyboard to send it with. The card stays up, and this
    /// is the locale key of the line it shows instead of *copied*.
    Unavailable(&'static str),
}

/// What the panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    /// Nothing; the window is hidden.
    #[default]
    Closed,
    /// The hint after a tap too short to be a sentence.
    Idle,
    /// A dictation is in progress, or being transcribed.
    Live,
    /// A finished dictation is on screen, waiting to be transferred.
    Result,
}

/// The window a dictation is aimed at, captured when the recording began.
#[derive(Clone, Debug)]
struct Target {
    handle: Hwnd,
    exe: PathBuf,
    label: String,
}

/// A finished dictation, both ways round.
#[derive(Clone, Debug, Default)]
struct Dictation {
    /// What the engine wrote, before any rule ran. The panel's "raw" pill shows this.
    raw: String,
    /// What the cleanup made of it, and what is transferred unless the user edits it.
    cleaned: String,
}

/// Everything one panel remembers between calls.
#[derive(Debug, Default)]
struct Held {
    mode: Mode,
    target: Option<Target>,
    dictation: Option<Dictation>,
    /// The text as the webview currently has it, which is what a transfer actually sends.
    ///
    /// Kept here rather than asked for at transfer time, because the transfer can be an Enter
    /// that arrived through a global keyboard hook — and asking a webview a question from a
    /// key handler means waiting for a round trip before the paste can start.
    edited: Option<String>,
    /// The strictness this dictation is being shown at, which is per-dictation and is
    /// deliberately **not** written back to the settings.
    strictness: Strictness,
    /// The monitor the card was last placed on, and where the user dragged it to on it.
    ///
    /// The monitor rather than its name, because the position is stored relative to the
    /// display's own corner: a screen that moves in the arrangement takes its panel with it.
    monitor: Option<Monitor>,
    dragged: Option<PanelPosition>,
    /// Where this application last put the window itself.
    ///
    /// Windows reports a programmatic move through the same event as a dragged one, and it
    /// reports it *after* the call returns — so without this the panel would "remember" its
    /// own placement as a preference the first time it was ever shown, and the settings file
    /// would fill up with positions nobody chose.
    placed: Option<PanelPosition>,
    /// Whether this card is a debug preview rather than a dictation.
    ///
    /// Set by `DILE_OPEN_PANEL` and by nothing else. It switches the countdown off, because a
    /// preview that transferred itself after a second and a half would be a preview nobody
    /// could photograph — which is exactly what happened the first time one was tried.
    preview: bool,
}

/// The review panel.
pub struct Panel {
    app: AppHandle,
    store: SettingsStore,
    ui: Ui,
    /// How the panel arms and disarms its three keys. Replaced whenever the session installs
    /// a new hook, which is every time the chord changes.
    remote: Mutex<Option<Remote>>,
    /// The clipboard, or `None` when its agent would not start.
    ///
    /// Not fatal: a panel with no clipboard still shows the text, which is more than the
    /// previous package could do. The transfer says so in the log and the copy button reports
    /// it the same way.
    paster: Option<Paster>,
    /// The virtual keyboard the experimental auto-paste presses `Ctrl+V` on, once something
    /// has asked for one.
    ///
    /// **Opened lazily and then kept**, which is two decisions. Lazily, because a machine
    /// where the setting is off must not have an extra keyboard in its device list for no
    /// reason; kept, because creating one costs a settle window the compositor needs before
    /// it will read from the device at all (`platform::uinput`), and paying that on every
    /// dictation would make the first chord of each one the slow one.
    keyboard: Mutex<Option<AutoPaste>>,
    held: Mutex<Held>,
}

impl Panel {
    /// Build the panel and wire the window's own events.
    #[must_use]
    pub fn new(app: &AppHandle, store: SettingsStore, ui: Ui) -> Arc<Panel> {
        let paster = match Paster::start() {
            Ok(paster) => Some(paster),
            Err(error) => {
                match platform::DELIVERY {
                    Delivery::Paste => log::error!(
                        "the clipboard could not be started, so nothing can be pasted: {error}"
                    ),
                    // Worse here than on Windows, and the log should say which: the clipboard
                    // is not a step of the hand-over on this platform, it is the whole of it.
                    Delivery::Clipboard => log::error!(
                        "the clipboard could not be started, so a dictation can only be read in the panel and copied by hand: {error}"
                    ),
                }
                None
            }
        };

        let panel = Arc::new(Panel {
            app: app.clone(),
            store,
            ui,
            remote: Mutex::new(None),
            paster,
            keyboard: Mutex::new(None),
            held: Mutex::new(Held::default()),
        });

        if let Some(window) = app.get_webview_window(WINDOW) {
            let moved = Arc::clone(&panel);
            window.on_window_event(move |event| {
                if let WindowEvent::Moved(position) = event {
                    moved.remember(position.x, position.y);
                }
            });
            report_window_style(&window);
        } else {
            log::error!("there is no panel window, so no dictation has anywhere to go");
        }

        panel
    }

    /// Tell the panel how to reach the hook that is installed right now.
    pub fn attach(&self, remote: Remote) {
        match self.remote.lock() {
            Ok(mut held) => *held = Some(remote),
            Err(_) => log::error!("the panel cannot reach the hotkey hook; its keys are dead"),
        }
    }

    // ------------------------------------------------------------------ opening and closing

    /// Show the hint that says the hotkey works, after a tap too short to be a sentence.
    pub fn open_idle(&self) {
        {
            let Ok(mut held) = self.held.lock() else {
                return;
            };
            held.mode = Mode::Idle;
        }
        self.show();
        self.ui.emit(
            EVENT_PANEL,
            PanelPayload {
                mode: "idle",
                hotkey: crate::i18n::trigger_label(
                    &self.ui.strings(),
                    &self.store.get().hotkey.trigger,
                ),
                note: None,
            },
        );
    }

    /// A recording is starting: capture the target and open the panel on it.
    ///
    /// **A result still on screen is thrown away here**, with a line in the log. The user
    /// started talking again, which is a clearer statement of intent than a countdown that
    /// had not finished; the alternative — queueing dictations behind one another — would put
    /// a sentence into a window minutes after it was spoken.
    pub fn begin(&self) {
        let target = capture_target();
        match target.as_ref() {
            Some(target) => log::info!(
                "this dictation is aimed at {} (hwnd {:#x})",
                target.label,
                target.handle.as_isize()
            ),
            None => log::warn!("nothing had the focus, so this dictation is aimed at nothing"),
        }

        if let Ok(mut held) = self.held.lock() {
            if held.dictation.is_some() {
                log::info!("a new recording arrived over an unfinished result; the result is gone");
            }
            held.mode = Mode::Live;
            held.target = target;
            held.dictation = None;
            held.edited = None;
            held.strictness = self.store.get().strictness();
        }
        self.show();
        // The page is told the panel is live before any state arrives. Without it the first
        // `dile://state` of a dictation would land on a page that still believes it is
        // closed — and the resting state is emitted often enough that "open when a state
        // arrives" is not a rule the page can use instead.
        self.ui.emit(
            EVENT_PANEL,
            PanelPayload {
                mode: "live",
                hotkey: String::new(),
                note: None,
            },
        );
    }

    /// Put a finished dictation on screen.
    pub fn show_result(&self, raw: &str, cleaned: &str) {
        let (label, strictness) = {
            let Ok(mut held) = self.held.lock() else {
                return;
            };
            held.mode = Mode::Result;
            held.dictation = Some(Dictation {
                raw: raw.to_owned(),
                cleaned: cleaned.to_owned(),
            });
            held.edited = Some(cleaned.to_owned());
            held.strictness = self.store.get().strictness();
            (
                held.target.as_ref().map(|target| target.label.clone()),
                held.strictness,
            )
        };

        self.show();
        self.ui.emit(
            EVENT_RESULT,
            ResultPayload {
                raw: raw.to_owned(),
                cleaned: cleaned.to_owned(),
                target_label: label,
                strictness: strictness.as_str(),
            },
        );
    }

    /// Hide the panel and forget what was in it.
    pub fn close(&self) {
        if let Ok(mut held) = self.held.lock() {
            held.mode = Mode::Closed;
            held.dictation = None;
            held.edited = None;
        }
        self.ui.emit(
            EVENT_PANEL,
            PanelPayload {
                mode: "closed",
                hotkey: String::new(),
                note: None,
            },
        );
        self.hide();
    }

    // ------------------------------------------------------------------------- the actions

    /// Transfer what is in the panel into the window it was aimed at.
    ///
    /// **Blocks, so never call it on the main thread.** Restoring the user's clipboard means
    /// waiting for the target to come forward and then for the paste receipt, which is up to
    /// two and a bit seconds; the main thread is where every window in this process draws
    /// itself. Both callers — the webview command and the session's Enter — put it on a
    /// thread of their own.
    pub fn transfer(&self, text: Option<String>) {
        let (target, body) = {
            let Ok(mut held) = self.held.lock() else {
                return;
            };
            if held.mode != Mode::Result {
                return;
            }
            if let Some(text) = text {
                held.edited = Some(text);
            }
            // The edited text when the webview has sent one, and the cleaned transcript when
            // it has not — a transfer that arrives before the first keystroke still has
            // something to paste.
            let body = held
                .edited
                .clone()
                .or_else(|| {
                    held.dictation
                        .as_ref()
                        .map(|dictation| dictation.cleaned.clone())
                })
                .unwrap_or_default();
            (held.target.clone(), body)
        };

        if body.trim().is_empty() {
            log::info!("there was nothing in the panel to transfer");
            self.close();
            return;
        }

        // Where there is no window to aim at, the transfer *is* the hand-over: the text goes
        // on the clipboard and the card says so, rather than being pasted into whatever
        // happened to be in front. `platform::DELIVERY` is where that was decided and why.
        if platform::DELIVERY == Delivery::Clipboard {
            self.hand_over(&body, self.wants_auto_paste());
            return;
        }

        let Some(target) = target else {
            self.report_target_gone();
            return;
        };
        if !target.handle.exists() {
            self.report_target_gone();
            return;
        }

        // The window style goes back before the target does, or the panel would still be the
        // window Windows thinks the user is working in.
        self.set_focusable(false);
        self.close();

        let Some(paster) = self.paster.as_ref() else {
            log::error!("there is no clipboard, so this dictation could not be pasted");
            return;
        };
        match paster.transfer(target.handle, &target.exe, &body) {
            Outcome::Pasted | Outcome::Unclaimed => {}
            Outcome::TargetGone => self.report_target_gone(),
            Outcome::Refused => log::error!("the clipboard refused this transfer"),
        }
    }

    /// Put the text on the clipboard and paste nothing.
    ///
    /// Deliberately does **not** restore the previous clipboard afterwards: the user asked
    /// for their clipboard to hold this (`docs/PROJECT.md` §3).
    ///
    /// **Blocks, so never call it on the main thread**, for the reason [`Panel::transfer`]
    /// gives and for one more on Linux: the clipboard there lives on the main loop, so a copy
    /// that waited on it from the main loop would wait for itself.
    pub fn copy(&self, text: Option<String>) {
        let body = {
            let Ok(mut held) = self.held.lock() else {
                return;
            };
            if held.mode != Mode::Result {
                return;
            }
            if let Some(text) = text {
                held.edited = Some(text);
            }
            held.edited
                .clone()
                .or_else(|| {
                    held.dictation
                        .as_ref()
                        .map(|dictation| dictation.cleaned.clone())
                })
                .unwrap_or_default()
        };

        if platform::DELIVERY == Delivery::Clipboard {
            // Copy and transfer are one action on a platform that hands over on the
            // clipboard, and the card says the same thing after either of them. Closing is
            // left to the card, which waits long enough for the sentence to be read.
            //
            // **With one difference, and it is the whole of what the two words mean.** Copy
            // is *put this on my clipboard*; transfer is *put this where I am typing*. So the
            // experimental chord belongs to the second and never to the first: a person who
            // pressed Copy asked for the text to be somewhere they can take it from, not for
            // it to be pasted into whatever is in front of them.
            self.hand_over(&body, false);
            return;
        }

        match self.paster.as_ref() {
            Some(paster) => match paster.copy(&body) {
                Ok(()) => log::info!("the dictation was copied; the clipboard is not restored"),
                Err(error) => log::error!("the dictation could not be copied: {error}"),
            },
            None => log::error!("there is no clipboard to copy into"),
        }
        self.set_focusable(false);
        self.close();
    }

    /// Put the dictation on the clipboard and tell the card which of the two happened.
    ///
    /// The whole of the hand-over where [`Delivery::Clipboard`] is what this platform does.
    /// **The card is left on screen either way**: after a success for as long as it takes to
    /// read *press `Ctrl+V`*, and after a failure until the user takes the text out of it by
    /// hand or presses Esc. A card that vanished on a clipboard that refused would lose the
    /// dictation, which is the one outcome this application never accepts.
    ///
    /// `auto_paste` is the experimental setting, and the exception to the paragraph above: a
    /// chord that was actually sent takes the card down first, because the card is what holds
    /// the keyboard. [`Panel::press_paste`] is that sequence.
    ///
    /// **Blocks**, like everything else that touches a clipboard: see [`Panel::transfer`].
    fn hand_over(&self, body: &str, auto_paste: bool) {
        let Some(paster) = self.paster.as_ref() else {
            log::error!(
                "there is no clipboard on this platform, so the dictation stays in the panel"
            );
            self.report_clipboard_failed();
            return;
        };

        match paster.copy(body) {
            Ok(()) => {
                log::info!(
                    "the dictation is on the clipboard; Ctrl+V puts it wherever the caret is"
                );
                // The clipboard first and the chord second, always. If the chord never goes
                // out — the setting is off, or there is no way to send one — the text is
                // already somewhere the person can take it from, which is the promise this
                // platform actually makes.
                match self.press_paste(auto_paste) {
                    // The card is already gone: it had to be, or the chord would have landed
                    // in it. Nothing left to emit.
                    Chord::Handled => return,
                    Chord::NotAsked => self.report_copied(None),
                    Chord::Unavailable(note) => self.report_copied(Some(note)),
                }
            }
            Err(error) => {
                log::error!("the dictation could not be put on the clipboard: {error}");
                self.report_clipboard_failed();
            }
        }

        // After the write rather than before it, and after either outcome. The card still has
        // text on it that a person may want to select by hand, but it has no more typing to
        // take — and on a platform where this flag means something, a card that stayed
        // activatable would keep the caret it had borrowed.
        self.set_focusable(false);
    }

    /// Whether this dictation should end with a `Ctrl+V` this application pressed itself.
    fn wants_auto_paste(&self) -> bool {
        auto_paste_wanted(&self.store.get())
    }

    /// Send `Ctrl+V` at whatever holds the keyboard, with the card taken out of the way first.
    ///
    /// **The order is the whole of this function, and it is not the obvious one.**
    ///
    /// 1. The virtual keyboard is opened *before* anything moves, because opening one is what
    ///    fails, and a failure has to leave the card up with a line on it. A card cannot be
    ///    put back once it is down.
    /// 2. The card is hidden. It holds the keyboard while it is on screen — that is what let
    ///    the clipboard be written at all (`platform::linux`) — so a chord sent now would be
    ///    typed into the card.
    /// 3. [`FOCUS_RETURN`] passes, which is the compositor handing the keyboard back to
    ///    whatever had it. Nothing tells a client when that has happened.
    /// 4. The chord goes out, at whatever holds the keyboard **now**. Not at a window this
    ///    application chose, because it cannot choose one.
    ///
    /// A chord that could not be written after step 2 is a line in the log and nothing on
    /// screen: the card is down by then. The dictation is on the clipboard in every one of
    /// these cases, which is why none of them loses anything.
    ///
    /// **Blocks** for the settle window on the first call and for [`FOCUS_RETURN`] on every
    /// one. Both callers are already off the main thread.
    fn press_paste(&self, asked: bool) -> Chord {
        if !asked {
            return Chord::NotAsked;
        }

        let Ok(mut keyboard) = self.keyboard.lock() else {
            log::error!(
                "auto-paste: the virtual keyboard cannot be reached, because a thread panicked holding it"
            );
            return Chord::Unavailable(AUTO_PASTE_FAILED);
        };

        if keyboard.is_none() {
            match AutoPaste::open() {
                Ok(opened) => *keyboard = Some(opened),
                Err(error) => {
                    log::warn!(
                        "auto-paste is on and there is no virtual keyboard to send a chord with: {error}"
                    );
                    return Chord::Unavailable(error.key());
                }
            }
        }
        let Some(opened) = keyboard.as_ref() else {
            // Unreachable: the block above either filled it or returned.
            return Chord::Unavailable(AUTO_PASTE_FAILED);
        };

        self.set_focusable(false);
        self.close();
        std::thread::sleep(FOCUS_RETURN);

        match opened.press_paste() {
            Ok(()) => log::info!(
                "auto-paste: Ctrl+V was sent to whatever holds the keyboard, which this application is not told"
            ),
            Err(error) => log::error!(
                "auto-paste: the chord could not be sent: {error}. The dictation is on the clipboard"
            ),
        }
        Chord::Handled
    }

    /// The card's one line after a hand-over: *copied*, or why the chord did not happen.
    fn report_copied(&self, note: Option<&'static str>) {
        self.ui.emit(
            EVENT_PANEL,
            PanelPayload {
                mode: "copied",
                hotkey: String::new(),
                note,
            },
        );
    }

    /// Close the panel and paste nothing at all.
    pub fn cancel(&self) {
        log::info!("the panel was cancelled; nothing was pasted and the clipboard is untouched");
        self.set_focusable(false);
        self.close();
    }

    /// Throw this dictation away and get ready for another one.
    ///
    /// In toggle mode the next recording starts at once, because there is no key to hold; in
    /// hold mode the panel simply goes away and the next press starts fresh. The asymmetry is
    /// `dile-hotkey`'s, not this module's — see `HotkeyMachine::rerecord`.
    pub fn rerecord(&self) {
        self.set_focusable(false);
        self.close();

        let armed = match self.remote.lock() {
            Ok(remote) => remote.as_ref().map(Remote::rerecord),
            Err(_) => None,
        };
        match armed {
            Some(Ok(())) => log::info!("re-record: armed; a press starts a fresh dictation"),
            Some(Err(error)) => log::warn!("re-record could not reach the hook: {error}"),
            None => log::warn!("re-record: there is no hook to arm"),
        }
    }

    /// Run the cleanup again at another strictness, without changing the setting.
    ///
    /// The level is **per dictation**. A person reaching for "strict" because one sentence
    /// came out messy is not asking for every future sentence to be treated that way, and a
    /// panel button that quietly rewrites a setting is the kind of thing people stop trusting.
    #[must_use]
    pub fn reclean(&self, raw: &str, level: Strictness) -> String {
        // The panel sends the raw transcript back with the request, but the authority is the
        // copy held here: a webview that has been edited, reloaded or raced could send
        // something that is not what the engine wrote, and every rule in `dile-core` assumes
        // it is looking at an engine's output rather than at its own previous answer.
        let source = self
            .held
            .lock()
            .ok()
            .and_then(|held| {
                held.dictation
                    .as_ref()
                    .map(|dictation| dictation.raw.clone())
            })
            .filter(|held| !held.is_empty())
            .unwrap_or_else(|| raw.to_owned());

        let settings = self.store.get();
        let config = CleanupConfig {
            dictionary: settings.dictionary(),
            ..CleanupConfig::default()
        };
        let cleaned = cleanup::clean_with(&source, level, &config).text;

        if let Ok(mut held) = self.held.lock() {
            held.strictness = level;
            held.edited = Some(cleaned.clone());
        }
        cleaned
    }

    /// The webview says the text has changed. Remember it for a transfer that may not come
    /// from the webview at all.
    pub fn edited(&self, text: String) {
        if let Ok(mut held) = self.held.lock() {
            held.edited = Some(text);
        }
    }

    // ------------------------------------------------------------------------ the window

    /// The settings the panel needs to draw itself.
    #[must_use]
    pub fn context(&self) -> Context {
        let settings = self.store.get();
        let held = self.held.lock().ok();
        Context {
            hotkey: crate::i18n::trigger_label(&self.ui.strings(), &settings.hotkey.trigger),
            auto_transfer: settings.cleanup.auto_transfer
                && !held.as_ref().is_some_and(|held| held.preview),
            auto_transfer_ms: settings.cleanup.auto_transfer_ms,
            cap_ms: settings.cap_ms(),
            strictness: held
                .as_ref()
                .map_or(Strictness::Medium, |held| held.strictness)
                .as_str(),
            preview: held.as_ref().is_some_and(|held| held.preview),
            target_label: held
                .and_then(|held| held.target.as_ref().map(|target| target.label.clone())),
        }
    }

    /// Grow or shrink the card to fit its text.
    ///
    /// Driven from the webview, because only the webview can measure a line of text that has
    /// just been wrapped — but applied from Rust, because the panel's capability deliberately
    /// does not include `core:window:allow-set-size`.
    pub fn resize(&self, card_height: f64) {
        let Some(window) = self.window() else { return };
        // What the page measures is the card; what the window needs is the card and its
        // gutter. Clamped here rather than in the page, because a webview that could ask for
        // any height could ask for one that covers the screen.
        let clamped = card_height.clamp(CARD_BASE, CARD_MAX) + GUTTER * 2.0;
        if let Err(error) = window.set_size(LogicalSize::new(WIDTH, clamped)) {
            log::warn!("the panel could not be resized: {error}");
        }
    }

    /// Let the panel take the focus, because the user clicked into the text.
    ///
    /// The one action that activates this window. `focusable` goes back down on ✓, on ✗ and
    /// on copy, so the window spends the rest of its life unable to steal anything.
    pub fn take_focus(&self) {
        let Some(window) = self.window() else { return };
        if let Err(error) = window.set_focusable(true) {
            log::warn!("the panel could not be made focusable: {error}");
            return;
        }
        if let Err(error) = window.set_focus() {
            log::warn!("the panel could not take the focus: {error}");
        }
    }

    /// Put the window style back and hand the focus to the target.
    pub fn release_focus(&self) {
        self.set_focusable(false);
        let target = self
            .held
            .lock()
            .ok()
            .and_then(|held| held.target.as_ref().map(|target| target.handle));
        if let Some(handle) = target {
            let _ = handle.focus();
        }
    }

    /// Show the window, placing it first if it was hidden.
    fn show(&self) {
        let Some(window) = self.window() else { return };

        let already = window.is_visible().unwrap_or(false);
        if !already {
            self.place(&window);
            if let Err(error) = window.set_size(LogicalSize::new(WIDTH, BASE_HEIGHT)) {
                log::warn!("the panel could not be sized: {error}");
            }
            if let Err(error) = window.show() {
                log::warn!("the panel could not be shown: {error}");
            }
            self.arm_keys(true);
        }
    }

    /// Hide the window and give the three keys back to the machine.
    fn hide(&self) {
        self.arm_keys(false);
        self.persist_position();
        if let Some(window) = self.window()
            && let Err(error) = window.hide()
        {
            log::warn!("the panel could not be hidden: {error}");
        }
    }

    /// Put the card where this dictation's monitor says it belongs.
    fn place(&self, window: &tauri::WebviewWindow) {
        let monitor = self
            .held
            .lock()
            .ok()
            .and_then(|held| held.target.as_ref().map(|target| target.handle))
            .and_then(Hwnd::monitor)
            .or_else(|| Hwnd::foreground().and_then(Hwnd::monitor))
            .or_else(window::primary_monitor);

        let Some(monitor) = monitor else {
            log::warn!("no monitor could be identified, so the panel is left where it was");
            return;
        };

        let scale = window.scale_factor().unwrap_or(1.0);
        let remembered = self
            .store
            .get()
            .ui
            .panel_position
            .get(&monitor.device)
            .copied();

        let position = remembered
            .map(|position| clamp_into(&monitor, position, scale))
            .unwrap_or_else(|| top_centre(&monitor, scale));

        if let Ok(mut held) = self.held.lock() {
            held.monitor = Some(monitor.clone());
            held.dragged = None;
            held.placed = Some(position);
        }
        if let Err(error) = window.set_position(PhysicalPosition::new(position.x, position.y)) {
            log::warn!("the panel could not be placed: {error}");
        }
    }

    /// The user dragged the card. Remember where to, for this monitor.
    fn remember(&self, x: i32, y: i32) {
        let moved = PanelPosition { x, y };
        if let Ok(mut held) = self.held.lock()
            && held.placed != Some(moved)
        {
            held.dragged = Some(moved);
        }
    }

    /// Write the dragged position into the settings, on the way out.
    ///
    /// Not on every `Moved`: a drag is a hundred of those, and each one would be a settings
    /// document compared, written to a temporary file and renamed over the real one.
    fn persist_position(&self) {
        let moved = {
            let Ok(mut held) = self.held.lock() else {
                return;
            };
            let (Some(monitor), Some(dragged)) = (held.monitor.clone(), held.dragged.take()) else {
                return;
            };
            // Stored relative to the display's own corner, so that unplugging a screen and
            // plugging it back in at a different arrangement does not put the card somewhere
            // off the desktop.
            (
                monitor.device,
                PanelPosition {
                    x: dragged.x - monitor.bounds.left,
                    y: dragged.y - monitor.bounds.top,
                },
            )
        };

        let mut settings = self.store.get();
        if settings.ui.panel_position.get(&moved.0) == Some(&moved.1) {
            return;
        }
        log::info!("the panel position on {} was remembered", moved.0);
        settings.ui.panel_position.insert(moved.0, moved.1);
        if let Err(error) = self.store.set(settings) {
            log::warn!("the panel position could not be saved: {error}");
        }
    }

    /// Arm or disarm the panel's three keys on the hook that is installed right now.
    fn arm_keys(&self, enabled: bool) {
        let armed = match self.remote.lock() {
            Ok(remote) => remote.as_ref().map(|remote| remote.panel_keys(enabled)),
            Err(_) => None,
        };
        match armed {
            Some(Ok(())) | None => {}
            Some(Err(error)) => log::warn!("the panel keys could not be changed: {error}"),
        }
    }

    fn set_focusable(&self, focusable: bool) {
        if let Some(window) = self.window()
            && let Err(error) = window.set_focusable(focusable)
        {
            log::warn!("the panel window style could not be changed: {error}");
        }
    }

    fn report_target_gone(&self) {
        log::warn!("the target window is gone; the text is still in the panel");
        self.ui.emit(
            EVENT_PANEL,
            PanelPayload {
                mode: "target-gone",
                hotkey: String::new(),
                note: None,
            },
        );
    }

    /// The clipboard refused, so the card keeps the text and the countdown stops.
    ///
    /// The same shape of answer as [`Panel::report_target_gone`] and for the same reason: a
    /// dictation that could not be handed over is still a dictation, and the only place it
    /// exists is the card in front of the user.
    fn report_clipboard_failed(&self) {
        self.ui.emit(
            EVENT_PANEL,
            PanelPayload {
                mode: "clipboard-failed",
                hotkey: String::new(),
                note: None,
            },
        );
    }

    fn window(&self) -> Option<tauri::WebviewWindow> {
        self.app.get_webview_window(WINDOW)
    }
}

/// Whether a finished dictation should end with a `Ctrl+V` this application pressed itself.
///
/// **Two conditions, and the platform one comes first.** A build that cannot synthesise a key
/// press must not act on a setting about one: a `settings.json` carried from a Linux machine
/// to a Windows one would otherwise turn on a feature that does not exist there, and the
/// switch that wrote it is not even drawn in that window.
///
/// A free function of one document rather than a method, so that the rule is tested on a
/// machine with no application, no window and no keyboard — which is every machine that runs
/// this suite.
fn auto_paste_wanted(settings: &crate::settings::Settings) -> bool {
    platform::AUTO_PASTE_OFFERED && settings.paste.auto
}

/// Say whether the card can actually refuse the focus, read off the real window.
///
/// The style bit the whole design rests on, read back rather than assumed from the
/// configuration file: `focusable: false` is a request, and a request that was quietly not
/// honoured is the difference between a panel and a thief.
#[cfg(windows)]
fn report_window_style(window: &tauri::WebviewWindow) {
    match window
        .hwnd()
        .ok()
        .and_then(|handle| crate::platform::win32::window::Hwnd::from_isize(handle.0 as isize))
    {
        Some(handle) => log::info!(
            "panel window: hwnd {:#x}, non-activating {}",
            handle.as_isize(),
            handle.is_non_activating()
        ),
        None => log::warn!("the panel window has no handle to check the style on"),
    }
}

/// The same report, on a platform where the answer is known in advance.
///
/// There is no style bit to read: `xdg-shell` has no "do not activate me" request, so
/// `focusable: false` is accepted by Tauri and means nothing to the compositor. A card that
/// may take the focus is a different product from the one this module documents, and a
/// person running it deserves to be told that at start-up rather than to work it out from a
/// caret that stopped blinking.
///
/// **And on a clipboard-first platform it is also the reason the hand-over works at all.**
/// A Wayland client may only set the selection while it has the keyboard, so the flag Dile
/// cannot have here is the one that lets the card put a dictation on the clipboard — see
/// `platform/linux.rs`. Both halves of that go in the same line, because a warning that only
/// said the first half would read as a defect rather than as a trade.
#[cfg(not(windows))]
fn report_window_style(window: &tauri::WebviewWindow) {
    let _ = window;
    match platform::DELIVERY {
        Delivery::Paste => log::warn!(
            "panel window: this platform has no non-activating window style, so the card may take the focus from whatever you are typing in"
        ),
        Delivery::Clipboard => log::warn!(
            "panel window: this platform has no non-activating window style, so the card takes the focus from whatever you are typing in — which is also what lets it put a dictation on the clipboard"
        ),
    }
}

/// The card's default place: centred on the work area, below the top edge.
fn top_centre(monitor: &Monitor, scale: f64) -> PanelPosition {
    let width = (WIDTH * scale).round() as i32;
    // The margin is measured to the card's edge, and the window's edge is one gutter further
    // out — so a person who asked for 24 px of clearance gets 24 px of clearance rather than
    // 24 px of invisible window.
    let margin = ((TOP_MARGIN - GUTTER) * scale).round() as i32;
    PanelPosition {
        x: monitor.work.left + (monitor.work.width() - width) / 2,
        y: monitor.work.top + margin,
    }
}

/// A remembered place, kept on the screen it was remembered for.
///
/// The stored value is relative to the monitor's top-left corner, so a display that moved in
/// the arrangement takes its panel with it rather than leaving it on the desktop's edge.
fn clamp_into(monitor: &Monitor, position: PanelPosition, scale: f64) -> PanelPosition {
    let width = (WIDTH * scale).round() as i32;
    let height = (MAX_HEIGHT * scale).round() as i32;
    // A screen narrower or shorter than the card is not a reason to panic in a clamp; the
    // card simply starts at the corner and overhangs.
    let right = (monitor.bounds.left + monitor.bounds.width() - width).max(monitor.bounds.left);
    let bottom = (monitor.bounds.top + monitor.bounds.height() - height).max(monitor.bounds.top);
    PanelPosition {
        x: (monitor.bounds.left + position.x).clamp(monitor.bounds.left, right),
        y: (monitor.bounds.top + position.y).clamp(monitor.bounds.top, bottom),
    }
}

/// Who has the focus, and what program is that.
fn capture_target() -> Option<Target> {
    let handle = Hwnd::foreground()?;
    let exe = handle.process_path().unwrap_or_default();
    Some(Target {
        handle,
        label: crate::paste::label_for(&exe),
        exe,
    })
}

/// The payload of [`EVENT_RESULT`].
#[derive(Clone, Debug, Serialize)]
pub struct ResultPayload {
    /// What the engine wrote, before any rule ran.
    pub raw: String,
    /// What the Turkish layer made of it.
    pub cleaned: String,
    /// The friendly name of the window this is aimed at, if there is one.
    pub target_label: Option<String>,
    /// Which strictness produced `cleaned`.
    pub strictness: &'static str,
}

/// The payload of [`EVENT_PANEL`].
#[derive(Clone, Debug, Serialize)]
pub struct PanelPayload {
    /// `live`, `idle`, `closed`, `copied`, `clipboard-failed` or `target-gone`.
    pub mode: &'static str,
    /// The chord the idle hint names. Empty for every other mode.
    pub hotkey: String,
    /// A locale key to show **instead of** the mode's own line, where there is something more
    /// specific to say.
    ///
    /// One user today: the card says *copied — press `Ctrl+V`* after a hand-over, and says
    /// why auto-paste did not happen when somebody had turned it on. Both are the same one
    /// line in the same corner, because the result layout must not move under a person who is
    /// reading it.
    ///
    /// A key rather than a sentence, for the rule every visible word in this product follows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// The settings the panel draws itself with.
#[derive(Clone, Debug, Serialize)]
pub struct Context {
    /// The chord, as the settings spell it.
    pub hotkey: String,
    /// Whether the countdown runs at all.
    pub auto_transfer: bool,
    /// How long it runs for, in milliseconds.
    pub auto_transfer_ms: u32,
    /// The recording cap, in milliseconds, for the `00:07 / 60 s` line.
    pub cap_ms: u64,
    /// Which strictness pill is lit.
    pub strictness: &'static str,
    /// The friendly name of the window the current dictation is aimed at.
    pub target_label: Option<String>,
    /// Whether this card is a debug preview rather than a dictation.
    ///
    /// A preview does not count down and does not close itself: both would make a state
    /// impossible to photograph, which is the only thing a preview is for. Always `false` in
    /// a release build, where nothing can set it.
    pub preview: bool,
}

/// How long the debug switch leaves a state on screen before the process is killed by hand.
#[cfg(debug_assertions)]
const DEBUG_SAMPLE_RAW: &str = "eee kubernetes cluster'ını bugün kurdum webhookları yarın sertifika yenilemesini cert manager'a bıraktım";

/// Render one state with sample text, for a screenshot.
///
/// **Debug builds only.** There is no way to speak into a microphone from a script, so this
/// is how the four states get looked at: `DILE_OPEN_PANEL=result cargo tauri dev`.
///
/// It deliberately does **not** arm the panel's keys. A preview that swallowed Esc and Enter
/// for the whole machine while somebody was taking a screenshot would be a worse bug than the
/// one it was helping to find.
#[cfg(debug_assertions)]
pub fn debug_open(panel: &Arc<Panel>, what: &str) {
    let panel = Arc::clone(panel);
    let what = what.to_owned();
    // **After the webview has loaded.** Every one of these states is an event, and an event
    // emitted during `setup` is emitted at a page that has not finished parsing its own
    // script yet — which is an empty card and half an hour of wondering why. A debug switch
    // is allowed to solve that with a sleep; nothing in the product path does.
    let spawned = std::thread::Builder::new()
        .name("dile-debug-panel".to_owned())
        .spawn(move || {
            std::thread::sleep(DEBUG_PAGE_LOAD);
            debug_render(&panel, &what);
        });
    if let Err(error) = spawned {
        log::error!("the debug panel could not be opened: {error}");
    }
}

/// How long the debug switch waits for the panel's page to be ready for an event.
#[cfg(debug_assertions)]
const DEBUG_PAGE_LOAD: std::time::Duration = std::time::Duration::from_millis(1_200);

#[cfg(debug_assertions)]
fn debug_render(panel: &Arc<Panel>, what: &str) {
    use crate::tray::Status;

    log::warn!("DILE_OPEN_PANEL={what}: rendering a sample state");
    if let Ok(mut held) = panel.held.lock() {
        held.preview = true;
    }
    match what {
        "recording" => {
            panel.begin();
            panel.ui.show(Status::Recording);
        }
        "working" => {
            panel.begin();
            // Through the recording state, because that is the only way to reach this one: a
            // preview that jumped straight to "working" would be testing a transition the
            // product does not have.
            panel.ui.show(Status::Recording);
            panel.ui.show(Status::Working);
        }
        "nothing" => {
            panel.begin();
            panel.ui.show(Status::Recording);
            panel.ui.show(Status::NothingHeard);
        }
        "result" => {
            panel.begin();
            let settings = panel.store.get();
            let config = CleanupConfig {
                dictionary: settings.dictionary(),
                ..CleanupConfig::default()
            };
            let cleaned =
                cleanup::clean_with(DEBUG_SAMPLE_RAW, settings.strictness(), &config).text;
            panel.show_result(DEBUG_SAMPLE_RAW, &cleaned);
        }
        other => log::error!("DILE_OPEN_PANEL={other} is not a state this build can render"),
    }
    // The preview is for looking at, not for typing into.
    panel.arm_keys(false);
}

/// What the webview is allowed to ask for.
pub mod commands {
    use std::sync::Arc;

    use dile_core::cleanup::Strictness;
    use tauri::State;

    use super::{Context, Panel};

    /// The settings the panel draws itself with.
    #[tauri::command]
    pub fn panel_context(panel: State<'_, Arc<Panel>>) -> Context {
        panel.context()
    }

    /// Grow or shrink the card to fit its text.
    #[tauri::command]
    pub fn panel_resize(height: f64, panel: State<'_, Arc<Panel>>) {
        panel.resize(height);
    }

    /// The user clicked into the text, which is the one action that activates this window.
    #[tauri::command]
    pub fn panel_take_focus(panel: State<'_, Arc<Panel>>) {
        panel.take_focus();
    }

    /// The user clicked out of the text again.
    #[tauri::command]
    pub fn panel_release_focus(panel: State<'_, Arc<Panel>>) {
        panel.release_focus();
    }

    /// The text in the panel changed.
    #[tauri::command]
    pub fn panel_edited(text: String, panel: State<'_, Arc<Panel>>) {
        panel.edited(text);
    }

    /// ✓, or the countdown running out.
    #[tauri::command]
    pub fn panel_transfer(text: Option<String>, panel: State<'_, Arc<Panel>>) {
        let panel = Arc::clone(&panel);
        // Off the main thread, for the reason `Panel::transfer` documents.
        let spawned = std::thread::Builder::new()
            .name("dile-paste".to_owned())
            .spawn(move || panel.transfer(text));
        if let Err(error) = spawned {
            log::error!("the paste thread could not be started: {error}");
        }
    }

    /// The copy button.
    ///
    /// On a thread of its own for the same reason [`panel_transfer`] is, and it became the
    /// same reason only on Linux: Tauri runs a synchronous command **on the main loop**, and
    /// on Linux that loop is the one the clipboard belongs to. A copy that waited there for a
    /// clipboard whose answer arrives through that loop would be waiting for itself.
    #[tauri::command]
    pub fn panel_copy(text: Option<String>, panel: State<'_, Arc<Panel>>) {
        let panel = Arc::clone(&panel);
        let spawned = std::thread::Builder::new()
            .name("dile-copy".to_owned())
            .spawn(move || panel.copy(text));
        if let Err(error) = spawned {
            log::error!("the copy thread could not be started: {error}");
        }
    }

    /// ✗.
    #[tauri::command]
    pub fn panel_cancel(panel: State<'_, Arc<Panel>>) {
        panel.cancel();
    }

    /// The re-record button.
    #[tauri::command]
    pub fn panel_rerecord(panel: State<'_, Arc<Panel>>) {
        panel.rerecord();
    }

    /// Close the panel: what the "nothing heard" and idle states do when their moment is up.
    #[tauri::command]
    pub fn panel_close(panel: State<'_, Arc<Panel>>) {
        panel.close();
    }

    /// Run the cleanup again at another strictness, for this dictation only.
    #[tauri::command]
    pub fn panel_reclean(raw: String, level: String, panel: State<'_, Arc<Panel>>) -> String {
        let level = match level.as_str() {
            "light" => Strictness::Light,
            "strict" => Strictness::Strict,
            _ => Strictness::Medium,
        };
        panel.reclean(&raw, level)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BASE_HEIGHT, CARD_BASE, CARD_MAX, GUTTER, MAX_HEIGHT, PanelPayload, ResultPayload, WIDTH,
        auto_paste_wanted, clamp_into, top_centre,
    };
    use crate::platform::screen::{Monitor, Rect};
    use crate::settings::{PanelPosition, Settings};

    fn monitor(left: i32, top: i32) -> Monitor {
        Monitor {
            device: r"\\.\DISPLAY1".to_owned(),
            bounds: Rect {
                left,
                top,
                right: left + 1_920,
                bottom: top + 1_080,
            },
            work: Rect {
                left,
                top,
                right: left + 1_920,
                bottom: top + 1_032,
            },
        }
    }

    #[test]
    fn the_card_lands_top_centre_of_whichever_monitor_it_was_asked_about() {
        let primary = top_centre(&monitor(0, 0), 1.0);
        // The window is centred, so the card inside it is too — the gutter is the same on
        // both sides.
        assert_eq!(primary.x, (1_920 - WIDTH as i32) / 2);
        assert_eq!(
            primary.y + GUTTER as i32,
            24,
            "the *card* is 24 px below the work area, not the window around it"
        );

        // A second screen to the left of the primary has negative coordinates, which is the
        // case an absolute "centre of the screen" would get wrong.
        let left_hand = top_centre(&monitor(-1_920, 0), 1.0);
        assert_eq!(left_hand.x, -1_920 + (1_920 - WIDTH as i32) / 2);
        assert_eq!(left_hand.y + GUTTER as i32, 24);
    }

    #[test]
    fn a_high_dpi_screen_gets_the_same_card_in_more_pixels() {
        let scaled = top_centre(&monitor(0, 0), 1.5);
        assert_eq!(scaled.x, (1_920 - (WIDTH * 1.5) as i32) / 2);
        assert_eq!(
            scaled.y,
            ((24.0 - GUTTER) * 1.5) as i32,
            "24 logical pixels to the card's edge, at 150 %"
        );
    }

    #[test]
    fn a_remembered_position_is_relative_to_its_monitor_and_stays_on_it() {
        let screen = monitor(-1_920, -200);
        let remembered = PanelPosition { x: 40, y: 300 };
        let placed = clamp_into(&screen, remembered, 1.0);
        assert_eq!(placed.x, -1_920 + 40);
        assert_eq!(placed.y, -200 + 300);

        // And a position from a screen that has since shrunk is pulled back onto it rather
        // than leaving the card somewhere nobody can reach.
        let far = PanelPosition { x: 9_000, y: 9_000 };
        let pulled = clamp_into(&screen, far, 1.0);
        assert!(pulled.x <= screen.bounds.right - WIDTH as i32);
        assert!(pulled.y <= screen.bounds.bottom - MAX_HEIGHT as i32);
    }

    #[test]
    fn the_card_never_grows_past_three_lines_and_never_shrinks_below_one() {
        // The mockup: 64 px for the one-line states, 92 for a result on one line, 112 for
        // two. Three lines plus the footer is the 2026-09-13 decision, and `resize` clamps
        // whatever the page measures into exactly that range before a gutter is added.
        for asked in [0.0_f64, 12.0, 64.0, 100.0, 168.0, 900.0] {
            let applied = asked.clamp(CARD_BASE, CARD_MAX) + GUTTER * 2.0;
            assert!(
                (BASE_HEIGHT..=MAX_HEIGHT).contains(&applied),
                "{asked} became {applied}"
            );
        }
        // The ceiling has to fit a third line of 14 px text on top of the two-line mockup.
        let third_line = 112.0_f64 + 14.0 * 1.45;
        assert!(CARD_MAX >= third_line, "{CARD_MAX} < {third_line}");
    }

    #[test]
    fn the_two_payloads_are_the_shape_the_webview_reads() {
        let result = serde_json::to_value(ResultPayload {
            raw: "eee cron".to_owned(),
            cleaned: "Cron".to_owned(),
            target_label: Some("VS Code".to_owned()),
            strictness: "medium",
        })
        .expect("a result payload serializes");
        assert_eq!(result["raw"], "eee cron");
        assert_eq!(result["cleaned"], "Cron");
        assert_eq!(result["target_label"], "VS Code");
        assert_eq!(result["strictness"], "medium");

        let panel = serde_json::to_value(PanelPayload {
            mode: "closed",
            hotkey: String::new(),
            note: None,
        })
        .expect("a panel payload serializes");
        assert_eq!(panel["mode"], "closed");

        // A payload with nothing extra to say carries **no** note at all rather than a null,
        // because the page's fallback is `payload.note || "panel.state.copied"` and a null
        // and a missing key have to mean the same thing there.
        assert!(panel.get("note").is_none(), "{panel}");

        let noted = serde_json::to_value(PanelPayload {
            mode: "copied",
            hotkey: String::new(),
            note: Some("panel.state.autopaste.nopermission"),
        })
        .expect("a panel payload serializes");
        assert_eq!(noted["note"], "panel.state.autopaste.nopermission");
    }

    #[test]
    fn the_setting_alone_does_not_send_a_chord_on_a_build_that_has_none() {
        // A settings file is carried between machines, and this is the rule that makes that
        // safe: the switch is drawn on one platform and read on the same one. On Windows a
        // dictation is pasted into the window it was aimed at, and a stray `true` in a file
        // must not add an unaimed key press to that.
        let mut settings = Settings::default();
        assert!(!auto_paste_wanted(&settings), "the shipped default is off");

        settings.paste.auto = true;
        assert_eq!(
            auto_paste_wanted(&settings),
            crate::platform::AUTO_PASTE_OFFERED
        );
        if cfg!(windows) {
            assert!(!auto_paste_wanted(&settings));
        }
    }
}
