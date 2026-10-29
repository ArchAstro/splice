//! Recording through splice-capture, the helper built from `capture/` and
//! carried inside Splice: it keeps the chosen sources live (their pictures
//! shown small, their sound as levels), and on request writes each source
//! to its own file, all starting at the same moment.

use crate::ffmpeg;
use crate::recorder::{self, Click, Source};
use crate::session::dirs;
use anyhow::{Context as _, Result, bail};
use gpui::RenderImage;
use serde::Deserialize;
use serde_json::{Value, json};
use splice_core::studio::{Codec, Screen, Studio, Take};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/splice-capture"));

/// The helper, written out beside Splice's other caches the first time
/// this build of it is needed.
pub fn helper() -> Result<PathBuf> {
    if HELPER.is_empty() {
        bail!("recording needs macOS");
    }
    let hash = blake3::hash(HELPER).to_hex();
    let dir = dirs().cache.join("bin");
    let path = dir.join(format!("splice-capture-{}", &hash[..12]));
    if !path.exists() {
        std::fs::create_dir_all(&dir)?;
        let partial = dir.join(format!("splice-capture.{}.part", std::process::id()));
        std::fs::write(&partial, HELPER)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&partial, &path)?;
    }
    Ok(path)
}

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub default: bool,
}

/// The cameras and microphones there are, and whether Splice may use them.
#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
pub struct Devices {
    pub cameras: Vec<Device>,
    pub microphones: Vec<Device>,
    pub camera_access: String,
    pub microphone_access: String,
    pub screen_access: bool,
}

/// Blocking: a fraction of a second.
pub fn devices() -> Result<Devices> {
    let output = Command::new(helper()?)
        .arg("devices")
        .output()
        .context("running splice-capture")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// A device chosen before, found again: by its id, else by its name (ids of
/// phones and USB devices can change).
pub fn find<'a>(chosen: &splice_core::studio::Device, there: &'a [Device]) -> Option<&'a Device> {
    there
        .iter()
        .find(|d| d.id == chosen.id)
        .or_else(|| there.iter().find(|d| d.name == chosen.name))
}

/// What of the screen `studio` records, as the click watcher sees it.
pub fn source(screen: &Screen) -> Result<Source> {
    Ok(match screen {
        Screen::Display { number } => Source::Display { number: *number },
        Screen::Window { app, title } => {
            let windows = recorder::windows();
            let found = windows
                .iter()
                .find(|w| &w.app == app && &w.title == title)
                .or_else(|| windows.iter().find(|w| &w.app == app));
            match found {
                Some(w) => Source::Window { id: w.id },
                None => bail!("{app} has no window open to record"),
            }
        }
        Screen::Region {
            display,
            x,
            y,
            width,
            height,
        } => {
            let origin = recorder::displays()
                .into_iter()
                .find(|d| d.number == *display)
                .map_or((0.0, 0.0), |d| (d.x, d.y));
            Source::Region {
                x: (origin.0 + x) as i32,
                y: (origin.1 + y) as i32,
                width: *width as u32,
                height: *height as u32,
            }
        }
    })
}

/// The helper's instructions for `studio`, with the devices there are.
pub fn config(studio: &Studio, devices: &Devices, preview: u32) -> Result<Value> {
    let screen = match &studio.screen {
        None => Value::Null,
        Some(screen) => match source(screen)? {
            Source::Window { id } => json!({ "window": id }),
            Source::Display { number } => json!({ "display": display_id(number)? }),
            Source::Region { .. } => {
                let Screen::Region {
                    display,
                    x,
                    y,
                    width,
                    height,
                } = screen
                else {
                    unreachable!()
                };
                json!({ "display": display_id(*display)?, "region": [x, y, width, height] })
            }
        },
    };
    let device = |chosen: &Option<splice_core::studio::Device>, there: &[Device], what: &str| -> Result<Value> {
        match chosen {
            None => Ok(Value::Null),
            Some(c) => match find(c, there) {
                Some(d) => Ok(json!(d.id)),
                None => bail!("the {what} “{}” is not connected", c.name),
            },
        }
    };
    let out = studio.output;
    Ok(json!({
        "screen": screen,
        "system_audio": studio.system_audio,
        "camera": device(&studio.camera, &devices.cameras, "camera")?,
        "microphone": device(&studio.microphone, &devices.microphones, "microphone")?,
        "codec": match out.codec { Codec::H264 => "h264", Codec::Hevc => "hevc" },
        "quality": out.quality.value(),
        "fps": out.fps.clamp(1, 120),
        "max_height": out.height,
        "cursor": out.cursor,
        "exclude": std::process::id(),
        "preview": preview,
        "mirror": studio.scene.mirror,
    }))
}

fn display_id(number: u32) -> Result<u32> {
    recorder::displays()
        .into_iter()
        .find(|d| d.number == number)
        .map(|d| d.id)
        .with_context(|| format!("there is no display {number}"))
}

/// The sources, as the helper numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feed {
    Screen,
    Camera,
}

enum Message {
    Event(Value),
    /// A preview picture, its full size, and the part of it kept.
    Frame(Feed, Arc<RenderImage>, (u32, u32), Option<Part>),
    Gone,
}

/// Part of a picture: left, top, right and bottom, as fractions of it.
pub type Part = [f32; 4];

/// The pixels of `part` of a BGRA picture, and their size.
fn cut(bgra: &[u8], (w, h): (u32, u32), part: Part) -> (Vec<u8>, (u32, u32)) {
    let edge = |f: f32, n: u32| ((f.clamp(0.0, 1.0) * n as f32).round() as u32).min(n);
    let (l, t) = (edge(part[0], w).min(w - 1), edge(part[1], h).min(h - 1));
    let (r, b) = (edge(part[2], w).clamp(l + 1, w), edge(part[3], h).clamp(t + 1, h));
    let (cw, ch) = (r - l, b - t);
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in t..t + ch {
        let row = (y * w + l) as usize * 4;
        out.extend_from_slice(&bgra[row..row + cw as usize * 4]);
    }
    (out, (cw, ch))
}

/// A recording under way.
pub struct Rec {
    pub dir: PathBuf,
    /// The files' first moment, in seconds since 1970.
    pub zero: Option<f64>,
    pub paused: Option<Instant>,
    /// Pauses so far, by the clock of `zero`: from, to.
    pub pauses: Vec<(f64, Option<f64>)>,
    /// How long has been recorded, as the helper last said.
    pub seconds: f64,
    clicks: Option<(Arc<AtomicBool>, std::thread::JoinHandle<Vec<Click>>)>,
}

/// The helper, running: the sources live.
pub struct Live {
    child: Option<Child>,
    stdin: ChildStdin,
    messages: Receiver<Message>,
    /// The instructions it was started with.
    pub config: Value,
    pub frames: HashMap<Feed, Arc<RenderImage>>,
    /// Each picture's full size, and the part of it its frame holds.
    pub natural: HashMap<Feed, (u32, u32)>,
    pub parts: HashMap<Feed, Option<Part>>,
    /// The part of the camera's picture the scene shows; its preview frames
    /// arrive cut to it, so a bubble can be drawn round.
    pub camera_part: Arc<std::sync::Mutex<Option<Part>>>,
    /// The loudest each sound has been lately, 0 to 1.
    pub levels: HashMap<String, f32>,
    /// Sources that have sent something.
    pub live: bool,
    pub recording: Option<Rec>,
    pub problem: Option<String>,
    /// What macOS is asking the person to allow, while it waits.
    pub asking: Option<String>,
    /// What to watch for clicks while recording.
    area: Option<Source>,
}

/// Something the editor should act on.
pub enum Happened {
    Recording,
    Paused,
    Resumed,
    /// A take is finished; its clicks are noted beside the screen file.
    Stopped(Result<Take, String>),
    Warning(String),
    Failed(String),
}

impl Live {
    pub fn start(config: Value, area: Option<Source>) -> Result<Live> {
        let mut child = Command::new(helper()?)
            .arg("run")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting splice-capture")?;
        let mut stdin = child.stdin.take().expect("piped");
        writeln!(stdin, "{config}")?;
        let mut stdout = child.stdout.take().expect("piped");
        let (tx, messages) = channel();
        let camera_part = Arc::new(std::sync::Mutex::new(None));
        let wanted = camera_part.clone();
        std::thread::Builder::new()
            .name("splice-capture".into())
            .spawn(move || {
                let mut head = [0u8; 5];
                while stdout.read_exact(&mut head).is_ok() {
                    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
                    let mut body = vec![0u8; len];
                    if stdout.read_exact(&mut body).is_err() {
                        break;
                    }
                    let message = match head[0] {
                        b'J' => match serde_json::from_slice(&body) {
                            Ok(v) => Message::Event(v),
                            Err(_) => continue,
                        },
                        b'F' if len >= 9 => {
                            let feed = if body[0] == 0 { Feed::Screen } else { Feed::Camera };
                            let w = u32::from_le_bytes([body[1], body[2], body[3], body[4]]);
                            let h = u32::from_le_bytes([body[5], body[6], body[7], body[8]]);
                            if w == 0 || h == 0 || body.len() != 9 + (w * h * 4) as usize {
                                continue;
                            }
                            body.drain(..9);
                            let part = match feed {
                                Feed::Camera => *wanted.lock().unwrap(),
                                Feed::Screen => None,
                            };
                            let (pixels, size) = match part {
                                Some(part) => cut(&body, (w, h), part),
                                None => (body, (w, h)),
                            };
                            match ffmpeg::render_image(size.0, size.1, pixels) {
                                Some(image) => Message::Frame(feed, image, (w, h), part),
                                None => continue,
                            }
                        }
                        _ => continue,
                    };
                    if tx.send(message).is_err() {
                        return;
                    }
                }
                let _ = tx.send(Message::Gone);
            })?;
        Ok(Live {
            child: Some(child),
            stdin,
            messages,
            config,
            frames: HashMap::new(),
            natural: HashMap::new(),
            parts: HashMap::new(),
            camera_part,
            levels: HashMap::new(),
            live: false,
            recording: None,
            problem: None,
            asking: None,
            area,
        })
    }

    fn command(&mut self, command: Value) {
        let _ = writeln!(self.stdin, "{command}");
        let _ = self.stdin.flush();
    }

    /// Starts writing the files into `dir`.
    pub fn record(&mut self, dir: PathBuf) -> Result<()> {
        if self.recording.is_some() {
            bail!("already recording");
        }
        std::fs::create_dir_all(&dir)?;
        self.command(json!({ "cmd": "record", "dir": dir }));
        let clicks = self.area.as_ref().and_then(recorder::watch_clicks);
        self.recording = Some(Rec {
            dir,
            zero: None,
            paused: None,
            pauses: Vec::new(),
            seconds: 0.0,
            clicks,
        });
        Ok(())
    }

    pub fn pause(&mut self) {
        self.command(json!({ "cmd": "pause" }));
    }

    pub fn resume(&mut self) {
        self.command(json!({ "cmd": "resume" }));
    }

    pub fn stop(&mut self) {
        self.command(json!({ "cmd": "stop" }));
    }

    /// How long has been recorded, not counting pauses.
    pub fn seconds(&self) -> f64 {
        let Some(rec) = &self.recording else { return 0.0 };
        match (rec.zero, rec.paused) {
            (Some(zero), None) => {
                let paused: f64 = rec.pauses.iter().map(|(a, b)| b.unwrap_or(*a) - a).sum();
                (now() - zero - paused).max(rec.seconds)
            }
            _ => rec.seconds,
        }
    }

    /// Takes in what the helper has said, and whether it said anything.
    /// `retire` gets frames no longer shown.
    pub fn poll(&mut self, retire: &mut Vec<Arc<RenderImage>>) -> (Vec<Happened>, bool) {
        let mut out = Vec::new();
        let mut heard = false;
        while let Ok(message) = self.messages.try_recv() {
            heard = true;
            match message {
                Message::Frame(feed, image, natural, part) => {
                    self.live = true;
                    self.natural.insert(feed, natural);
                    self.parts.insert(feed, part);
                    if let Some(old) = self.frames.insert(feed, image) {
                        retire.push(old);
                    }
                }
                Message::Gone => {
                    if self.problem.is_none() {
                        self.problem = Some("the recorder stopped".into());
                    }
                    out.push(Happened::Failed(self.problem.clone().unwrap_or_default()));
                    let seconds = self.seconds();
                    if let Some(rec) = self.recording.take() {
                        // Its files are written in pieces as it goes: what
                        // was written before it stopped still plays.
                        let file =
                            |name: &str| Some(rec.dir.join(name)).filter(|p| p.metadata().is_ok_and(|m| m.len() > 0));
                        let take = Take {
                            screen: file("screen.mp4"),
                            camera: file("camera.mp4"),
                            microphone: file("microphone.m4a"),
                            system: file("system.m4a"),
                            seconds,
                        };
                        out.push(Happened::Stopped(if take.files().is_empty() {
                            Err("the recorder stopped before anything was written".into())
                        } else {
                            Ok(take)
                        }));
                    }
                }
                Message::Event(e) => self.event(e, &mut out),
            }
        }
        (out, heard)
    }

    fn event(&mut self, e: Value, out: &mut Vec<Happened>) {
        let wall = e["wall"].as_f64();
        match e["event"].as_str().unwrap_or("") {
            "levels" => {
                self.live = true;
                for feed in ["microphone", "system"] {
                    match e[feed].as_f64() {
                        Some(v) => self.levels.insert(feed.into(), v as f32),
                        None => self.levels.remove(feed),
                    };
                }
                if let (Some(rec), Some(s)) = (&mut self.recording, e["seconds"].as_f64()) {
                    rec.seconds = s;
                }
            }
            "live" => self.live = true,
            // Every source has started: nothing is waiting on permission.
            "started" => self.asking = None,
            "asking" => self.asking = e["source"].as_str().map(str::to_string),
            "recording" => {
                if let Some(rec) = &mut self.recording {
                    rec.zero = wall;
                }
                out.push(Happened::Recording);
            }
            "paused" => {
                if let Some(rec) = &mut self.recording {
                    rec.paused = Some(Instant::now());
                    rec.seconds = e["seconds"].as_f64().unwrap_or(rec.seconds);
                    rec.pauses.push((wall.unwrap_or_else(now), None));
                }
                out.push(Happened::Paused);
            }
            "resumed" => {
                if let Some(rec) = &mut self.recording {
                    rec.paused = None;
                    if let Some(last) = rec.pauses.last_mut() {
                        last.1 = Some(wall.unwrap_or_else(now));
                    }
                }
                out.push(Happened::Resumed);
            }
            "stopped" => {
                let Some(mut rec) = self.recording.take() else { return };
                let clicks = rec.clicks.take().and_then(|(stop, watcher)| {
                    stop.store(true, Ordering::Relaxed);
                    watcher.join().ok()
                });
                out.push(Happened::Stopped(finished(&e, &rec, clicks)));
            }
            "warning" => out.push(Happened::Warning(e["message"].as_str().unwrap_or("").to_string())),
            "error" => {
                let message = plain(e["message"].as_str().unwrap_or("the recorder failed"));
                self.problem = Some(message.clone());
                out.push(Happened::Failed(message));
            }
            _ => {}
        }
    }

    pub fn running(&mut self) -> bool {
        self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // Closing its input finishes any file and ends it.
        let _ = writeln!(self.stdin, "{}", json!({ "cmd": "quit" }));
        let _ = self.stdin.flush();
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// The helper's errors, said so a person knows what to do.
fn plain(message: &str) -> String {
    if message.contains("declined TCC") {
        return "macOS stopped Splice recording the screen. Answer its prompt, or turn Splice (or the terminal it \
                was started from) on in System Settings → Privacy & Security → Screen Recording, then try again."
            .into();
    }
    message.to_string()
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The take from the helper's "stopped", its clicks written beside the
/// screen file in the take's own time.
fn finished(e: &Value, rec: &Rec, clicks: Option<Vec<Click>>) -> Result<Take, String> {
    if let Some(failed) = e["failed"].as_array().filter(|f| !f.is_empty()) {
        let said: Vec<&str> = failed.iter().filter_map(Value::as_str).collect();
        return Err(format!("some files could not be finished: {}", said.join("; ")));
    }
    let files = &e["files"];
    let file = |name: &str| files[name].as_str().map(PathBuf::from).filter(|p| p.exists());
    let take = Take {
        screen: file("screen"),
        camera: file("camera"),
        microphone: file("microphone"),
        system: file("system"),
        seconds: e["seconds"].as_f64().unwrap_or(0.0),
    };
    if take.files().is_empty() {
        return Err("nothing was recorded; check that the sources are live".into());
    }
    if let (Some(screen), Some(zero), Some(clicks)) = (&take.screen, rec.zero, clicks) {
        let pauses: Vec<(f64, f64)> = rec.pauses.iter().map(|(a, b)| (*a, b.unwrap_or(f64::MAX))).collect();
        let clicks = recorder::in_take(clicks, zero, &pauses, take.seconds);
        if !clicks.is_empty() {
            let _ = std::fs::write(
                recorder::clicks_file(screen),
                serde_json::to_string(&clicks).unwrap_or_default(),
            );
        }
    }
    let _ = std::fs::write(
        rec.dir.join("take.json"),
        serde_json::to_string_pretty(&take).unwrap_or_default(),
    );
    Ok(take)
}

/// A new folder for a take of `project`: `Recordings/<project>/Take <when>`.
pub fn take_dir(project: &Path) -> PathBuf {
    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());
    let mut dir = recorder::takes().join(name).join(format!("Take {}", recorder::stamp()));
    let mut n = 2;
    while dir.exists() {
        dir.set_file_name(format!("Take {} {n}", recorder::stamp()));
        n += 1;
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_of_a_picture_keeps_its_pixels() {
        // 4×2, each pixel's first byte its index.
        let bgra: Vec<u8> = (0..8u8).flat_map(|i| [i, 0, 0, 255]).collect();
        let (pixels, size) = cut(&bgra, (4, 2), [0.25, 0.5, 0.75, 1.0]);
        assert_eq!(size, (2, 1));
        assert_eq!(pixels.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), vec![5, 6]);
        // Nothing left still leaves a pixel, inside the picture.
        let (pixels, size) = cut(&bgra, (4, 2), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!((size, pixels[0]), ((1, 1), 7));
    }
}
