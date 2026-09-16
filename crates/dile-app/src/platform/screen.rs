//! Rectangles and displays, in physical pixels.
//!
//! Neither type asks an operating system anything, which is why they live above the platform
//! split rather than inside one of its halves: the panel's placement arithmetic — centre the
//! card on the work area, keep a remembered position on the screen it was remembered for —
//! is the same arithmetic everywhere, and it is unit-tested everywhere.
//!
//! The platform modules fill these in. `win32` reads them out of `MONITORINFOEXW`; a port
//! reads them out of whatever its own window system calls a display.

/// A rectangle in physical pixels, with the origin at the desktop's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge, exclusive.
    pub right: i32,
    /// Bottom edge, exclusive.
    pub bottom: i32,
}

impl Rect {
    /// How wide it is.
    #[must_use]
    pub const fn width(self) -> i32 {
        self.right - self.left
    }

    /// How tall it is.
    #[must_use]
    pub const fn height(self) -> i32 {
        self.bottom - self.top
    }
}

/// One display, as the panel needs to know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// The name the window system gives it, such as `\\.\DISPLAY1`.
    ///
    /// The key the remembered panel position is stored under. It is stable across a session
    /// and across most reboots, which is the right amount of stability for "where did I drag
    /// this to on the left-hand screen".
    pub device: String,
    /// The whole display, in physical pixels.
    pub bounds: Rect,
    /// The part of it not covered by a taskbar or a dock, in physical pixels.
    pub work: Rect,
}
