//! The clipboard, with a receipt.
//!
//! `docs/PROJECT.md` §3 asks for a paste that knows when it has happened: "paste receipt via
//! delayed render, never a fixed delay". This module is that receipt.
//!
//! ## Why not a library
//!
//! `arboard` (MIT/Apache-2.0) is the obvious crate and was evaluated first. It cannot do this:
//! its API sets clipboard *contents*, and delayed rendering is not contents — it is a promise
//! with an owner window behind it, and the owner has to be alive and pumping messages when the
//! promise is called in. Nothing in `arboard` exposes a window, so the only thing it could
//! offer is `set_text` followed by a sleep, which is the fixed delay the decision rules out.
//! The Win32 calls underneath are six functions; the wrapper would have been the larger half.
//!
//! ## How the receipt works
//!
//! ```text
//!   offer("…")                    target presses Ctrl+V              wait() returns true
//!       │                                  │                                  │
//!       ▼                                  ▼                                  ▼
//!   OpenClipboard(own window)        Windows sends WM_RENDERFORMAT      restore the
//!   EmptyClipboard()          ──▶    to the owner ──▶ SetClipboardData  previous text
//!   SetClipboardData(CF_UNICODETEXT, NULL)           (the real text)
//! ```
//!
//! The null handle is a promise: *I own this format, ask me when somebody wants it.* Nobody
//! pays for the text until a paste actually happens, and the moment one does, this process is
//! told. That is the difference between knowing the paste landed and waiting a second and
//! hoping — and it is what makes restoring the user's own clipboard safe, because the restore
//! happens after the text has been handed over rather than after a guess.
//!
//! ## What is restored, and what is not
//!
//! **CF_UNICODETEXT, byte for byte, and nothing else.** A clipboard holding an image, a file
//! list, rich text or a spreadsheet range keeps none of that through a Dile paste: this
//! reads the text format before the offer and writes the text format back afterwards, and
//! `EmptyClipboard` in between takes everything. It is written down here and in
//! `docs/PROJECT.md` §3 rather than discovered, because the honest version of a v1 limit is
//! the documented one.
//!
//! ## One thread, because the clipboard belongs to one
//!
//! `OpenClipboard` wants a window owned by the calling thread, and `WM_RENDERFORMAT` is
//! delivered to the thread that owns the window. So every call in this module happens on one
//! thread with a message-only window and a message loop, and the rest of the application
//! talks to it through a queue. That also keeps a clipboard that another process is holding
//! open from blocking a dictation: the retry loop spins on that thread and nowhere else.

// The module documentation in `mod.rs` explains why this is here and why it is only here.
#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_DESTROYCLIPBOARD, WM_RENDERALLFORMATS, WM_RENDERFORMAT, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

/// The clipboard format this application reads and writes: plain UTF-16 text.
///
/// Spelled out rather than imported, because the constant lives behind a feature of the
/// `windows` crate nothing else here needs. Its value has been 13 since Windows NT 3.1.
const CF_UNICODETEXT: u32 = 13;

/// The message the agent thread posts to itself when there is work in the queue.
///
/// `WM_APP` is the range reserved for an application's own messages, so nothing else on the
/// machine can mean anything by it.
const WM_DILE_JOB: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// The window class of the hidden owner window.
const CLASS_NAME: PCWSTR = w!("DileClipboardOwner");

/// How long a clipboard that somebody else has open is waited for.
///
/// `OpenClipboard` fails outright while another process holds it, and something always does
/// for a few milliseconds — a clipboard manager, a browser, the shell. Half a second of
/// retrying turns that into a non-event; longer than that is a paste the user has given up on.
const OPEN_TIMEOUT: Duration = Duration::from_millis(500);

/// How often that retry loop re-asks.
const OPEN_POLL: Duration = Duration::from_millis(10);

/// What can go wrong with a clipboard.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    /// The owner window or its thread could not be created.
    #[error("the clipboard window could not be created: {0}")]
    Start(String),
    /// The agent thread has stopped, so nothing can be asked of it any more.
    #[error("the clipboard thread is not running")]
    NotRunning,
    /// Windows refused an operation.
    #[error("the clipboard refused: {0}")]
    Refused(String),
    /// A global memory block would not lock, so nothing could be copied into it.
    #[error("the clipboard block could not be locked")]
    Locked,
}

/// A promise that a piece of text will be handed over when somebody pastes.
///
/// Dropping it does not withdraw the promise — the clipboard still holds it, and the next
/// [`Clipboard::offer`], [`Clipboard::set_text`] or [`Clipboard::restore`] replaces it.
#[derive(Clone, Debug)]
pub struct Receipt {
    signal: Arc<Signal>,
}

impl Receipt {
    /// Wait until the text is handed over, or until the deadline passes.
    ///
    /// `true` means a paste actually took the text. `false` means the wait ran out, which is
    /// worth a log line and not an error: the target may simply not have pasted.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let Ok(mut rendered) = self.signal.rendered.lock() else {
            return false;
        };
        while !*rendered {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            let Ok((next, _)) = self.signal.woken.wait_timeout(rendered, left) else {
                return false;
            };
            rendered = next;
        }
        true
    }
}

/// The flag one offer is waited on through.
#[derive(Debug, Default)]
struct Signal {
    rendered: Mutex<bool>,
    woken: Condvar,
}

impl Signal {
    fn raise(&self) {
        if let Ok(mut rendered) = self.rendered.lock() {
            *rendered = true;
        }
        self.woken.notify_all();
    }
}

/// What the offer currently on the clipboard is, and who is waiting for it.
#[derive(Debug)]
struct Offer {
    /// The text, already NUL-terminated UTF-16, ready to copy into a global block.
    utf16: Vec<u16>,
    signal: Arc<Signal>,
}

/// Everything the agent thread and its window procedure share.
#[derive(Debug, Default)]
struct Shared {
    /// The promise waiting to be called in, or `None` when this process owns nothing.
    offer: Mutex<Option<Offer>>,
    /// Work posted from other threads.
    jobs: Mutex<VecDeque<Job>>,
}

/// One thing the agent thread is asked to do.
enum Job {
    /// Read CF_UNICODETEXT as it is now.
    Read(Sender<Option<String>>),
    /// Put text on the clipboard outright, with no promise and no receipt.
    Set(String, Sender<Result<(), ClipboardError>>),
    /// Promise text, to be handed over when somebody pastes.
    Offer(String, Arc<Signal>, Sender<Result<(), ClipboardError>>),
    /// Put back what was there before, or empty the clipboard when there was nothing.
    Restore(Option<String>, Sender<Result<(), ClipboardError>>),
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately says nothing about the text: this type carries whatever the user just
        // dictated, and a `{:?}` in a log line would be the one place it leaked.
        let name = match self {
            Job::Read(_) => "read",
            Job::Set(..) => "set",
            Job::Offer(..) => "offer",
            Job::Restore(..) => "restore",
        };
        f.write_str(name)
    }
}

/// The clipboard, as one thread that owns a window.
///
/// Cheap to clone; every clone talks to the same thread. Dropping the last one ends it.
#[derive(Debug)]
pub struct Clipboard {
    shared: Arc<Shared>,
    /// The owner window, as a number: the agent thread owns the handle, and this is how
    /// other threads ask it to look at its queue.
    window: isize,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Clipboard {
    /// Start the agent thread and its hidden window.
    ///
    /// # Errors
    ///
    /// The thread would not start, or Windows refused the window.
    pub fn start() -> Result<Clipboard, ClipboardError> {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread = thread::Builder::new()
            .name("dile-clipboard".to_owned())
            .spawn(move || run(&thread_shared, &ready_tx))
            .map_err(|error| ClipboardError::Start(error.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(window)) => Ok(Clipboard {
                shared,
                window,
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(ClipboardError::NotRunning)
            }
        }
    }

    /// What the clipboard holds as text right now, or `None` when it holds no text.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let (tx, rx) = mpsc::channel();
        self.post(Job::Read(tx)).ok()?;
        rx.recv().ok().flatten()
    }

    /// Put text on the clipboard outright.
    ///
    /// What the panel's **copy** button does. No promise, no receipt and no restore: the user
    /// asked for their clipboard to hold this, so it holds this until they change it.
    ///
    /// # Errors
    ///
    /// Windows would not give up the clipboard, or the agent thread has stopped.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let (tx, rx) = mpsc::channel();
        self.post(Job::Set(text.to_owned(), tx))?;
        rx.recv().map_err(|_| ClipboardError::NotRunning)?
    }

    /// Promise text, and get back the receipt that says when it was taken.
    ///
    /// # Errors
    ///
    /// Windows would not give up the clipboard, or the agent thread has stopped.
    pub fn offer(&self, text: &str) -> Result<Receipt, ClipboardError> {
        let signal = Arc::new(Signal::default());
        let (tx, rx) = mpsc::channel();
        self.post(Job::Offer(text.to_owned(), Arc::clone(&signal), tx))?;
        rx.recv().map_err(|_| ClipboardError::NotRunning)??;
        Ok(Receipt { signal })
    }

    /// Put the clipboard back the way it was.
    ///
    /// `None` means there was no text on it, and the clipboard is emptied rather than left
    /// holding a dictation the user never asked to keep.
    ///
    /// # Errors
    ///
    /// Windows would not give up the clipboard, or the agent thread has stopped.
    pub fn restore(&self, previous: Option<String>) -> Result<(), ClipboardError> {
        let (tx, rx) = mpsc::channel();
        self.post(Job::Restore(previous, tx))?;
        rx.recv().map_err(|_| ClipboardError::NotRunning)?
    }

    /// Queue one job and wake the message loop.
    fn post(&self, job: Job) -> Result<(), ClipboardError> {
        match self.shared.jobs.lock() {
            Ok(mut jobs) => jobs.push_back(job),
            Err(_) => return Err(ClipboardError::NotRunning),
        }
        // SAFETY: the window belongs to the agent thread and lives as long as it does, which
        // is as long as this struct. `PostMessageW` is one of the few calls documented as
        // safe to make against another thread's window.
        unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut core::ffi::c_void)),
                WM_DILE_JOB,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .map_err(|error| ClipboardError::Refused(error.to_string()))
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: as in `post` — a message posted to another thread's window. `DestroyWindow`
        // is deliberately not called from here: it may only be called on the thread that
        // created the window, so the agent does it on its way out.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut core::ffi::c_void)),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let Ok(mut thread) = self.thread.lock()
            && let Some(handle) = thread.take()
        {
            let _ = handle.join();
        }
    }
}

thread_local! {
    /// What the window procedure reaches the rest of this module through.
    ///
    /// A thread-local rather than `GWLP_USERDATA`: the window procedure only ever runs on the
    /// thread that created the window, so this is the same lifetime with none of the pointer
    /// arithmetic — and there is nothing to get wrong when the window is destroyed.
    static AGENT: std::cell::RefCell<Option<Arc<Shared>>> =
        const { std::cell::RefCell::new(None) };
}

/// The agent thread: make a window, then answer messages until told to stop.
fn run(shared: &Arc<Shared>, ready: &Sender<Result<isize, ClipboardError>>) {
    AGENT.with(|agent| *agent.borrow_mut() = Some(Arc::clone(shared)));

    let window = match create_window() {
        Ok(window) => window,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(window.0 as isize)).is_err() {
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is a live local. `GetMessageW` blocks until something arrives and
        // dispatches any *sent* message — WM_RENDERFORMAT among them — while it waits.
        let got = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if got.0 <= 0 {
            break;
        }
        if message.message == WM_DILE_JOB {
            drain(shared, window);
            continue;
        }
        // SAFETY: a message this loop received and has not modified.
        unsafe {
            let _ = DispatchMessageW(&message);
        }
    }

    // SAFETY: called on the thread that created the window, which is this one.
    unsafe {
        let _ = DestroyWindow(window);
    }
    AGENT.with(|agent| *agent.borrow_mut() = None);
}

/// Do everything in the queue.
fn drain(shared: &Arc<Shared>, window: HWND) {
    loop {
        let job = match shared.jobs.lock() {
            Ok(mut jobs) => jobs.pop_front(),
            Err(_) => return,
        };
        let Some(job) = job else { return };

        match job {
            Job::Read(answer) => {
                let _ = answer.send(read_text(window));
            }
            Job::Set(text, answer) => {
                let _ = answer.send(write_text(window, shared, Some(&text)));
            }
            Job::Offer(text, signal, answer) => {
                let _ = answer.send(write_offer(window, shared, &text, signal));
            }
            Job::Restore(previous, answer) => {
                let _ = answer.send(write_text(window, shared, previous.as_deref()));
            }
        }
    }
}

/// Open the clipboard, retrying while somebody else has it.
fn open(window: HWND) -> Result<(), ClipboardError> {
    let deadline = Instant::now() + OPEN_TIMEOUT;
    loop {
        // SAFETY: `window` belongs to this thread, which is what `OpenClipboard` requires of
        // the handle it is given.
        match unsafe { OpenClipboard(Some(window)) } {
            Ok(()) => return Ok(()),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(ClipboardError::Refused(error.to_string()));
                }
                thread::sleep(OPEN_POLL);
            }
        }
    }
}

/// Close the clipboard, which must happen on every path out of an `open`.
fn close() {
    // SAFETY: paired with the `open` above it; closing a clipboard this thread does not have
    // open is an error return rather than a fault.
    unsafe {
        let _ = CloseClipboard();
    }
}

/// Read CF_UNICODETEXT, or `None` when the clipboard holds no text.
fn read_text(window: HWND) -> Option<String> {
    if open(window).is_err() {
        return None;
    }
    // SAFETY: the clipboard is open on this thread until `close` below. `GetClipboardData`
    // returns a handle owned by the clipboard, which must not be freed and must not be used
    // after `CloseClipboard` — both of which hold here, since the copy is made first.
    let text = unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            None
        } else {
            GetClipboardData(CF_UNICODETEXT)
                .ok()
                .and_then(|handle| read_global(HGLOBAL(handle.0)))
        }
    };
    close();
    text
}

/// Copy a NUL-terminated UTF-16 string out of a global memory block.
///
/// # Safety
///
/// `handle` must be a live global memory block holding NUL-terminated UTF-16, which is what
/// the CF_UNICODETEXT contract guarantees of the handle the clipboard hands over.
unsafe fn read_global(handle: HGLOBAL) -> Option<String> {
    if handle.is_invalid() {
        return None;
    }
    // SAFETY: the caller's contract. `GlobalLock` answers null rather than faulting for a
    // handle that cannot be locked, which the check below catches.
    let locked = unsafe { GlobalLock(handle) }.cast::<u16>();
    if locked.is_null() {
        return None;
    }

    let mut units = Vec::new();
    let mut index = 0isize;
    loop {
        // SAFETY: the block is NUL-terminated by the format's contract, so this walk stops
        // inside it. A block that is not would be a clipboard entry no program could read.
        let unit = unsafe { *locked.offset(index) };
        if unit == 0 {
            break;
        }
        units.push(unit);
        index += 1;
    }
    // SAFETY: paired with the lock above. The documented failure — the count reaching zero —
    // is reported as an error, which is why the result is dropped.
    unsafe {
        let _ = GlobalUnlock(handle);
    }

    Some(String::from_utf16_lossy(&units))
}

/// A global memory block holding `text` as NUL-terminated UTF-16.
///
/// The caller hands the block to `SetClipboardData`, after which it belongs to the system.
fn global_from(utf16: &[u16]) -> Result<HGLOBAL, ClipboardError> {
    let bytes = utf16.len().saturating_mul(size_of::<u16>());
    // SAFETY: `GMEM_MOVEABLE` is what CF_UNICODETEXT requires of the block it is given.
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }
        .map_err(|error| ClipboardError::Refused(error.to_string()))?;

    // SAFETY: the handle was allocated with at least `bytes` bytes one line above, so the
    // copy stays inside it.
    unsafe {
        let locked = GlobalLock(handle).cast::<u16>();
        if locked.is_null() {
            return Err(ClipboardError::Locked);
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), locked, utf16.len());
        let _ = GlobalUnlock(handle);
    }
    Ok(handle)
}

/// `text` as UTF-16 with the NUL the clipboard format requires.
fn utf16_of(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Put text on the clipboard outright, withdrawing any promise this process was holding.
fn write_text(
    window: HWND,
    shared: &Arc<Shared>,
    text: Option<&str>,
) -> Result<(), ClipboardError> {
    // Dropped before the clipboard is touched: once `EmptyClipboard` runs, a WM_RENDERFORMAT
    // for the old promise can no longer arrive, and a stale offer left here would be handed
    // over on the next paste.
    if let Ok(mut offer) = shared.offer.lock() {
        *offer = None;
    }

    open(window)?;
    let written = (|| {
        // SAFETY: the clipboard is open on this thread. `EmptyClipboard` makes this process
        // its owner, which is what `SetClipboardData` then requires.
        unsafe { EmptyClipboard() }.map_err(|error| ClipboardError::Refused(error.to_string()))?;
        let Some(text) = text else {
            return Ok(());
        };
        let block = global_from(&utf16_of(text))?;
        // SAFETY: the block was allocated for this call and, once the call succeeds, belongs
        // to the system — which is why nothing frees it afterwards.
        unsafe { SetClipboardData(CF_UNICODETEXT, Some(HANDLE(block.0))) }
            .map(|_| ())
            .map_err(|error| ClipboardError::Refused(error.to_string()))
    })();
    close();
    written
}

/// Promise text: own the format, hand nothing over until somebody asks.
fn write_offer(
    window: HWND,
    shared: &Arc<Shared>,
    text: &str,
    signal: Arc<Signal>,
) -> Result<(), ClipboardError> {
    let offer = Offer {
        utf16: utf16_of(text),
        signal,
    };

    open(window)?;
    let written = (|| {
        // SAFETY: the clipboard is open on this thread, and emptying it makes this process
        // the owner — which is the whole point: only the owner is asked to render.
        unsafe { EmptyClipboard() }.map_err(|error| ClipboardError::Refused(error.to_string()))?;

        // **After `EmptyClipboard`, and that is not a detail.** Emptying the clipboard sends
        // WM_DESTROYCLIPBOARD to the *previous* owner, synchronously, on this thread — and
        // when a second dictation follows a first, the previous owner is this process. The
        // handler for that message drops the pending offer, which is right when somebody else
        // took the clipboard and catastrophic one line too early: the promise would be
        // installed and then immediately forgotten, so the paste that asked for it would be
        // answered with nothing. Seen live on 2026-09-13, in exactly that shape.
        match shared.offer.lock() {
            Ok(mut held) => *held = Some(offer),
            Err(_) => return Err(ClipboardError::NotRunning),
        }
        // SAFETY: a null handle is the documented way to say "I will supply this format when
        // it is wanted". The promise is kept by the window procedure below.
        //
        // **The last error has to be cleared first, and the reason is the whole trick.**
        // `SetClipboardData` answers with the handle it was given, so an offer succeeds by
        // returning *null* — which no wrapper can tell apart from a failure by the return
        // value alone, so it asks Windows what went wrong. Windows has not been asked to
        // record anything, and answers with whatever error some earlier call left lying
        // about. Zeroing it first makes the question meaningful: a null return with a last
        // error of zero is a promise that was accepted. Both halves were seen live on
        // 2026-09-13, first as `0x00000000` and then, after only the second half existed, as
        // `ERROR_INVALID_HANDLE` from a call that had in fact worked.
        //
        // SAFETY: `SetLastError` writes one thread-local value and cannot fail.
        unsafe { SetLastError(WIN32_ERROR(0)) };
        match unsafe { SetClipboardData(CF_UNICODETEXT, None) } {
            Ok(_) => Ok(()),
            Err(error) if error.code().is_ok() => Ok(()),
            Err(error) => Err(ClipboardError::Refused(error.to_string())),
        }
    })();
    close();

    if written.is_err()
        && let Ok(mut held) = shared.offer.lock()
    {
        *held = None;
    }
    written
}

/// Hand the promised text over, from inside `WM_RENDERFORMAT`.
///
/// **The clipboard must not be opened here.** The owner is being asked to render *while*
/// somebody else has the clipboard open; calling `OpenClipboard` would deadlock. The one
/// exception is `WM_RENDERALLFORMATS`, which arrives when this window is about to be
/// destroyed and nobody else holds it — see the window procedure.
fn render(shared: &Shared) {
    let Ok(offer) = shared.offer.lock() else {
        return;
    };
    let Some(offer) = offer.as_ref() else {
        return;
    };
    let Ok(block) = global_from(&offer.utf16) else {
        return;
    };
    // SAFETY: inside WM_RENDERFORMAT the clipboard is already open on this thread's behalf,
    // which is exactly the state `SetClipboardData` needs and the reason nothing opens it
    // here. The block belongs to the system once the call succeeds.
    let handed = unsafe { SetClipboardData(CF_UNICODETEXT, Some(HANDLE(block.0))) };
    if handed.is_ok() {
        offer.signal.raise();
    }
}

/// Create the hidden owner window on this thread.
fn create_window() -> Result<HWND, ClipboardError> {
    // SAFETY: `GetModuleHandleW(None)` names this executable and cannot fail for it.
    let instance = unsafe { GetModuleHandleW(None) }
        .map_err(|error| ClipboardError::Start(error.to_string()))?;

    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        lpszClassName: CLASS_NAME,
        ..WNDCLASSW::default()
    };
    // SAFETY: every pointer in `class` is either null or a string literal with a static
    // lifetime. Registering a class that already exists returns 0 and sets an error, which
    // the window creation below then either survives or reports.
    unsafe {
        RegisterClassW(&raw const class);
    }

    // SAFETY: `HWND_MESSAGE` as the parent is the documented way to ask for a message-only
    // window: it is never shown, never painted and never in the z-order, which is what a
    // clipboard owner should be.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            CLASS_NAME,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|error| ClipboardError::Start(error.to_string()))
}

/// The owner window's procedure: three clipboard messages and nothing else.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Somebody is pasting. Supply the text, without opening the clipboard.
        WM_RENDERFORMAT if wparam.0 as u32 == CF_UNICODETEXT => {
            AGENT.with(|agent| {
                if let Some(shared) = agent.borrow().as_ref() {
                    render(shared);
                }
            });
            LRESULT(0)
        }

        // This window is going away while it still owes the clipboard a format. Here — and
        // only here — the clipboard has to be opened, because nobody else has it.
        WM_RENDERALLFORMATS => {
            AGENT.with(|agent| {
                if let Some(shared) = agent.borrow().as_ref()
                    && open(window).is_ok()
                {
                    // SAFETY: the clipboard is open on this thread, and the owner check is
                    // what the documentation requires before rendering into it.
                    let ours = unsafe { GetClipboardOwner() }.is_ok_and(|owner| owner == window);
                    if ours {
                        render(shared);
                    }
                    close();
                }
            });
            LRESULT(0)
        }

        // Somebody else took the clipboard, so the promise is void. Dropping it here stops a
        // later paste from being handed a dictation that is no longer on the clipboard.
        WM_DESTROYCLIPBOARD => {
            AGENT.with(|agent| {
                if let Some(shared) = agent.borrow().as_ref()
                    && let Ok(mut offer) = shared.offer.lock()
                {
                    *offer = None;
                }
            });
            LRESULT(0)
        }

        windows::Win32::UI::WindowsAndMessaging::WM_CLOSE => {
            // SAFETY: posting a quit to this thread's own message loop, which is the loop in
            // `run` above; it ends and destroys the window there.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }

        // SAFETY: the default handler, called with the arguments it was given.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::{Clipboard, utf16_of};
    use std::time::Duration;

    #[test]
    fn text_reaches_the_clipboard_as_nul_terminated_utf16() {
        let units = utf16_of("güzel");
        assert_eq!(units.last(), Some(&0), "CF_UNICODETEXT is NUL-terminated");
        assert_eq!(String::from_utf16_lossy(&units[..units.len() - 1]), "güzel");

        // Turkish outside the BMP is not a thing, but an emoji in a dictation is: it has to
        // survive as a surrogate pair rather than as one lost unit.
        let pair = utf16_of("🙂");
        assert_eq!(pair.len(), 3, "two surrogates and a terminator");
    }

    /// The whole round trip, against the real Windows clipboard.
    ///
    /// `#[ignore]` because it takes the machine's clipboard for the length of the test, which
    /// is rude on a developer's desktop and meaningless on a runner with no interactive
    /// session. Run it by hand:
    ///
    /// ```text
    /// cargo test -p dile-app --lib clipboard -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "takes the machine's clipboard"]
    fn the_clipboard_round_trips_and_gives_the_previous_text_back() {
        let clipboard = Clipboard::start().expect("the clipboard agent starts");

        clipboard.set_text("dile önceki").expect("a plain set");
        assert_eq!(clipboard.text().as_deref(), Some("dile önceki"));

        let previous = clipboard.text();
        let receipt = clipboard.offer("dile test").expect("an offer");
        assert!(
            !receipt.wait(Duration::ZERO),
            "nothing has pasted yet, so the promise is still outstanding"
        );
        // Reading it is a paste as far as the clipboard is concerned: the promise is called
        // in, which is exactly what the receipt is for.
        assert_eq!(clipboard.text().as_deref(), Some("dile test"));
        assert!(
            receipt.wait(Duration::from_secs(2)),
            "the promise was taken"
        );

        clipboard.restore(previous).expect("a restore");
        assert_eq!(clipboard.text().as_deref(), Some("dile önceki"));
    }
}
