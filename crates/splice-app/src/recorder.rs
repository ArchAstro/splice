//! Recording the screen: a whole display, one window, or a rectangle, with
//! the cursor, optional clicks and microphone. macOS's own `screencapture`
//! does the capture (hardware H.264, 60 fps); Splice starts it, stops it with
//! an interrupt so the file is finished properly, and brings the take in.

use crate::session::dirs;
use anyhow::{Context, Result};
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

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

#[derive(Clone, Debug, Default, Serialize)]
pub struct Options {
    pub microphone: bool,
    pub clicks: bool,
    /// Stops by itself after this many seconds.
    pub seconds: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Display {
    pub number: u32,
    pub name: String,
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
        for i in 0..count {
            let screen: *mut Object = msg_send![screens, objectAtIndex: i];
            let frame: Rect = msg_send![screen, frame];
            let name: *mut Object = msg_send![screen, localizedName];
            let text: *const std::os::raw::c_char = msg_send![name, UTF8String];
            let name = if text.is_null() {
                format!("Display {}", i + 1)
            } else {
                std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned()
            };
            out.push(Display {
                number: i as u32 + 1,
                name,
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

/// A recording under way.
pub struct Recording {
    child: Child,
    pub path: PathBuf,
    pub started: Instant,
    pub source: Source,
    /// Watching the mouse for clicks, until told to stop.
    clicks: Option<(Arc<AtomicBool>, std::thread::JoinHandle<Vec<Click>>)>,
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
    fn CGEventSourceButtonState(state: u32, button: u32) -> bool;
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

/// Whether the left button is down (no permission needed to ask).
fn pressed() -> bool {
    const COMBINED: u32 = 0;
    const LEFT: u32 = 0;
    unsafe { CGEventSourceButtonState(COMBINED, LEFT) }
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

/// screencapture starts recording about this long after it is started.
const LATE: f64 = 0.2;

/// Clicks in the recording's own time, without the click that stopped it
/// (the Stop button, or Splice in the Dock) in its last half second.
fn settle(clicks: Vec<Click>, ended: f64) -> Vec<Click> {
    clicks
        .into_iter()
        .filter(|c| c.t < ended - 0.5)
        .map(|c| Click {
            t: (c.t - LATE).max(0.0),
            ..c
        })
        .collect()
}

/// Watches for clicks inside `area` until `stop`, from `started`.
fn watch(area: (f64, f64, f64, f64), started: Instant, stop: Arc<AtomicBool>) -> Vec<Click> {
    let (ax, ay, aw, ah) = area;
    let mut clicks = Vec::new();
    let mut was = pressed();
    while !stop.load(Ordering::Relaxed) {
        let now = pressed();
        if now
            && !was
            && let Some((x, y)) = mouse()
        {
            let (u, v) = ((x - ax) / aw.max(1.0), (y - ay) / ah.max(1.0));
            if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                clicks.push(Click {
                    t: started.elapsed().as_secs_f64(),
                    x: u,
                    y: v,
                });
            }
        }
        was = now;
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    clicks
}

impl Recording {
    pub fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Whether it has stopped by itself (a time limit, an error).
    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Stops it and waits for the file to be finished, with its clicks
    /// beside it. Blocking (a second or two).
    pub fn stop(mut self) -> Result<PathBuf> {
        if !self.finished() {
            unsafe {
                libc_kill(self.child.id() as i32, 2);
            }
        }
        if let Some((stop, watcher)) = self.clicks.take() {
            let ended = self.started.elapsed().as_secs_f64();
            stop.store(true, Ordering::Relaxed);
            if let Ok(clicks) = watcher.join()
                && let clicks = settle(clicks, ended)
                && !clicks.is_empty()
            {
                let _ = std::fs::write(
                    clicks_file(&self.path),
                    serde_json::to_string(&clicks).unwrap_or_default(),
                );
            }
        }
        let output = self.child.wait_with_output()?;
        let ok = std::fs::metadata(&self.path).is_ok_and(|m| m.len() > 0);
        if !ok {
            let why = String::from_utf8_lossy(&output.stderr).trim().to_string();
            anyhow::bail!(if why.is_empty() {
                "the recording came out empty; check that Splice's app (or your terminal) has Screen Recording \
                 permission in System Settings → Privacy & Security"
                    .to_string()
            } else {
                why
            });
        }
        Ok(self.path)
    }
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}

/// Where takes are kept before they are brought into a project.
pub fn takes() -> PathBuf {
    dirs().projects.join("Recordings")
}

/// Starts recording `source`.
pub fn start(source: Source, options: &Options) -> Result<Recording> {
    let dir = takes();
    std::fs::create_dir_all(&dir)?;
    let stamp = {
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
    };
    let path = dir.join(format!("Recording {stamp}.mov"));
    let mut command = Command::new("/usr/sbin/screencapture");
    command.args(["-x", "-v", "-C"]);
    match &source {
        Source::Display { number } => {
            command.arg(format!("-D{}", number.max(&1)));
        }
        Source::Window { id } => {
            command.arg(format!("-l{id}")).arg("-o");
        }
        Source::Region { x, y, width, height } => {
            command.arg(format!("-R{x},{y},{width},{height}"));
        }
    }
    if options.microphone {
        command.arg("-g");
    }
    if options.clicks {
        command.arg("-k");
    }
    if let Some(s) = options.seconds {
        command.arg(format!("-V{}", s.max(1)));
    }
    let child = command
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting screencapture")?;
    let started = Instant::now();
    // Clicks are noted for zooming in on later.
    let clicks = area(&source).and_then(|area| {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("splice-clicks".into())
            .spawn(move || watch(area, started, flag))
            .ok()
            .map(|watcher| (stop, watcher))
    });
    Ok(Recording {
        child,
        path,
        started,
        source,
        clicks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_click_that_stops_a_recording_is_not_kept() {
        let clicks = vec![
            Click { t: 1.0, x: 0.5, y: 0.5 },
            Click {
                t: 9.8,
                x: 0.9,
                y: 0.99,
            },
        ];
        let kept = settle(clicks, 10.0);
        assert_eq!(kept.len(), 1);
        assert!((kept[0].t - 0.8).abs() < 1e-9, "in the recording's own time");
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
