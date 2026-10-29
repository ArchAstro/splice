//! What can be recorded (displays and windows), and the clicks noticed
//! while recording, kept beside a take to zoom in on later. The recording
//! itself is `capture`'s.

use crate::session::dirs;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What to record.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// A display, counting from 1 (the main one).
    Display {
        number: u32,
    },
    Window {
        id: u32,
    },
    /// Screen points, from the top left of the main display.
    Region {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct Display {
    pub number: u32,
    /// CoreGraphics' number for it.
    pub id: u32,
    pub name: String,
    /// Its top left, in points from the main display's top left.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Window {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
}

/// The displays, main first.
pub fn displays() -> Vec<Display> {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    let mut out = Vec::new();
    unsafe {
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        let key = CFString::new("NSScreenNumber");
        let mut main_height = 0.0;
        for i in 0..count {
            let screen: *mut Object = msg_send![screens, objectAtIndex: i];
            let frame: Rect = msg_send![screen, frame];
            if i == 0 {
                main_height = frame.h;
            }
            let description: *mut Object = msg_send![screen, deviceDescription];
            let number: *mut Object = msg_send![description, objectForKey: key.as_concrete_TypeRef() as *mut Object];
            let id: u32 = if number.is_null() {
                0
            } else {
                msg_send![number, unsignedIntValue]
            };
            let name: *mut Object = msg_send![screen, localizedName];
            let text: *const std::os::raw::c_char = msg_send![name, UTF8String];
            let name = if text.is_null() {
                format!("Display {}", i + 1)
            } else {
                std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned()
            };
            out.push(Display {
                number: i as u32 + 1,
                id,
                name,
                // Cocoa counts up from the bottom; CoreGraphics, down from the top.
                x: frame.x,
                y: main_height - (frame.y + frame.h),
                width: frame.w,
                height: frame.h,
            });
        }
    }
    out
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative: u32) -> core_foundation::array::CFArrayRef;
}

/// Ordinary windows on screen, front first, without Splice's own.
pub fn windows() -> Vec<Window> {
    const ON_SCREEN: u32 = 1;
    const NO_DESKTOP: u32 = 1 << 4;
    let raw = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN | NO_DESKTOP, 0) };
    if raw.is_null() {
        return Vec::new();
    }
    let list: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw) };
    let me = std::process::id() as i64;
    let mut out = Vec::new();
    for info in list.iter() {
        let get = |key: &str| info.find(CFString::new(key)).map(|v| v.clone());
        let number = |key: &str| get(key).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
        let text = |key: &str| get(key).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string());
        if number("kCGWindowLayer") != Some(0) || number("kCGWindowOwnerPID") == Some(me) {
            continue;
        }
        let Some(id) = number("kCGWindowNumber") else { continue };
        let (mut w, mut h) = (0.0, 0.0);
        if let Some(bounds) = get("kCGWindowBounds").and_then(|v| v.downcast::<CFDictionary>()) {
            let bounds: CFDictionary<CFString, CFType> =
                unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef()) };
            let side = |k: &str| {
                bounds
                    .find(CFString::new(k))
                    .and_then(|v| v.downcast::<CFNumber>())
                    .and_then(|n| n.to_f64())
                    .unwrap_or(0.0)
            };
            (w, h) = (side("Width"), side("Height"));
        }
        if w < 80.0 || h < 60.0 {
            continue;
        }
        out.push(Window {
            id: id as u32,
            app: text("kCGWindowOwnerName").unwrap_or_default(),
            title: text("kCGWindowName").unwrap_or_default(),
            width: w,
            height: h,
        });
    }
    out
}

/// A click while recording: seconds in, and where, as fractions of what is
/// recorded from its top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct Click {
    pub t: f64,
    pub x: f64,
    pub y: f64,
}

/// The clicks noted beside a take, for its media.
pub fn read_clicks(take: &std::path::Path) -> Vec<splice_core::model::Click> {
    let Ok(text) = std::fs::read_to_string(clicks_file(take)) else {
        return Vec::new();
    };
    let clicks: Vec<Click> = serde_json::from_str(&text).unwrap_or_default();
    clicks
        .into_iter()
        .map(|c| splice_core::model::Click {
            at: splice_core::time::from_secs(c.t),
            x: c.x as f32,
            y: c.y as f32,
        })
        .collect()
}

/// Where clicks of a take are kept: beside it.
pub fn clicks_file(take: &std::path::Path) -> PathBuf {
    take.with_extension("clicks.json")
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceCounterForEventType(state: u32, event: u32) -> u32;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> Point;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const std::ffi::c_void);
}

/// Where the mouse is, in screen points from the main display's top left.
fn mouse() -> Option<(f64, f64)> {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some((p.x, p.y))
    }
}

/// How many times the left button has gone down since the login session
/// began (no permission needed to ask).
fn presses() -> u32 {
    const COMBINED: u32 = 0;
    const LEFT_MOUSE_DOWN: u32 = 1;
    unsafe { CGEventSourceCounterForEventType(COMBINED, LEFT_MOUSE_DOWN) }
}

/// The part of the screen `source` records, in screen points from the main
/// display's top left: x, y, width, height.
fn area(source: &Source) -> Option<(f64, f64, f64, f64)> {
    match source {
        Source::Region { x, y, width, height } => Some((*x as f64, *y as f64, *width as f64, *height as f64)),
        Source::Display { number } => {
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct Rect {
                x: f64,
                y: f64,
                w: f64,
                h: f64,
            }
            unsafe {
                let screens: *mut Object = msg_send![class!(NSScreen), screens];
                let count: usize = msg_send![screens, count];
                let index = (*number as usize).saturating_sub(1);
                if count == 0 || index >= count {
                    return None;
                }
                let main: *mut Object = msg_send![screens, objectAtIndex: 0usize];
                let main: Rect = msg_send![main, frame];
                let screen: *mut Object = msg_send![screens, objectAtIndex: index];
                let f: Rect = msg_send![screen, frame];
                // Cocoa counts up from the bottom; the mouse, down from the top.
                Some((f.x, main.h - (f.y + f.h), f.w, f.h))
            }
        }
        Source::Window { id } => window_bounds(*id),
    }
}

/// A window's place on screen, in points from the main display's top left.
fn window_bounds(id: u32) -> Option<(f64, f64, f64, f64)> {
    const ON_SCREEN: u32 = 1;
    let raw = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN, 0) };
    if raw.is_null() {
        return None;
    }
    let list: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw) };
    for info in list.iter() {
        let number = info
            .find(CFString::new("kCGWindowNumber"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64());
        if number != Some(id as i64) {
            continue;
        }
        let bounds = info
            .find(CFString::new("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>())?;
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_concrete_TypeRef()) };
        let side = |k: &str| {
            bounds
                .find(CFString::new(k))
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_f64())
                .unwrap_or(0.0)
        };
        return Some((side("X"), side("Y"), side("Width"), side("Height")));
    }
    None
}

/// Clicks noted by the wall clock (seconds since 1970) in a take's own
/// time: the take began at `zero`, was paused for `pauses` (wall clock,
/// from and to) and lasted `length`. Clicks outside it are left out, as is
/// the click that stopped it, in its last half second.
pub fn in_take(clicks: Vec<Click>, zero: f64, pauses: &[(f64, f64)], length: f64) -> Vec<Click> {
    clicks
        .into_iter()
        .filter_map(|c| {
            if pauses.iter().any(|(a, b)| c.t >= *a && c.t < *b) {
                return None;
            }
            let paused: f64 = pauses.iter().filter(|(_, b)| *b <= c.t).map(|(a, b)| b - a).sum();
            let t = c.t - zero - paused;
            (t >= 0.0 && t < length - 0.5).then_some(Click { t, ..c })
        })
        .collect()
}

/// Starts noting clicks inside what `source` records, by the wall clock,
/// until the flag is set; the thread's result is the clicks.
pub fn watch_clicks(source: &Source) -> Option<(Arc<AtomicBool>, std::thread::JoinHandle<Vec<Click>>)> {
    let area = area(source)?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::Builder::new()
        .name("splice-clicks".into())
        .spawn(move || watch(area, flag))
        .ok()
        .map(|watcher| (stop, watcher))
}

/// Watches for clicks inside `area` until `stop`.
fn watch(area: (f64, f64, f64, f64), stop: Arc<AtomicBool>) -> Vec<Click> {
    let (ax, ay, aw, ah) = area;
    let mut clicks = Vec::new();
    // Counting presses rather than looking at the button: a tap on a
    // trackpad can go down and up between two looks.
    let mut seen = presses();
    while !stop.load(Ordering::Relaxed) {
        let count = presses();
        let new = count != seen;
        seen = count;
        if new && let Some((x, y)) = mouse() {
            let (u, v) = ((x - ax) / aw.max(1.0), (y - ay) / ah.max(1.0));
            if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                let t = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64());
                clicks.push(Click { t, x: u, y: v });
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    clicks
}

/// Where takes are kept before they are brought into a project.
pub fn takes() -> PathBuf {
    dirs().projects.join("Recordings")
}

/// Now, as a take's name: 2026-10-28 14.03.12.
pub fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let output = Command::new("date")
        .args(["-r", &secs.to_string(), "+%Y-%m-%d %H.%M.%S"])
        .output()
        .ok();
    output
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| secs.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_are_timed_in_the_take_without_its_pauses() {
        let at = |t: f64| Click { t, x: 0.5, y: 0.5 };
        let zero = 1000.0;
        let clicks = vec![at(999.0), at(1001.0), at(1003.0), at(1006.0), at(1009.8)];
        // Paused from 2 s in until 5 s in (wall clock 1002 to 1005).
        let kept = in_take(clicks, zero, &[(1002.0, 1005.0)], 7.0);
        let times: Vec<f64> = kept.iter().map(|c| c.t).collect();
        // Before the take, while paused, and the stopping click are gone.
        assert_eq!(times, vec![1.0, 3.0]);
    }

    #[test]
    fn lists_displays_and_windows() {
        // A machine without a screen (a CI runner) may list none; listing must
        // still not fail.
        for display in displays() {
            assert!(display.number >= 1 && display.width > 0.0);
        }
        let _ = windows();
    }
}
