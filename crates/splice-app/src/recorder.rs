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
}

impl Recording {
    pub fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Whether it has stopped by itself (a time limit, an error).
    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Stops it and waits for the file to be finished. Blocking (a second or two).
    pub fn stop(mut self) -> Result<PathBuf> {
        if !self.finished() {
            unsafe {
                libc_kill(self.child.id() as i32, 2);
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
    Ok(Recording {
        child,
        path,
        started: Instant::now(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
