//! Everything that touches ffmpeg: probing, frame streams, single stills,
//! PCM for the mixer, filmstrips. ffmpeg runs as a child process so codec
//! support is whatever the installed build offers, and a crash in a decoder
//! never takes the editor with it.

use anyhow::{Context, Result, anyhow, bail};
use gpui::RenderImage;
use splice_core::model::MediaKind;
use splice_core::time::{SECOND, Time, from_secs, secs};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "heic", "heif", "avif",
];

/// Finds an ffmpeg tool. Apps started from Finder get a bare PATH, so the
/// usual Homebrew prefixes are searched too. `SPLICE_FFMPEG_DIR` overrides.
pub fn tool(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("SPLICE_FFMPEG_DIR") {
        dirs.push(dir.into());
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn ffmpeg() -> Result<&'static Path> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| tool("ffmpeg"))
        .as_deref()
        .ok_or_else(|| anyhow!("ffmpeg was not found (brew install ffmpeg)"))
}

fn ffprobe() -> Result<&'static Path> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| tool("ffprobe"))
        .as_deref()
        .ok_or_else(|| anyhow!("ffprobe was not found (brew install ffmpeg)"))
}

pub fn available() -> bool {
    ffmpeg().is_ok() && ffprobe().is_ok()
}

fn base_command() -> Result<Command> {
    let mut command = Command::new(ffmpeg()?);
    command.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
    Ok(command)
}

/// Runs what an export needs made first (reversed pieces), one at a time,
/// stopping when `going` says to. Call `tidy` once the export is over.
pub fn prepare(plan: &splice_core::export::Plan, going: impl Fn() -> bool) -> Result<()> {
    if let Some(dir) = &plan.scratch {
        std::fs::create_dir_all(dir)?;
    }
    for (path, text) in &plan.files {
        std::fs::write(path, text)?;
    }
    for command in &plan.prepare {
        if !going() {
            bail!("cancelled");
        }
        let out = export_command()?.args(command).stdout(Stdio::null()).output()?;
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stderr);
            bail!("{}", text.lines().last().unwrap_or("could not reverse a clip"));
        }
    }
    Ok(())
}

/// Removes what `prepare` made.
pub fn tidy(plan: &splice_core::export::Plan) {
    if let Some(dir) = &plan.scratch {
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub fn export_command() -> Result<Command> {
    Ok(Command::new(ffmpeg()?))
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub kind: MediaKind,
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

pub fn probe(path: &Path) -> Result<Probe> {
    let output = Command::new(ffprobe()?)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .context("running ffprobe")?;
    if !output.status.success() {
        bail!("{} is not a video, image or sound file", file_name(path));
    }
    let info: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let streams = info["streams"].as_array().cloned().unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0);
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let duration = info["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| video.and_then(|v| v["duration"].as_str()).and_then(|d| d.parse().ok()))
        .map(from_secs)
        .unwrap_or(0);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if let Some(video) = video {
        let (mut width, mut height) = (
            video["width"].as_u64().unwrap_or(0) as u32,
            video["height"].as_u64().unwrap_or(0) as u32,
        );
        // Phones record sideways and tag a rotation; ffmpeg applies it on decode.
        let rotation = video["side_data_list"]
            .as_array()
            .and_then(|list| list.iter().find_map(|d| d["rotation"].as_f64()))
            .or_else(|| video["tags"]["rotate"].as_str().and_then(|r| r.parse().ok()))
            .unwrap_or(0.0);
        if (rotation.abs() as i64) % 180 == 90 {
            std::mem::swap(&mut width, &mut height);
        }
        let still = IMAGE_EXTENSIONS.contains(&ext.as_str()) || duration < SECOND / 20;
        if still {
            return Ok(Probe {
                kind: MediaKind::Image,
                duration: 0,
                width,
                height,
                has_audio: false,
            });
        }
        return Ok(Probe {
            kind: MediaKind::Video,
            duration,
            width,
            height,
            has_audio,
        });
    }
    if has_audio && duration > 0 {
        return Ok(Probe {
            kind: MediaKind::Audio,
            duration,
            width: 0,
            height: 0,
            has_audio: true,
        });
    }
    bail!("{} has no picture or sound Splice can use", file_name(path))
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// The largest even size with the source's shape that fits in `max_w` x `max_h`.
pub fn fit(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (max_w.max(2) & !1, max_h.max(2) & !1);
    }
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let even = |v: f64| ((v.round() as u32).max(2)) & !1;
    (even(src_w as f64 * scale), even(src_h as f64 * scale))
}

fn with_filters(base: String, extra: Option<&str>) -> String {
    match extra {
        Some(extra) => format!("{base},{extra}"),
        None => base,
    }
}

/// BGRA bytes as an image gpui can paint.
pub fn render_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// One decoded frame of `path` at `at`, scaled to `width` x `height`.
/// `filters` (a key or mask) run on the frame after scaling.
pub fn still(path: &Path, at: Time, width: u32, height: u32, filters: Option<&str>) -> Result<Arc<RenderImage>> {
    let mut command = base_command()?;
    if at > 0 {
        command.args(["-ss", &format!("{:.6}", secs(at))]);
    }
    let output = command
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-an", "-sn", "-vf"])
        .arg(with_filters(format!("scale={width}:{height}:flags=bilinear"), filters))
        .args(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
        .stderr(Stdio::null())
        .output()?;
    let expected = (width * height * 4) as usize;
    if output.stdout.len() < expected {
        // Past the picture's end (its sound runs on): the last frame there is.
        return last_still(path, at, width, height, filters);
    }
    let mut bytes = output.stdout;
    bytes.truncate(expected);
    render_image(width, height, bytes).ok_or_else(|| anyhow!("bad frame"))
}

/// The last frame at or before `at`: read backwards from just after it.
fn last_still(path: &Path, at: Time, width: u32, height: u32, filters: Option<&str>) -> Result<Arc<RenderImage>> {
    let from = (at - SECOND).max(0);
    let output = base_command()?
        .args([
            "-ss",
            &format!("{:.6}", secs(from)),
            "-t",
            &format!("{:.6}", secs(at - from + SECOND / 10)),
        ])
        .arg("-i")
        .arg(path)
        .args(["-an", "-sn", "-vf"])
        .arg(with_filters(
            format!("reverse,trim=end_frame=1,scale={width}:{height}:flags=bilinear"),
            filters,
        ))
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
        .stderr(Stdio::null())
        .output()?;
    let expected = (width * height * 4) as usize;
    if output.stdout.len() < expected {
        bail!("no frame at {:.2}s in {}", secs(at), file_name(path));
    }
    let mut bytes = output.stdout;
    bytes.truncate(expected);
    render_image(width, height, bytes).ok_or_else(|| anyhow!("bad frame"))
}

/// Evenly spaced small frames for a clip's filmstrip: `(source time, image)`.
pub fn filmstrip(path: &Path, duration: Time, width: u32, height: u32) -> Result<Vec<(Time, Arc<RenderImage>)>> {
    let count = (secs(duration) / 2.0).ceil().clamp(1.0, 120.0) as usize;
    let interval = duration as f64 / count as f64;
    let rate = SECOND as f64 / interval;
    let mut child = base_command()?
        .arg("-i")
        .arg(path)
        .args(["-an", "-sn", "-vf"])
        .arg(format!("fps={rate:.6},scale={width}:{height}:flags=fast_bilinear"))
        .args(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let size = (width * height * 4) as usize;
    let mut frames = Vec::new();
    loop {
        let mut buffer = vec![0u8; size];
        if stdout.read_exact(&mut buffer).is_err() {
            break;
        }
        let t = (frames.len() as f64 * interval) as Time;
        if let Some(image) = render_image(width, height, buffer) {
            frames.push((t, image));
        }
    }
    let _ = child.wait();
    Ok(frames)
}

/// Interleaved stereo 16-bit PCM at `rate`.
pub struct Pcm {
    pub rate: u32,
    pub samples: Vec<i16>,
}

pub fn decode_audio(path: &Path, rate: u32) -> Result<Pcm> {
    let output = base_command()?
        .arg("-i")
        .arg(path)
        .args([
            "-vn",
            "-sn",
            "-ac",
            "2",
            "-ar",
            &rate.to_string(),
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stderr(Stdio::null())
        .output()?;
    let samples = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    Ok(Pcm { rate, samples })
}

/// The sound of `length` of `path` from `from`, run through `first` (a
/// voice's clean-up), played at `speed` (its pitch kept) and backwards when
/// `reverse`: what a retimed or cleaned clip sounds like.
pub fn decode_audio_retimed(
    path: &Path,
    rate: u32,
    from: Time,
    length: Time,
    first: Vec<String>,
    speed: f64,
    reverse: bool,
) -> Result<Pcm> {
    let mut filters = first;
    if reverse {
        filters.push("areverse".to_string());
    }
    // Each atempo stretches by at most a factor of two.
    let mut rest = speed;
    while rest < 0.5 - 1e-9 {
        filters.push("atempo=0.5".into());
        rest /= 0.5;
    }
    while rest > 2.0 + 1e-9 {
        filters.push("atempo=2.0".into());
        rest /= 2.0;
    }
    if (rest - 1.0).abs() > 1e-9 {
        filters.push(format!("atempo={rest:.6}"));
    }
    let mut command = base_command()?;
    if from > 0 {
        command.args(["-ss", &format!("{:.6}", secs(from))]);
    }
    command
        .args(["-t", &format!("{:.6}", secs(length))])
        .arg("-i")
        .arg(path);
    if !filters.is_empty() {
        command.args(["-af", &filters.join(",")]);
    }
    let output = command
        .args([
            "-vn",
            "-sn",
            "-ac",
            "2",
            "-ar",
            &rate.to_string(),
            "-f",
            "s16le",
            "pipe:1",
        ])
        .stderr(Stdio::null())
        .output()?;
    let samples = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    Ok(Pcm { rate, samples })
}

/// Loudness per `bucket` of time, 0–255, for waveforms.
pub fn peaks(pcm: &Pcm, bucket: Time) -> Vec<u8> {
    let per = ((pcm.rate as i64 * bucket / SECOND) as usize).max(1) * 2;
    pcm.samples
        .chunks(per)
        .map(|chunk| {
            let peak = chunk.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0;
            // A gentle curve so quiet passages still show.
            (peak.sqrt() * 255.0) as u8
        })
        .collect()
}

/// A running decoder: frames of one source from a start time onwards, at a
/// fixed rate and size. Dropping it stops ffmpeg.
pub struct FrameStream {
    /// The decoder now running; a stream going backwards runs one per piece.
    child: Arc<Mutex<Option<Child>>>,
    stop: Arc<AtomicBool>,
    frames: Receiver<Arc<RenderImage>>,
    /// Source time of the first frame.
    pub origin: Time,
    pub fps: u32,
    /// Source seconds per second of playback.
    pub speed: f64,
    /// Frames come from later to earlier source.
    pub reverse: bool,
    /// Frames taken so far.
    pub taken: u64,
}

/// How much source each backwards piece decodes: a piece is held whole to
/// be reversed, so pieces stay short.
const REVERSE_PIECE: Time = SECOND;

impl FrameStream {
    /// Frames of `path` from source time `from`, one per frame of playback
    /// at `fps`: `speed` source seconds a second, backwards when `reverse`.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        path: &Path,
        from: Time,
        fps: u32,
        width: u32,
        height: u32,
        filters: Option<&str>,
        speed: f64,
        reverse: bool,
    ) -> Result<FrameStream> {
        let speed = if speed.is_finite() && speed > 0.0 { speed } else { 1.0 };
        let vf = with_filters(
            format!("setpts=(PTS-STARTPTS)/{speed:.6},fps={fps},scale={width}:{height}:flags=fast_bilinear"),
            filters,
        );
        let decode = {
            let path = path.to_path_buf();
            let vf = vf.clone();
            move |at: Time, length: Option<Time>, backwards: bool| -> Result<Child> {
                let mut command = base_command()?;
                if at > 0 {
                    command.args(["-ss", &format!("{:.6}", secs(at))]);
                }
                if let Some(length) = length {
                    command.args(["-t", &format!("{:.6}", secs(length))]);
                }
                let vf = if backwards { format!("{vf},reverse") } else { vf.clone() };
                Ok(command
                    .arg("-i")
                    .arg(&path)
                    .args(["-an", "-sn", "-vf"])
                    .arg(vf)
                    .args(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()?)
            }
        };
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = sync_channel(8);
        if !reverse {
            let mut first = decode(from, None, false)?;
            let stdout = first.stdout.take().unwrap();
            *child.lock().unwrap() = Some(first);
            std::thread::Builder::new()
                .name("splice-decode".into())
                .spawn(move || {
                    pump(stdout, &tx, width, height);
                })?;
        } else {
            let (child, stop) = (child.clone(), stop.clone());
            std::thread::Builder::new()
                .name("splice-decode-back".into())
                .spawn(move || {
                    // Each piece ends where the last began, latest first; the
                    // first ends a frame past `from`, so its frame comes first.
                    let mut top = from + SECOND / fps.max(1) as Time;
                    while top > 0 && !stop.load(Ordering::Relaxed) {
                        let bottom = (top - REVERSE_PIECE).max(0);
                        let Ok(mut piece) = decode(bottom, Some(top - bottom), true) else {
                            break;
                        };
                        let stdout = piece.stdout.take().unwrap();
                        if let Some(mut done) = child.lock().unwrap().replace(piece) {
                            let _ = done.wait();
                        }
                        if !pump(stdout, &tx, width, height) {
                            break;
                        }
                        top = bottom;
                    }
                    // A piece started as the stream was dropped is ended here.
                    if let Some(mut last) = child.lock().unwrap().take() {
                        let _ = last.kill();
                        let _ = last.wait();
                    }
                })?;
        }
        Ok(FrameStream {
            child,
            stop,
            frames: rx,
            origin: from,
            fps,
            speed,
            reverse,
            taken: 0,
        })
    }

    /// Source time of the next frame this stream will yield.
    pub fn next_time(&self) -> Time {
        let along = (self.taken as f64 * SECOND as f64 * self.speed / self.fps as f64) as Time;
        if self.reverse {
            self.origin - along
        } else {
            self.origin + along
        }
    }

    /// The next frame if it is decoded already.
    pub fn try_next(&mut self) -> Option<Arc<RenderImage>> {
        let frame = self.frames.try_recv().ok()?;
        self.taken += 1;
        Some(frame)
    }
}

/// Passes frames on until the decoder ends; false when nobody wants more.
fn pump(mut stdout: std::process::ChildStdout, tx: &SyncSender<Arc<RenderImage>>, width: u32, height: u32) -> bool {
    let size = (width * height * 4) as usize;
    loop {
        let mut buffer = vec![0u8; size];
        if stdout.read_exact(&mut buffer).is_err() {
            return true;
        }
        let Some(image) = render_image(width, height, buffer) else {
            return true;
        };
        if tx.send(image).is_err() {
            return false;
        }
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes a 2s 64x36 test clip with a tone; None when ffmpeg is missing.
    fn sample(dir: &Path) -> Option<PathBuf> {
        if !available() {
            return None;
        }
        let path = dir.join("sample.mp4");
        let status = Command::new(ffmpeg().ok()?)
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=64x36:r=25:d=2",
                "-f",
                "lavfi",
            ])
            .args([
                "-i",
                "sine=f=440:d=2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&path)
            .status()
            .ok()?;
        status.success().then_some(path)
    }

    #[test]
    fn decodes_what_playback_needs() {
        let dir = std::env::temp_dir().join(format!("splice-ffmpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let Some(path) = sample(&dir) else { return };

        let probe = probe(&path).unwrap();
        assert_eq!(probe.kind, MediaKind::Video);
        assert_eq!((probe.width, probe.height), (64, 36));
        assert!(probe.has_audio);
        assert!((probe.duration - 2 * SECOND).abs() < SECOND / 10);

        assert!(still(&path, SECOND, 32, 18, None).is_ok());

        let mut stream = FrameStream::open(&path, SECOND / 2, 30, 32, 18, None, 1.0, false).unwrap();
        let start = std::time::Instant::now();
        let mut frames = 0;
        while frames < 10 && start.elapsed() < std::time::Duration::from_secs(5) {
            if stream.try_next().is_some() {
                frames += 1;
            }
        }
        assert_eq!(frames, 10);
        assert_eq!(stream.next_time(), SECOND / 2 + 10 * SECOND / 30);

        // Backwards, across pieces, and at another speed.
        let mut back = FrameStream::open(&path, 3 * SECOND / 2, 30, 32, 18, None, 2.0, true).unwrap();
        let start = std::time::Instant::now();
        let mut frames = 0;
        while frames < 20 && start.elapsed() < std::time::Duration::from_secs(5) {
            if back.try_next().is_some() {
                frames += 1;
            }
        }
        assert_eq!(frames, 20, "the second piece follows the first");
        assert_eq!(back.next_time(), 3 * SECOND / 2 - 20 * 2 * SECOND / 30);

        assert_eq!(filmstrip(&path, probe.duration, 32, 18).unwrap().len(), 1);

        let pcm = decode_audio(&path, 48_000).unwrap();
        assert!((pcm.samples.len() as i64 / 2 - 96_000).abs() < 4_800);
        assert!(
            peaks(&pcm, SECOND / 100).iter().any(|&p| p > 40),
            "a 1/8-amplitude tone shows as about 90"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fits_keep_shape() {
        assert_eq!(fit(1920, 1080, 640, 640), (640, 360));
        assert_eq!(fit(1080, 1920, 640, 360), (202, 360));
    }
}
