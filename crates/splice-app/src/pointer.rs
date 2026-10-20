//! Mouse events made inside Splice and handed to its own window, as AppKit
//! would hand it a real click: the same path, hit-testing and hovers. They
//! never leave the process, so macOS's guard on synthetic input (which needs
//! Accessibility) does not apply. For `splice ctl click` and friends.

use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateScrollWheelEvent(source: *const c_void, units: u32, count: u32, wheel1: i32, ...) -> *mut c_void;
    fn CGEventSetLocation(event: *mut c_void, at: NSPoint);
    fn CFRelease(object: *const c_void);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Keys {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub command: bool,
}

impl Keys {
    fn flags(self) -> u64 {
        (if self.shift { 1 << 17 } else { 0 })
            | (if self.control { 1 << 18 } else { 0 })
            | (if self.alt { 1 << 19 } else { 0 })
            | (if self.command { 1 << 20 } else { 0 })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Move {
        x: f64,
        y: f64,
        held: Option<Button>,
    },
    Down {
        x: f64,
        y: f64,
        button: Button,
        count: isize,
    },
    Up {
        x: f64,
        y: f64,
        button: Button,
        count: isize,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
}

/// A gpui window's AppKit window, to send events to. Only use it on the main
/// thread, and not from inside a gpui update (the window handles the event
/// as if AppKit had sent it).
#[derive(Clone, Copy)]
pub struct Target {
    window: *mut Object,
    view: *mut Object,
}

impl Target {
    pub fn of(window: &gpui::Window) -> Option<Target> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        let view = appkit.ns_view.as_ptr() as *mut Object;
        let window: *mut Object = unsafe { msg_send![view, window] };
        (!window.is_null()).then_some(Target { window, view })
    }

    /// Sends `event` at a point given in window points from the top left.
    pub fn send(&self, event: Event, keys: Keys) {
        unsafe {
            let frame: NSRect = msg_send![self.view, frame];
            // AppKit counts up from the bottom.
            let flip = |x: f64, y: f64| NSPoint {
                x,
                y: frame.size.height - y,
            };
            let number: isize = msg_send![self.window, windowNumber];
            let made: *mut Object = match event {
                Event::Move { x, y, held } => {
                    let kind: usize = match held {
                        None => 5,
                        Some(Button::Left) => 6,
                        Some(Button::Right) => 7,
                    };
                    self.mouse(kind, flip(x, y), keys, number, 0)
                }
                Event::Down { x, y, button, count } => {
                    let kind = if button == Button::Left { 1 } else { 3 };
                    self.mouse(kind, flip(x, y), keys, number, count)
                }
                Event::Up { x, y, button, count } => {
                    let kind = if button == Button::Left { 2 } else { 4 };
                    self.mouse(kind, flip(x, y), keys, number, count)
                }
                Event::Scroll { x, y, dx, dy } => {
                    // Scroll events come from Core Graphics, placed on screen.
                    let in_window = flip(x, y);
                    let on_screen: NSPoint = msg_send![self.window, convertPointToScreen: in_window];
                    let screen: *mut Object = msg_send![class!(NSScreen), mainScreen];
                    let screen_frame: NSRect = msg_send![screen, frame];
                    let cg =
                        CGEventCreateScrollWheelEvent(std::ptr::null(), 0, 2, dy.round() as i32, dx.round() as i32);
                    if cg.is_null() {
                        return;
                    }
                    CGEventSetLocation(
                        cg,
                        NSPoint {
                            x: on_screen.x,
                            y: screen_frame.size.height - on_screen.y,
                        },
                    );
                    let event: *mut Object = msg_send![class!(NSEvent), eventWithCGEvent: cg];
                    CFRelease(cg);
                    event
                }
            };
            if !made.is_null() {
                let () = msg_send![self.window, sendEvent: made];
            }
        }
    }

    unsafe fn mouse(&self, kind: usize, at: NSPoint, keys: Keys, number: isize, count: isize) -> *mut Object {
        let nil: *mut Object = std::ptr::null_mut();
        let info: *mut Object = unsafe { msg_send![class!(NSProcessInfo), processInfo] };
        let timestamp: f64 = unsafe { msg_send![info, systemUptime] };
        unsafe {
            msg_send![class!(NSEvent),
                mouseEventWithType: kind
                location: at
                modifierFlags: keys.flags()
                timestamp: timestamp
                windowNumber: number
                context: nil
                eventNumber: 0isize
                clickCount: count
                pressure: if matches!(kind, 1 | 3 | 6 | 7) { 1.0f32 } else { 0.0f32 }]
        }
    }
}
