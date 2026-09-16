//! The clipboard on Linux, on the application's own Wayland connection.
//!
//! This is the one of [`super::unported`]'s four answers that Linux replaces. The other three
//! stay no — there is no foreground window to name, no screen coordinate to place a card at
//! and no paste chord to send — and `super`'s table says which module answers what.
//!
//! ## Why GTK rather than a clipboard crate
//!
//! Two things were measured on this machine (Fedora 44, GNOME 50.4, Wayland) before either
//! was written:
//!
//! | Route | Wrote the selection | Costs |
//! |---|---|---|
//! | `gtk::Clipboard`, this module | yes, with the caveat below | **no new crates**: `gtk`, `gdk`, `gio` and `glib` 0.18 are already compiled into this binary, because Tauri's Linux backend is GTK3 |
//! | `arboard`, which is what `tauri-plugin-clipboard-manager` uses | yes, even with no window at all | six new crates, and it gets there through **XWayland** — `x11rb` talking to the X server that GNOME bridges to the Wayland selection |
//!
//! `arboard` is the more capable of the two and it is still the wrong one here. It reaches
//! the clipboard through a compatibility layer rather than through the connection this
//! application already has, so a session without XWayland has no clipboard at all and nothing
//! says so; and it cannot offer a *delayed render*, which is the whole mechanism the paste
//! path of `docs/PROJECT.md` §3 is built on and the reason it was already turned down for
//! Windows. GTK can (`gtk_clipboard_set_with_data`), which is what the Linux paste package
//! will need.
//!
//! ## The rule this module is built around, measured rather than assumed
//!
//! **A Wayland client may set the selection only while it holds the keyboard focus.**
//! `wl_data_device.set_selection` takes an input serial, and GNOME rejects one from a client
//! that is not the focused one. Measured both ways with an unrelated window stealing the
//! focus in between: focused, `wl-paste` read the text back; unfocused, `set_text` returned
//! normally, logged nothing, and the selection never changed.
//!
//! **And `wait_for_text` cannot be used to check.** GDK answers a read from its own local
//! owner without asking the compositor, so after a write that the compositor threw away it
//! still returns the text that was written. A check that passes when the thing failed is
//! worse than no check.
//!
//! What does work is the compositor's own answer: a selection that is actually taken comes
//! back as an `owner-change`, and one that is refused does not. [`Clipboard::set_text`]
//! writes, then waits [`RECEIPT_WINDOW`] for that event. Measured: it is there inside the
//! first [`RECEIPT_TICK`] on a successful write, and never at all on a refused one. Both
//! halves were checked against the real application — a build with this subscription taken
//! out reported the refusal, on a machine where the write had in fact succeeded.
//!
//! ### Which makes the panel's Linux limitation the reason this works at all
//!
//! `panel.rs` asks for a window that cannot take the focus, and Wayland has no way to grant
//! that — the card takes the keyboard whether it asks to or not (`wl_keyboard.enter` arrives
//! on the panel surface, seen on the wire). On Windows that flag is what makes the paste
//! land in the right place. Here its absence is what makes the copy land at all.
//!
//! ## Two more measured facts, because both are promises to the user
//!
//! * **The selection survives the panel being hidden.** The owner is the connection, not the
//!   surface, so hiding the card does not take the text back.
//! * **It does not survive the process.** Once the main loop stops, nothing can answer a
//!   paste request. Dile is a tray application and stays running, so this only bites on
//!   *Quit* — and only where no clipboard manager took a copy. `README.md` says so.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};

/// How long to wait for the compositor to say it took the selection.
///
/// A successful write is answered well inside the first [`RECEIPT_TICK`], and a refused one
/// is never answered at all — so this is the time a *failure* costs and a success does not,
/// and the panel spends it with the card still on screen and the text still in it. Wide
/// rather than tight because it is the loaded-machine margin on a signal that is normally
/// immediate, and being slow to admit a failure is cheaper than inventing one.
const RECEIPT_WINDOW: Duration = Duration::from_millis(900);

/// How often the receipt is looked for while waiting for it.
const RECEIPT_TICK: Duration = Duration::from_millis(25);

/// How long a caller waits for the main loop to get round to its job at all.
///
/// Generously more than [`RECEIPT_WINDOW`], because it covers the same wait plus whatever
/// the main loop was busy with when the job was posted.
const MAIN_LOOP_DEADLINE: Duration = Duration::from_millis(2_500);

/// What can go wrong with a clipboard.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    /// There is no display connection, so there is no selection to own.
    ///
    /// A headless build, or one started outside a session.
    #[error("there is no display to put a clipboard on")]
    NoDisplay,

    /// The main loop did not run the job. Everything here happens on the GTK thread.
    #[error("the user interface thread did not answer within {} ms", MAIN_LOOP_DEADLINE.as_millis())]
    NoMainLoop,

    /// The compositor did not take the text, and said nothing about why.
    ///
    /// Almost always the focus rule at the top of this module: the panel did not have the
    /// keyboard at the moment of the write. The window's own focus state is carried along so
    /// that the log line can say which of the two it was.
    #[error(
        "the compositor did not take the text; on Wayland only the window holding the keyboard may set the clipboard, and whether this application was holding it: {}",
        if *focused { "yes" } else { "no" }
    )]
    NotTaken {
        /// Whether any window of this application had the keyboard when the write went out.
        focused: bool,
    },

    /// Pasting is not built on Linux yet, so there is nothing to offer and nothing to hold.
    ///
    /// Not a failure to report to a user: `platform::DELIVERY` says this platform hands a
    /// dictation over on the clipboard, so nothing asks for an offer in the first place.
    #[error("this build does not paste on Linux, so it makes no clipboard promises either")]
    NoPaste,
}

/// The clipboard, and the three things this application does with it.
///
/// Every method hands its work to the GTK main loop and waits for an answer, because a GTK
/// clipboard belongs to the thread that runs the loop and every caller here is on another
/// one. **They block**, for up to [`MAIN_LOOP_DEADLINE`].
#[derive(Debug)]
pub struct Clipboard {
    /// How many times the selection has changed hands since this application started.
    ///
    /// Counted on the GTK thread and read from anywhere: this is the receipt. A write that
    /// the compositor accepted moves it; a write it threw away does not.
    changes: Arc<AtomicU64>,
}

impl Clipboard {
    /// Start the clipboard agent.
    ///
    /// Nothing is started, in the Windows sense: there is no hidden window and no thread of
    /// its own, because the loop that owns the selection is the one this application already
    /// runs. What this does is check that there is a display at all and subscribe to the
    /// compositor's `owner-change`, which is what every later write is checked against.
    ///
    /// # Errors
    ///
    /// [`ClipboardError::NoDisplay`] when there is no display connection, and
    /// [`ClipboardError::NoMainLoop`] when the user interface thread never ran the job.
    pub fn start() -> Result<Clipboard, ClipboardError> {
        let changes = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&changes);

        let started = on_gtk(move |answer| {
            if gdk::Display::default().is_none() {
                let _ = answer.send(Err(ClipboardError::NoDisplay));
                return;
            }
            // `gtk::Clipboard::get` returns the one clipboard of this display, so this
            // subscribes once for the life of the process. The handler id is not a guard —
            // dropping it does not disconnect — and this subscription is meant to outlive
            // every caller, so there is nothing to keep.
            let _ = clipboard().connect_local("owner-change", false, move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                None
            });
            let _ = answer.send(Ok(()));
        })?;
        started?;

        Ok(Clipboard { changes })
    }

    /// What the clipboard holds as text right now.
    ///
    /// **Not a question for the compositor when this application is the owner.** GDK answers
    /// from its own copy in that case, which is exactly what makes it useless as a check on a
    /// write — see the module documentation. It is a real read of somebody else's selection.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        on_gtk(|answer| {
            let answer = answer.clone();
            // The asynchronous form on purpose: `wait_for_text` runs a main loop inside the
            // main loop, and re-entering Tauri's event loop from a clipboard read is a way to
            // find out which of its handlers were not written for that.
            clipboard().request_text(move |_, text| {
                let _ = answer.send(text.map(|text| text.to_string()));
            });
        })
        .ok()
        .flatten()
    }

    /// Put text on the clipboard and leave it there.
    ///
    /// Writes, then waits for the compositor to say it took it. See the module documentation
    /// for why the waiting is the whole of the honesty here.
    ///
    /// # Errors
    ///
    /// [`ClipboardError::NotTaken`] when the selection never changed hands, which on Wayland
    /// means this application did not have the keyboard; [`ClipboardError::NoMainLoop`] when
    /// the user interface thread never ran the job.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let text = text.to_owned();
        let changes = Arc::clone(&self.changes);

        on_gtk(move |answer| {
            let before = changes.load(Ordering::SeqCst);
            let focused = anything_here_has_the_keyboard();
            clipboard().set_text(&text);

            // The receipt. `timeout_add_local` rather than a sleep, because the event this is
            // waiting for is delivered by the very loop a sleep would be holding up.
            let answer = answer.clone();
            let deadline = Instant::now() + RECEIPT_WINDOW;
            let mut sent = false;
            glib::timeout_add_local(RECEIPT_TICK, move || {
                if sent {
                    return glib::ControlFlow::Break;
                }
                if changes.load(Ordering::SeqCst) > before {
                    sent = true;
                    let _ = answer.send(Ok(()));
                    return glib::ControlFlow::Break;
                }
                if Instant::now() >= deadline {
                    sent = true;
                    let _ = answer.send(Err(ClipboardError::NotTaken { focused }));
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        })?
    }

    /// Offer text as a delayed render, with a receipt for when it is taken.
    ///
    /// # Errors
    ///
    /// Always [`ClipboardError::NoPaste`]. A delayed render is a promise made to a window
    /// that is about to be sent a paste chord, and on Linux there is neither — `platform`'s
    /// `DELIVERY` says as much, so nothing reaches this. GTK has the mechanism
    /// (`gtk_clipboard_set_with_data`) for the package that builds the other half.
    pub fn offer(&self, text: &str) -> Result<Receipt, ClipboardError> {
        let _ = text;
        Err(ClipboardError::NoPaste)
    }

    /// Give the user's own clipboard back.
    ///
    /// # Errors
    ///
    /// The same as [`Clipboard::set_text`], which is what this is. Unreachable today for the
    /// same reason [`Clipboard::offer`] is: nothing takes the clipboard away, because the
    /// copy this platform does is one the user asked for and keeps.
    pub fn restore(&self, previous: Option<String>) -> Result<(), ClipboardError> {
        match previous {
            Some(text) => self.set_text(&text),
            // Clearing rather than writing an empty string: an empty selection and a
            // selection of nothing at all are different things to whatever pastes next.
            None => on_gtk(|answer| {
                clipboard().clear();
                let _ = answer.send(());
            }),
        }
    }
}

/// A promise that a piece of text will be handed over when somebody pastes.
///
/// Never handed out on this platform: [`Clipboard::offer`] fails, so nothing reaches the
/// point of making a promise.
#[derive(Clone, Debug)]
pub struct Receipt {
    never: std::convert::Infallible,
}

impl Receipt {
    /// Wait until the text is handed over, or until the deadline passes.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let _ = timeout;
        match self.never {}
    }
}

/// This display's clipboard — the `CLIPBOARD` selection, not the middle-click one.
fn clipboard() -> gtk::Clipboard {
    gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD)
}

/// Whether any window of this application has the keyboard.
///
/// Asked of the toolkit rather than of a window handle this module does not have: the panel
/// and the settings window are the only two, and the question is about the client rather than
/// about either of them. Used for the sentence on [`ClipboardError::NotTaken`], never to
/// refuse a write — a write is attempted whatever this says, so that a compositor which one
/// day allows an unfocused one starts working here without a change.
fn anything_here_has_the_keyboard() -> bool {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
        .any(|window| window.is_active())
}

/// Run `job` on the thread the GTK main loop is on, and wait for its answer.
///
/// The job is handed a channel rather than returning a value, because two of the three
/// callers answer from a later turn of the loop rather than from this one.
///
/// **The caller may already be that thread.** Tauri runs a synchronous command on the main
/// loop, so a job posted from one would wait for a loop that is waiting for it. In that case
/// the loop is turned here instead, which is what GTK's own blocking calls do.
fn on_gtk<T, F>(job: F) -> Result<T, ClipboardError>
where
    T: Send + 'static,
    F: FnOnce(&mpsc::SyncSender<T>) + Send + 'static,
{
    let (answer, answered) = mpsc::sync_channel::<T>(1);
    let deadline = Instant::now() + MAIN_LOOP_DEADLINE;

    if glib::MainContext::default().is_owner() {
        job(&answer);
        drop(answer);
        loop {
            if let Ok(value) = answered.try_recv() {
                return Ok(value);
            }
            if Instant::now() >= deadline {
                return Err(ClipboardError::NoMainLoop);
            }
            // Without blocking: there may be nothing pending at all yet, and the receipt
            // this is waiting for arrives on a timer rather than on an event.
            let _pending = gtk::main_iteration_do(false);
            std::thread::sleep(RECEIPT_TICK);
        }
    }

    glib::idle_add_once(move || job(&answer));
    answered
        .recv_timeout(MAIN_LOOP_DEADLINE)
        .map_err(|_| ClipboardError::NoMainLoop)
}

#[cfg(test)]
mod tests {
    use super::{ClipboardError, MAIN_LOOP_DEADLINE, RECEIPT_TICK, RECEIPT_WINDOW};

    #[test]
    fn a_refused_write_says_which_of_the_two_reasons_it_was() {
        // The sentence is the whole value of this variant: "the clipboard failed" sends
        // somebody to look at their clipboard manager, and the real answer is that a window
        // did not have the keyboard.
        let unfocused = ClipboardError::NotTaken { focused: false }.to_string();
        assert!(unfocused.ends_with("no"), "{unfocused}");
        assert!(unfocused.contains("keyboard"), "{unfocused}");

        let focused = ClipboardError::NotTaken { focused: true }.to_string();
        assert!(focused.ends_with("yes"), "{focused}");
        assert_ne!(focused, unfocused);
    }

    #[test]
    fn a_caller_waits_for_longer_than_the_receipt_it_is_waiting_on() {
        // Otherwise every refused write would be reported as a main loop that never answered,
        // which is a different problem in a different place.
        assert!(
            MAIN_LOOP_DEADLINE > RECEIPT_WINDOW,
            "the deadline must outlast the receipt it contains"
        );
        assert!(RECEIPT_TICK < RECEIPT_WINDOW);
    }

    #[test]
    fn no_paste_is_not_dressed_up_as_a_clipboard_failure() {
        // `offer` failing means this build does not paste, which is a fact about the build
        // rather than about the machine it is running on.
        let message = ClipboardError::NoPaste.to_string();
        assert!(message.contains("paste"), "{message}");
        assert!(!message.contains("compositor"), "{message}");
    }
}
