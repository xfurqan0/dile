//! The desktop, per platform: which window is in front, which screen it is on, and the
//! clipboard.
//!
//! Everything above this module is platform-free. `panel.rs` decides where a card belongs and
//! `paste.rs` decides what to call the target and which chord to send it; both of them ask
//! this module the four questions that only an operating system can answer, and neither of
//! them names one. That is why their tables are ordinary unit tests on a machine with no
//! clipboard and no foreground window — on **any** machine, now that there is more than one
//! kind.
//!
//! | Platform | Foreground window | Screen coordinates | Aimed paste chord | Clipboard | Unaimed chord |
//! |---|---|---|---|---|---|
//! | Windows | [`win32`] | [`win32`] | [`win32`] | [`win32`] | — |
//! | Linux | [`unported`] | [`unported`] | [`unported`] | **[`linux`]** | **[`uinput`]** |
//! | everything else | [`unported`] | [`unported`] | [`unported`] | [`unported`] | [`unported`] |
//!
//! Linux is why that is a table of columns rather than a list of two modules. A platform
//! is not ported or unported as a whole: three of those questions have no answer under
//! Wayland by design, and the clipboard does. [`DELIVERY`] is what the
//! difference adds up to, and it is a value rather than a `cfg` so that both of the panel's
//! paths are ordinary code with ordinary tests on either kind of machine.
//!
//! [`screen`] sits outside that split because a rectangle is not a platform question. The
//! panel's placement arithmetic is tested on every platform this workspace builds on, which
//! is the same reason `dile-hotkey` keeps its state machine away from its listener.
//!
//! ## What [`unported`] is, and what it is not
//!
//! It is not a fallback and it is not a stub that pretends. Every one of its answers is the
//! true one for a platform whose desktop integration has not been written: there is no
//! foreground window it can name, so it names none; there is no clipboard agent it can start,
//! so starting one fails with a sentence that says which platform and why. The application
//! runs on top of that — the tray works, the trigger works, a dictation is transcribed and
//! shown — and the parts that need a desktop say they cannot rather than doing something
//! approximate.
//!
//! On Linux three of those answers are **permanently** no rather than not-yet: a Wayland
//! client is not told which window has the focus, cannot place a window at a screen
//! coordinate, and cannot send a key press to a window it cannot name. Those are the
//! protocol's decisions rather than this application's, and `unported` states them for Linux
//! as exactly what they are. The clipboard was the one that had a route, so [`linux`] is the
//! module that took it.
//!
//! The last column is the one thing that is neither: a key press **can** be synthesised on
//! Linux, through the `uinput` permission the trigger already asks for, and it goes to
//! whatever holds the keyboard rather than to a window anybody chose. [`uinput`] is that, and
//! [`AUTO_PASTE_OFFERED`] is the constant that says where the settings window may offer it.
//! It is a different and smaller thing from the aimed chord in the column before it, which is
//! why it is a column of its own rather than an answer in that one.

pub mod screen;

// A display is what the application above this seam works in; the rectangles it is made of
// are its innards, and `screen::Rect` is the name for them.
pub use screen::Monitor;

#[cfg(windows)]
pub mod win32;

#[cfg(windows)]
pub use win32::{Clipboard, ClipboardError, Hwnd, window};

#[cfg(windows)]
pub use unported::AutoPaste;

// Compiled on every platform, and on Windows for one item only: `win32` answers every other
// question this module has, and the unaimed chord is not a question `win32` has. The file
// itself says which half of it each platform takes.
pub mod unported;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub mod uinput;

// Linux keeps three of `unported`'s answers and brings its own clipboard and its own
// unaimed chord. The split is here rather than inside any of the modules so that none of
// them has to know about the others.
#[cfg(target_os = "linux")]
pub use linux::{Clipboard, ClipboardError};

// The error type is deliberately **not** re-exported alongside it, on any platform: the one
// caller reads a locale key off a failure rather than matching on its variants, and a binary
// crate's unused `pub use` is a warning this workspace treats as an error. It is named by the
// module that produces it, which is where its variants are worth reading.
#[cfg(target_os = "linux")]
pub use uinput::AutoPaste;

#[cfg(target_os = "linux")]
pub use unported::{Hwnd, window};

#[cfg(all(not(windows), not(target_os = "linux")))]
pub use unported::{AutoPaste, Clipboard, ClipboardError, Hwnd, window};

/// How a finished dictation gets from the panel into what the user is typing in.
///
/// The one product decision this seam carries, because it is the one that a missing platform
/// answer decides rather than merely limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Dile puts the text where the caret is: it knows the target window, it can bring it
    /// back to the front, and it can send it a paste chord.
    Paste,
    /// Dile puts the text on the clipboard and says so, and the person presses `Ctrl+V`
    /// wherever they want it.
    ///
    /// Not a degraded paste. There is no window to aim at under Wayland — a client is not
    /// told which one has the focus — so a paste here would go to whatever happened to be in
    /// front, which is the one thing `paste.rs` exists to refuse. The hand-over is a smaller
    /// promise that is kept.
    Clipboard,
}

/// How this build hands a dictation over.
///
/// `cfg!` rather than `#[cfg]` on purpose: this is a constant both branches of `panel.rs`
/// compile against, so the clipboard path is type-checked on Windows and the paste path is
/// type-checked on Linux.
pub const DELIVERY: Delivery = if cfg!(windows) {
    Delivery::Paste
} else {
    Delivery::Clipboard
};

/// Whether this build can press `Ctrl+V` at whatever holds the keyboard, and may therefore
/// offer the experimental switch that does.
///
/// **Not the same question as [`DELIVERY`], and not the same as a paste.** It is true exactly
/// where two things hold at once: the hand-over is a clipboard one, so there is something for
/// a chord to add; and there is a way to synthesise a key press at all. Linux is the only
/// platform where both are true — [`uinput`] is the way, and the permission it needs is the
/// one the trigger already asks for.
///
/// On Windows it is false because a dictation is pasted into the window it was **aimed at**,
/// which is the larger promise; a switch offering an unaimed press next to it would be a
/// worse option presented as an extra.
pub const AUTO_PASTE_OFFERED: bool = cfg!(target_os = "linux");

/// The locale key of the card's line when an auto-paste that was asked for did not happen and
/// there is nothing more useful to say than that.
///
/// Here rather than in either module, because both of them return it and the panel names it
/// too: three places, one string.
pub const AUTO_PASTE_FAILED: &str = "panel.state.autopaste.failed";

/// Which window system this build is talking to.
///
/// **Not the operating system.** The same binary on the same Linux machine answers one way
/// under Wayland and another under `GDK_BACKEND=x11`, and the two differ on exactly the two
/// questions below. It is asked once, when the panel is built, because GDK chooses a backend
/// at start-up and never changes its mind afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSystem {
    /// The Windows desktop.
    Windows,
    /// A Wayland compositor, through GTK3.
    Wayland,
    /// An X server — which includes XWayland, because from a client's side it is the same
    /// protocol giving the same answers.
    X11,
    /// Something this application has not been taught to recognise, and will not guess about.
    Other,
}

impl WindowSystem {
    /// What this process is actually talking to.
    ///
    /// `cfg!` rather than `#[cfg]`, for [`DELIVERY`]'s reason: both branches are then ordinary
    /// compiled code on both platforms, so neither of them is a spelling only one machine ever
    /// type-checks.
    #[must_use]
    pub fn current() -> WindowSystem {
        if cfg!(windows) {
            WindowSystem::Windows
        } else {
            WindowSystem::named(&display_type_name())
        }
    }

    /// Which window system a GDK display's type name means.
    ///
    /// Here rather than in [`linux`] so that it is a pure function of a string on every
    /// platform, with the two names in one place and a test that reads them. An unrecognised
    /// name — or no display at all, which is what a `cargo test` process has — is
    /// [`WindowSystem::Other`]: a desktop nobody has measured gets the behaviour every build
    /// had before it was measured, rather than a guess.
    #[must_use]
    pub fn named(name: &str) -> WindowSystem {
        match name {
            "GdkWaylandDisplay" => WindowSystem::Wayland,
            "GdkX11Display" => WindowSystem::X11,
            _ => WindowSystem::Other,
        }
    }

    /// The name for a log line.
    ///
    /// One word each, and `unknown` rather than a sentence for the last one: a log line is not
    /// a translated string, and the i18n scan is right to refuse prose in Rust either way.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            WindowSystem::Windows => "windows",
            WindowSystem::Wayland => "wayland",
            WindowSystem::X11 => "x11",
            WindowSystem::Other => "unknown",
        }
    }
}

/// The GDK display's type name, where there is a GDK to ask.
///
/// Empty on a platform with no GTK backend, which [`WindowSystem::named`] reads as
/// [`WindowSystem::Other`] — the same answer it gives a Linux machine with no display.
fn display_type_name() -> String {
    #[cfg(target_os = "linux")]
    {
        linux::display_type_name()
    }
    #[cfg(not(target_os = "linux"))]
    {
        String::new()
    }
}

/// How the panel's window leaves the screen when a card closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    /// Unmap it, which is what *hidden* means everywhere a window's position belongs to the
    /// application that owns it.
    Unmap,
    /// Minimize it, and bring it back by asking to be activated.
    ///
    /// The window then lives between two cards instead of being built again for each one.
    /// That is visible to the user in two ways, and both are the point: the card comes back
    /// where it was dragged to, and a closed panel is a **minimized window rather than no
    /// window** — it is in the switcher and in the overview, which an unmapped one would not
    /// be. `skipTaskbar` cannot take it back out; that flag has no Wayland equivalent either.
    ///
    /// Only for a window system that was watched giving a minimized window back. That is a
    /// real condition and not a formality: see [`closing`] for the compositor that does not.
    Minimize,
}

/// How a card is taken off the screen, which is a different act on each window system.
///
/// **Measured on this machine — Fedora 44, GNOME 50.4 Wayland, mutter 50.4 — rather than
/// read.** Two protocol traces and one round trip decided it:
///
/// * Under Wayland, `gtk_widget_hide` sends `xdg_toplevel.destroy` and `xdg_surface.destroy`,
///   and the next show builds a **new** `wl_surface`, `xdg_surface` and `xdg_toplevel`. mutter
///   answers a destroyed toplevel by destroying its window and placing the replacement from
///   scratch, which with `center-new-windows` on is the middle of the screen. Dragging was
///   never the broken half — the compositor keeps a dragged position for as long as the window
///   lives — and `hide()` is what ends that life. `gtk_widget_unmap` was measured too, and is
///   the same two requests.
/// * `gtk_window_iconify` sends `xdg_toplevel.set_minimized` and destroys nothing: one
///   `xdg_toplevel` survived two full rounds of minimize and restore. **And that is where the
///   first version of this function stopped, one measurement short.** Keeping the window alive
///   is only half of a round trip; the other half is the compositor agreeing to put it back,
///   and mutter does not. With another application holding the focus — which is the only state
///   a trigger read off an evdev device can ever fire in, because the compositor never saw the
///   key — the same `xdg_activation_v1.activate` gets two different answers:
///   * on a **mapped** surface, `xdg_toplevel.configure` carrying *activated*, and the other
///     window deactivated in the same round;
///   * on a **minimized** surface, **nothing at all**. No configure, no state, no error. The
///     card stays down and the next trigger does nothing a user can see.
///
///   The token is the reason. A trigger that arrives from outside the compositor leaves the
///   process with no input serial to prove itself with, so GTK3 asks for one anyway and gets
///   `xdg_activation_token_v1.set_serial(0, seat)` and a token ending `_TIME0` — and
///   `gtk_window_present_with_time` does not help, because GDK3's Wayland backend never carries
///   an X11-style timestamp onto the wire. Raising a window it can already see is something
///   mutter will do on that token. Un-minimizing one is not.
/// * On the X11 backend, where a client may read its own coordinate, the whole round trip is
///   a number rather than an inference: a window moved to (417, 733) came back from
///   hide-and-show at (606, 570), the centre of this screen, and came back from
///   minimize-and-present at **(417, 733)**. Same compositor and same placement rule as the
///   Wayland case, and the one backend where the question could be answered by reading a
///   coordinate instead of by reading the wire.
///
/// So the two Linux backends part company. **X11 minimizes**, because there the card was
/// watched coming back. X11 could instead put the card back itself — its `set_position` before
/// a show was measured to work — but that path needs a monitor rectangle to clamp into, and
/// [`unported`]'s `primary_monitor` has none to give on this platform; minimizing needs no
/// coordinate at all.
///
/// **Wayland unmaps**, and pays the price named in [`remembers_position`]: every card opens
/// where the compositor puts it, which on a GNOME with `center-new-windows` on is the middle
/// of the screen. That is a worse card than one that reopens where it was dragged to, and a
/// far better one than a card that does not appear. Staying mapped and merely turning the
/// window invisible would keep the place *and* raise — the first of the two answers above says
/// so — but it cannot be done here either: GDK3 implements neither `set_opacity` nor
/// `set_keep_above` on its Wayland backend (both exist for X11 and for Broadway and neither is
/// a protocol request), so a panel that is never unmapped is a panel that is never off screen.
///
/// Windows unmaps, as it always has: there a position is the application's own to keep,
/// [`remembers_position`] is true, and a minimized panel would put a taskbar entry on screen
/// for a card that is supposed to be gone.
#[must_use]
pub const fn closing(system: WindowSystem) -> Closing {
    match system {
        WindowSystem::X11 => Closing::Minimize,
        // The conservative answer, and the one every desktop understands — including the one
        // that will raise a window it can see and ignore a request to raise one it cannot.
        WindowSystem::Windows | WindowSystem::Wayland | WindowSystem::Other => Closing::Unmap,
    }
}

/// Whether where a window says it is means anything on this window system.
///
/// The panel writes a dragged position into `settings.ui.panel_position` and puts the card
/// back there the next time it opens. That rests on two things a Wayland client does not have:
/// a `Moved` event carrying a real coordinate, and a `set_position` the compositor honours.
/// GTK3 answers `gdk_window_get_position` with (0, 0) under Wayland whatever the window is
/// doing, so the memory there would not be empty but **wrong** — a settings file filling up
/// with the origin, once per display.
///
/// False there rather than merely unused, so that the day a monitor rectangle does arrive on
/// Linux the memory does not quietly start recording zeroes.
///
/// Under Wayland this is now the whole of the answer rather than half of it. While a closed
/// card was minimized, the compositor kept the place this file could not; [`closing`] says why
/// that had to stop, and the honest consequence is that **on Wayland nothing remembers** — the
/// card opens where the compositor puts it, every time.
#[must_use]
pub const fn remembers_position(system: WindowSystem) -> bool {
    match system {
        WindowSystem::Windows | WindowSystem::X11 => true,
        WindowSystem::Wayland | WindowSystem::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AUTO_PASTE_OFFERED, Closing, DELIVERY, Delivery, WindowSystem, closing, remembers_position,
    };

    #[test]
    fn a_clipboard_hand_over_and_an_aimed_paste_chord_are_never_both_on() {
        // Two decisions taken in two places that have to agree. `send_paste_chord` means
        // *type the chord into the window this dictation was aimed at*, and a build that
        // could do that and still handed over on the clipboard would be refusing to use an
        // answer it has.
        //
        // **D-WP-L4 is the package that looked at this and did not change it.** What landed
        // there is a chord at whatever holds the keyboard, which is a different function in a
        // different module behind a setting that is off by default; the window this dictation
        // was aimed at is still not a thing a Wayland client can name, so this stays false
        // and this test stays a real guard rather than a historical one.
        if DELIVERY == Delivery::Clipboard {
            assert!(
                !super::window::send_paste_chord(false),
                "this build hands over on the clipboard and can also aim Ctrl+V"
            );
            assert!(
                !super::window::send_paste_chord(true),
                "this build hands over on the clipboard and can also aim Ctrl+Shift+V"
            );
        }
    }

    #[test]
    fn the_unaimed_chord_is_only_offered_where_the_hand_over_needs_one() {
        // The switch adds something only where a dictation is handed over rather than
        // pasted. Offering it next to a real paste would be offering a worse option as an
        // extra, and this is the assertion that says so on the platform that has the real one.
        if AUTO_PASTE_OFFERED {
            assert_eq!(
                DELIVERY,
                Delivery::Clipboard,
                "a platform that pastes has nothing to add an unaimed chord to"
            );
        }
        // The same invariant from the other end is deliberately not written a second time:
        // `assert!(!CONST)` is something the compiler already decided, which is exactly what
        // clippy's `assertions_on_constants` objects to, and the implication above is the
        // whole rule anyway.
        //
        // And it is a Linux answer today. A second platform arriving here is a decision, not
        // a side effect of a `cfg`.
        assert_eq!(AUTO_PASTE_OFFERED, cfg!(target_os = "linux"));
    }

    #[test]
    fn each_platform_hands_over_the_way_its_desktop_allows() {
        if cfg!(target_os = "linux") {
            assert_eq!(DELIVERY, Delivery::Clipboard);
        }
        if cfg!(windows) {
            assert_eq!(DELIVERY, Delivery::Paste);
        }
    }

    /// Every window system, on every machine that runs this suite.
    const EVERY: [WindowSystem; 4] = [
        WindowSystem::Windows,
        WindowSystem::Wayland,
        WindowSystem::X11,
        WindowSystem::Other,
    ];

    #[test]
    fn every_window_system_answers_both_questions() {
        // The table, written out, because the two questions are **independent** and the first
        // draft of this test assumed they were not. X11 is the case that proves it: a
        // coordinate there is a real number, so the position is worth remembering — and a
        // card there is minimized, because a minimized window on that backend comes back when
        // it is asked to. Wayland is the case that proves the point the other way round: a
        // card there is unmapped and its place is forgotten, and both halves of that are the
        // same measurement.
        let table = [
            (WindowSystem::Windows, Closing::Unmap, true),
            (WindowSystem::Wayland, Closing::Unmap, false),
            (WindowSystem::X11, Closing::Minimize, true),
            (WindowSystem::Other, Closing::Unmap, false),
        ];
        assert_eq!(table.len(), EVERY.len(), "a window system with no row");
        for (system, leaves_by, remembers) in table {
            assert_eq!(
                closing(system),
                leaves_by,
                "{system:?} closes the wrong way"
            );
            assert_eq!(
                remembers_position(system),
                remembers,
                "{system:?} remembers the wrong thing"
            );
        }
    }

    #[test]
    fn a_card_is_only_minimized_where_a_minimized_card_was_seen_to_come_back() {
        // The invariant this table really rests on, and the one the package before it got
        // wrong: **a way of closing a card is only allowed if opening it again was measured
        // to work.** Keeping a window alive is worth nothing if the compositor will not raise
        // it, and that is exactly what mutter does with a minimized surface — so X11 is the
        // only row that may say `Minimize`, and it says it because the round trip was run
        // there and the card came back.
        for system in EVERY {
            if closing(system) == Closing::Minimize {
                assert_eq!(
                    system,
                    WindowSystem::X11,
                    "{system:?} minimizes a card nobody has watched come back"
                );
            }
        }
    }

    #[test]
    fn wayland_unmaps_because_a_minimized_card_never_comes_back_there() {
        // The measurement is in `closing`: with another application focused, mutter answers
        // `xdg_activation_v1.activate` on a **mapped** surface with a `configure` carrying
        // *activated*, and answers the same request on a **minimized** one with nothing at
        // all. A card closed with `set_minimized` therefore stays closed, which is the fault
        // this replaces. Unmapping costs the dragged position, which `remembers_position`
        // already says is not readable here anyway.
        assert_eq!(closing(WindowSystem::Wayland), Closing::Unmap);
        assert!(!remembers_position(WindowSystem::Wayland));
    }

    #[test]
    fn x11_still_minimizes_because_there_the_card_does_come_back() {
        // The same round trip on the other backend, where a client may read its own
        // coordinate: (417, 733) came back from hide-and-show at (606, 570), the centre of
        // the screen, and came back from minimize-and-present at (417, 733). The X11 answer
        // is deliberately not "unmap and put it back", because putting it back needs a
        // monitor rectangle this platform does not have.
        assert_eq!(closing(WindowSystem::X11), Closing::Minimize);
        assert!(remembers_position(WindowSystem::X11));
    }

    #[test]
    fn windows_is_untouched_by_all_of_this() {
        // This package is a Linux fix, and the assertion that says so. A card on Windows is
        // hidden the way it has always been hidden, and its position is still this
        // application's own to remember.
        assert_eq!(closing(WindowSystem::Windows), Closing::Unmap);
        assert!(remembers_position(WindowSystem::Windows));
    }

    #[test]
    fn a_window_system_with_no_name_is_given_the_old_behaviour_rather_than_a_guess() {
        // `Other` is what GDK returns a name for that this application does not recognise.
        // Minimizing a window on a desktop nobody has measured would be a guess; unmapping it
        // is what every build did before this package, which is the one behaviour known not
        // to be a new surprise.
        assert_eq!(closing(WindowSystem::Other), Closing::Unmap);
        assert!(!remembers_position(WindowSystem::Other));
    }

    #[test]
    fn the_two_gdk_display_types_are_the_two_backends() {
        // The names GTK3 gives its display objects, and the only thing this application reads
        // to tell the two Linux backends apart. Written down as a test because they are a
        // dependency's spelling rather than ours: a build against something that renamed them
        // would not fail to compile, it would quietly decide every Linux machine is `Other`
        // and go back to hiding the panel.
        assert_eq!(
            WindowSystem::named("GdkWaylandDisplay"),
            WindowSystem::Wayland
        );
        assert_eq!(WindowSystem::named("GdkX11Display"), WindowSystem::X11);
        // A backend nothing here has measured, and no display at all, are the same answer.
        assert_eq!(
            WindowSystem::named("GdkBroadwayDisplay"),
            WindowSystem::Other
        );
        assert_eq!(WindowSystem::named(""), WindowSystem::Other);
    }

    #[test]
    fn this_machine_knows_which_window_system_it_is_on() {
        // `current()` reads GDK's own answer on Linux rather than an environment variable, so
        // this is a real question on a real display connection — except in a test process,
        // which has no display at all and gets `Other`. So the assertion is the one thing
        // that holds in both: a build never claims to be on a window system its own platform
        // could not be running.
        let system = WindowSystem::current();
        if cfg!(windows) {
            assert_eq!(system, WindowSystem::Windows);
        } else {
            assert_ne!(system, WindowSystem::Windows);
        }
    }
}
