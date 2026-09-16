//! Windows, monitors and the foreground, in four questions and two answers.
//!
//! The questions: *which window has the focus*, *what program is that*, *which monitor is it
//! on*, *does this window still exist*. The answers: *give that window the focus back* and
//! *send a key chord to whatever has it*. That is the whole of the platform surface WP5b's
//! panel needs, and it is deliberately small — the panel's own window is created and moved
//! through Tauri, which already wraps every one of those calls safely.
//!
//! **Handles do not leave this module as handles.** [`Hwnd`] is an `isize` with a name on it,
//! so the session thread can carry the target window across a channel without carrying a raw
//! pointer, and a stale one is a number that [`Hwnd::exists`] answers *no* about rather than
//! a pointer somebody could dereference.
//!
//! **The foreground dance is a Windows rule, not a trick.** `SetForegroundWindow` refuses a
//! process that has not recently been given input by the user, which is exactly Dile's
//! situation: the user pressed a key that a low-level hook swallowed, so as far as the
//! foreground lock is concerned nobody typed anything. The documented way through is to share
//! the input queue with the thread that currently owns the foreground — `AttachThreadInput`
//! — for the length of the call, and to say `AllowSetForegroundWindow` first. See
//! [`Hwnd::focus`], which does both and then *checks*, because a refused call returns
//! success on some Windows builds and the only honest test is to ask who is in front
//! afterwards.

// The module documentation in `mod.rs` explains why this is here and why it is only here.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::platform::screen::{Monitor, Rect};
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MONITORINFOEXW, MonitorFromWindow,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW,
    GetWindowThreadProcessId, IsWindow, SetForegroundWindow, WS_EX_NOACTIVATE,
};
use windows::core::PWSTR;

/// `AllowSetForegroundWindow`'s "any process may take the foreground from me".
///
/// Named here rather than imported because the constant lives behind a feature of the
/// `windows` crate this application has no other use for.
const ASFW_ANY: u32 = u32::MAX;

/// How long [`Hwnd::focus`] waits for Windows to agree that the target is in front.
///
/// A foreground change is asynchronous: `SetForegroundWindow` returns before the window
/// manager has finished, and a paste sent into the gap lands in the wrong window. A fifth of
/// a second is far longer than the switch takes and far shorter than a person notices.
const FOCUS_TIMEOUT: Duration = Duration::from_millis(250);

/// How often that wait re-asks.
const FOCUS_POLL: Duration = Duration::from_millis(10);

/// A window handle, as a number that can cross a thread boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hwnd(isize);

impl Hwnd {
    /// The window the user is working in, or `None` when nothing has the focus.
    #[must_use]
    pub fn foreground() -> Option<Hwnd> {
        // SAFETY: `GetForegroundWindow` takes no arguments, touches no memory this process
        // owns and is callable from any thread. A null result is the documented "no window
        // has the focus" and is filtered out below.
        let handle = unsafe { GetForegroundWindow() };
        Hwnd::from_raw(handle)
    }

    /// Wrap a raw handle, treating null as "no window".
    fn from_raw(handle: HWND) -> Option<Hwnd> {
        let raw = handle.0 as isize;
        (!handle.is_invalid()).then_some(Hwnd(raw))
    }

    /// Wrap the number a window toolkit hands back for one of its own windows.
    ///
    /// Tauri answers `WebviewWindow::hwnd` with a handle; this is how that becomes something
    /// the rest of this application can hold without holding a pointer.
    #[must_use]
    pub const fn from_isize(handle: isize) -> Option<Hwnd> {
        if handle == 0 {
            None
        } else {
            Some(Hwnd(handle))
        }
    }

    /// The raw handle this stands for.
    const fn raw(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }

    /// The handle as a plain number, for a log line or a settings key.
    #[must_use]
    pub const fn as_isize(self) -> isize {
        self.0
    }

    /// Whether this window is still there.
    ///
    /// A target captured when the recording started may be gone by the time the dictation
    /// comes back — the user closed the tab, quit the editor, or the application crashed —
    /// and pasting into a handle that has been reused by another window is the one mistake
    /// this product must not make.
    #[must_use]
    pub fn exists(self) -> bool {
        // SAFETY: `IsWindow` is documented to accept any value, including a handle that has
        // been destroyed, and to answer rather than fault. That is the whole reason it is
        // called here.
        unsafe { IsWindow(Some(self.raw())) }.as_bool()
    }

    /// Whether this window is the one in front right now.
    #[must_use]
    pub fn is_foreground(self) -> bool {
        Hwnd::foreground() == Some(self)
    }

    /// The full path of the program this window belongs to.
    ///
    /// `PROCESS_QUERY_LIMITED_INFORMATION` rather than `PROCESS_QUERY_INFORMATION`: it is the
    /// right that exists for this question, and it works against a process running at a
    /// higher integrity level, which `QUERY_INFORMATION` does not. A window Dile cannot open
    /// is a window with no label, not a reason to fail.
    #[must_use]
    pub fn process_path(self) -> Option<PathBuf> {
        let mut pid = 0u32;
        // SAFETY: the out-parameter is a live `u32` on this stack for the length of the call,
        // and the handle is only read. A dead window gives thread 0 and leaves `pid` as it
        // was, which the check below catches.
        let thread = unsafe { GetWindowThreadProcessId(self.raw(), Some(&mut pid)) };
        if thread == 0 || pid == 0 {
            return None;
        }

        // SAFETY: a plain open with the narrowest right that answers this question. The
        // handle is closed on every path below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;

        let mut buffer = [0u16; 512];
        let mut length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: `buffer` is a live array of `length` UTF-16 units, and `length` is its true
        // capacity; the call writes at most that many and stores the real count back.
        let queried = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        // SAFETY: `process` came from `OpenProcess` above and is not used again.
        let _ = unsafe { CloseHandle(process) };
        queried.ok()?;

        let used = usize::try_from(length).unwrap_or(0).min(buffer.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..used])))
    }

    /// The display this window is mostly on.
    #[must_use]
    pub fn monitor(self) -> Option<Monitor> {
        // SAFETY: `MonitorFromWindow` accepts any handle and answers with the nearest
        // monitor rather than failing, which is what `DEFAULTTONEAREST` asks for.
        let monitor = unsafe { MonitorFromWindow(self.raw(), MONITOR_DEFAULTTONEAREST) };
        monitor_info(monitor)
    }

    /// Put this window back in front, and say whether it worked.
    ///
    /// The documented dance, in order:
    ///
    /// 1. `AllowSetForegroundWindow(ASFW_ANY)` — this is Dile giving *up* its own claim on
    ///    the foreground rather than taking one; it is what lets the target's process be
    ///    raised when the request is routed through it.
    /// 2. `AttachThreadInput` to the thread that owns the current foreground window, so this
    ///    thread shares its input queue for the length of the call. Without it the
    ///    foreground lock refuses a process the user has not recently typed into — and Dile
    ///    is always that process, because its own chord was swallowed by a low-level hook.
    /// 3. `SetForegroundWindow`, then detach whatever was attached, whatever happened.
    /// 4. **Ask again.** The switch is asynchronous and the call has been known to report
    ///    success while quietly doing nothing, so the return value here is the answer to
    ///    "who is in front now", not to "did the call return true".
    #[must_use]
    pub fn focus(self) -> bool {
        if self.is_foreground() {
            return true;
        }
        if !self.exists() {
            return false;
        }

        // SAFETY: no memory is shared with any of these calls. `AttachThreadInput` is
        // undone on every path out, including the one where it never succeeded, because
        // detaching a pair that was never attached is a no-op rather than an error.
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);

            let ours = GetCurrentThreadId();
            let front = GetForegroundWindow();
            let theirs = if front.is_invalid() {
                0
            } else {
                GetWindowThreadProcessId(front, None)
            };
            let attached =
                theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();

            let _ = SetForegroundWindow(self.raw());

            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
        }

        let deadline = Instant::now() + FOCUS_TIMEOUT;
        while Instant::now() < deadline {
            if self.is_foreground() {
                return true;
            }
            std::thread::sleep(FOCUS_POLL);
        }
        self.is_foreground()
    }

    /// The extended window style bits, for the one thing that has to be verified by reading.
    ///
    /// The panel's whole design rests on `WS_EX_NOACTIVATE` being set on it, and that is not
    /// something a test can press a key to check. [`Hwnd::is_non_activating`] reads it back
    /// off the window instead.
    #[must_use]
    pub fn extended_style(self) -> isize {
        // SAFETY: a read of one window property. An invalid handle answers 0 and sets the
        // last error, which is the same thing this returns.
        unsafe { GetWindowLongPtrW(self.raw(), GWL_EXSTYLE) }
    }

    /// Whether this window refuses the focus.
    #[must_use]
    pub fn is_non_activating(self) -> bool {
        let wanted = isize::try_from(WS_EX_NOACTIVATE.0).unwrap_or_default();
        self.extended_style() & wanted != 0
    }
}

/// The primary display, for when there is no window to ask about.
#[must_use]
pub fn primary_monitor() -> Option<Monitor> {
    // SAFETY: a null window with `DEFAULTTOPRIMARY` is the documented way to name the
    // primary monitor without holding a handle to anything.
    let monitor = unsafe { MonitorFromWindow(HWND::default(), MONITOR_DEFAULTTOPRIMARY) };
    monitor_info(monitor)
}

/// Fill in one [`Monitor`] from a handle the caller has already resolved.
fn monitor_info(monitor: HMONITOR) -> Option<Monitor> {
    if monitor.is_invalid() {
        return None;
    }

    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: u32::try_from(size_of::<MONITORINFOEXW>()).ok()?,
            ..MONITORINFO::default()
        },
        ..Default::default()
    };
    // SAFETY: `MONITORINFOEXW` begins with the `MONITORINFO` the call is typed against —
    // that is the documented contract, and `cbSize` above is what tells the call which of
    // the two it was handed. The pointer is to a live local for the length of the call.
    let filled =
        unsafe { GetMonitorInfoW(monitor, std::ptr::from_mut(&mut info).cast::<MONITORINFO>()) };
    if !filled.as_bool() {
        return None;
    }

    let device = String::from_utf16_lossy(&info.szDevice);
    Some(Monitor {
        device: device.trim_end_matches('\0').to_owned(),
        bounds: rect(info.monitorInfo.rcMonitor),
        work: rect(info.monitorInfo.rcWork),
    })
}

fn rect(from: windows::Win32::Foundation::RECT) -> Rect {
    Rect {
        left: from.left,
        top: from.top,
        right: from.right,
        bottom: from.bottom,
    }
}

/// Type the paste chord into whatever has the focus.
///
/// `Ctrl+V`, or `Ctrl+Shift+V` for the terminal family — which application gets which is
/// `paste.rs`'s decision and not this module's.
///
/// **Stuck modifiers are released first.** Dile's own chord is `Ctrl+Alt+Space` by default,
/// and a transfer can happen while the user still has a finger on it: an Alt that is
/// physically down turns `Ctrl+V` into `Ctrl+Alt+V`, which is a different command in every
/// editor there is. Only the keys that would spoil this chord are released, and only when
/// Windows says they are actually down — a synthetic key-up for a key nobody is holding is a
/// stray event in somebody else's application.
pub fn send_paste_chord(with_shift: bool) -> bool {
    send_chord(with_shift, VK_V)
}

/// Send `Ctrl+key`, or `Ctrl+Shift+key`, to whatever has the focus.
fn send_chord(with_shift: bool, key: VIRTUAL_KEY) -> bool {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(10);

    for held in [VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN] {
        if is_down(held) {
            inputs.push(key_event(held, true));
        }
    }
    if !with_shift {
        for held in [VK_LSHIFT, VK_RSHIFT] {
            if is_down(held) {
                inputs.push(key_event(held, true));
            }
        }
    }

    inputs.push(key_event(VK_CONTROL, false));
    if with_shift {
        inputs.push(key_event(VK_SHIFT, false));
    }
    inputs.push(key_event(key, false));
    inputs.push(key_event(key, true));
    if with_shift {
        inputs.push(key_event(VK_SHIFT, true));
    }
    inputs.push(key_event(VK_CONTROL, true));

    let count = u32::try_from(inputs.len()).unwrap_or_default();
    // SAFETY: the slice is a live `Vec` for the length of the call and `cbSize` is the true
    // size of one element, which is what `SendInput` uses to walk it.
    let sent = unsafe {
        SendInput(
            &inputs,
            i32::try_from(size_of::<INPUT>()).unwrap_or_default(),
        )
    };
    sent == count
}

/// Whether a key is physically held right now.
fn is_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: a read of the asynchronous key state, which takes an integer and returns one.
    // The high bit is the documented "currently down"; the low bit is "pressed since the
    // last call" and is deliberately not read, because it would answer about history.
    let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };
    state as u16 & 0x8000 != 0
}

/// One synthetic key event.
fn key_event(key: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Select everything in the focused window and copy it.
///
/// **Test builds only.** The hand test in `paste.rs` needs to read back what actually landed
/// in Notepad, and Windows 11's Notepad has no window to ask: its editor is a XAML control,
/// so `WM_GETTEXT` finds a helper window's caption and nothing else. `Ctrl+A` then `Ctrl+C` is
/// how a person would check, and it is the only check that works against every editor rather
/// than against the ones that still use a classic control.
#[cfg(test)]
pub fn send_select_all_and_copy() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_A, VK_C};

    send_chord(false, VK_A) && send_chord(false, VK_C)
}

/// The process this window belongs to.
///
/// **Test builds only.** The hand test in `paste.rs` needs it because a window's process is
/// not always the process that was started: on Windows 11 `notepad.exe` is a stub that hands
/// off to a packaged application, so the window that appears belongs to a pid nobody was
/// told about — and the only honest way to close it afterwards is to ask the window.
#[cfg(test)]
#[must_use]
pub fn process_id(window: Hwnd) -> Option<u32> {
    let mut owner = 0u32;
    // SAFETY: a live local for the length of the call.
    let thread = unsafe { GetWindowThreadProcessId(window.raw(), Some(&mut owner)) };
    (thread != 0 && owner != 0).then_some(owner)
}

/// The text this window and its children report through `WM_GETTEXT`.
///
/// **Test builds only**, and for the same reason as [`process_id`]: the hand test in
/// `paste.rs` has to find out whether a paste actually landed, and the only program that
/// knows is the one it landed in. A classic edit control answers directly; a window whose
/// editor is a XAML control answers with a helper window's caption, which is why the test
/// treats an answer without the dictation in it as "ask another way" rather than as a
/// failure.
#[cfg(test)]
#[must_use]
pub fn text_of(window: Hwnd) -> String {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, SendMessageW, WM_GETTEXT, WM_GETTEXTLENGTH,
    };
    use windows::core::BOOL;

    fn read(window: HWND) -> String {
        // SAFETY: `WM_GETTEXTLENGTH` takes no pointers and answers with a count.
        let length = unsafe { SendMessageW(window, WM_GETTEXTLENGTH, None, None) }.0;
        let Ok(length) = usize::try_from(length) else {
            return String::new();
        };
        if length == 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length + 1];
        // SAFETY: the buffer holds `length + 1` units and that is exactly what `wParam`
        // promises the receiver, which is the contract `WM_GETTEXT` is defined by.
        let written = unsafe {
            SendMessageW(
                window,
                WM_GETTEXT,
                Some(WPARAM(buffer.len())),
                Some(LPARAM(buffer.as_mut_ptr() as isize)),
            )
        }
        .0;
        let written = usize::try_from(written).unwrap_or(0).min(length);
        String::from_utf16_lossy(&buffer[..written])
    }

    unsafe extern "system" fn collect(child: HWND, into: LPARAM) -> BOOL {
        // SAFETY: as in `windows_of` — the address of a local that outlives the enumeration.
        let found = unsafe { &mut *(into.0 as *mut Vec<String>) };
        let text = read(child);
        if !text.is_empty() {
            found.push(text);
        }
        BOOL(1)
    }

    let mut found: Vec<String> = Vec::new();
    // SAFETY: as in `windows_of`.
    let _ = unsafe {
        EnumChildWindows(
            Some(window.raw()),
            Some(collect),
            LPARAM(std::ptr::from_mut(&mut found) as isize),
        )
    };
    if found.is_empty() {
        return read(window.raw());
    }
    found.join(
        "
",
    )
}

#[cfg(test)]
mod tests {
    use super::{Hwnd, Rect, primary_monitor};

    #[test]
    fn a_rectangle_measures_itself() {
        let rect = Rect {
            left: 100,
            top: 50,
            right: 1_000,
            bottom: 650,
        };
        assert_eq!(rect.width(), 900);
        assert_eq!(rect.height(), 600);
    }

    #[test]
    fn a_handle_that_never_was_is_not_a_window() {
        // Not a null handle — that one is filtered before it becomes an `Hwnd` — but a
        // number no window will ever have. `IsWindow` is documented to answer rather than
        // fault, which is the property the paste path depends on.
        let nonsense = Hwnd::foreground();
        let made_up = super::Hwnd(0x7FFF_FFF0);
        assert!(!made_up.exists());
        // And whatever has the focus while the tests run, asking about it does not panic.
        let _ = nonsense.map(Hwnd::exists);
    }

    #[test]
    fn the_primary_monitor_has_a_name_and_a_size() {
        // Every machine this test suite runs on has a display; a headless runner would be a
        // reason to skip rather than to fail, so this asserts only when one is found.
        if let Some(monitor) = primary_monitor() {
            assert!(!monitor.device.is_empty());
            assert!(monitor.bounds.width() > 0, "{:?}", monitor.bounds);
            assert!(monitor.work.height() > 0, "{:?}", monitor.work);
            assert!(
                monitor.work.height() <= monitor.bounds.height(),
                "the work area cannot be taller than the screen"
            );
        }
    }
}
