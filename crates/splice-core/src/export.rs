//! Export: the timeline as one ffmpeg invocation.
//!
//! The picture starts as a solid black canvas. Each visible clip, bottom
//! track first, is an input trimmed by input seeking; its key and mask run
//! on its own pixels, then it is sized, faded, shifted to its timeline start
//! and overlaid where its look puts it (both can move over time). Titles and
//! colours arrive as pictures the app drew. Sound from every audible clip is
//! delayed to its start and mixed over silence.

use crate::look::MotionKind;
use crate::model::{Clip, Generator, Media, MediaKind, Project, Source};
use crate::time::{SECOND, Time, secs};
use std::path::{Path, PathBuf};

/// Overlapping loud clips are held under full scale instead of clipping: a
/// look-ahead limiter at -0.25 dBFS that only acts on peaks. `level=0` keeps it
/// from raising quiet mixes; `latency=1` keeps sound in step with picture.
const LIMITER: &str = "alimiter=limit=0.97:attack=5:release=80:level=0:latency=1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Small files, quick encode.
    Draft,
    Standard,
    High,
}

impl Quality {
    fn crf(self) -> &'static str {
        match self {
            Quality::Draft => "28",
            Quality::Standard => "20",
            Quality::High => "16",
        }
    }

    fn preset(self) -> &'static str {
        match self {
            Quality::Draft => "veryfast",
            Quality::Standard => "medium",
            Quality::High => "slow",
        }
    }
}

#[derive(Debug)]
pub struct Plan {
    pub args: Vec<String>,
    pub duration: Time,
    /// ffmpeg runs to make, one after another, before `args`: the pieces of
    /// reversed clips, each reversed on its own so memory stays small.
    pub prepare: Vec<Vec<String>>,
    /// Files to write before running (into `scratch`): the commands that
    /// change a clip's opacity frame by frame.
    pub files: Vec<(PathBuf, String)>,
    /// Where those pieces go; remove it once the export is done.
    pub scratch: Option<PathBuf>,
}

/// Work done before the export proper, and where it puts its files.
struct Prepare {
    commands: Vec<Vec<String>>,
    files: Vec<(PathBuf, String)>,
    dir: PathBuf,
    used: bool,
}

/// A title or colour drawn by the app: a PNG at canvas resolution, with any
/// mask already applied.
pub struct Rendered {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// Where export finds its pictures.
pub trait Resolve {
    /// A readable file for a piece of media.
    fn media(&self, media: &Media) -> Option<PathBuf>;
    /// The picture for a title or colour clip.
    fn generated(&self, clip: &Clip, generator: &Generator) -> Option<Rendered>;
}

/// Builds the ffmpeg arguments that render `project` to `output` (an `.mp4`).
pub fn plan(project: &Project, resolve: &impl Resolve, output: &Path, quality: Quality) -> Result<Plan, String> {
    if project.duration() <= 0 {
        return Err("the timeline is empty".into());
    }
    build(project, resolve, output, Some(Out::Movie(quality, None)), None, false)
}

/// Builds the ffmpeg arguments that render just `from` to `to` of `project`
/// to `output`. The movie is drawn up to `to` and the stretch cut from it,
/// so every clip moves exactly as it does there (an entrance under way, a
/// slow zoom), and clips outside it are left out.
pub fn plan_range(
    project: &Project,
    resolve: &impl Resolve,
    output: &Path,
    quality: Quality,
    from: Time,
    to: Time,
) -> Result<Plan, String> {
    if to <= from {
        return Err("the stretch ends before it starts".into());
    }
    let part = project.window(from, to);
    if part.is_empty() {
        return Err("nothing is on the timeline there".into());
    }
    build(
        &part,
        resolve,
        output,
        Some(Out::Movie(quality, Some((from, to)))),
        None,
        false,
    )
}

/// Builds the ffmpeg arguments that render `project` to `output` (a `.mov`)
/// as a layer: ProRes 4444 that is see-through wherever nothing is drawn,
/// with its sound as PCM. This is how a symbol's timeline becomes one clip.
pub fn plan_layer(project: &Project, resolve: &impl Resolve, output: &Path) -> Result<Plan, String> {
    if project.duration() <= 0 {
        return Err("the timeline is empty".into());
    }
    build(project, resolve, output, Some(Out::Layer), None, true)
}

/// What a movie export writes.
#[derive(Clone, Copy)]
enum Out {
    /// A movie; with a window, just that stretch of it.
    Movie(Quality, Option<(Time, Time)>),
    Layer,
}

/// The picture at timeline time `t`, composited exactly as export would,
/// written as a still image (the format follows `output`'s extension).
pub fn plan_still(project: &Project, resolve: &impl Resolve, t: Time, output: &Path) -> Result<Plan, String> {
    still(project, resolve, t, output, false)
}

/// Like `plan_still`, see-through wherever nothing is drawn (write a PNG).
pub fn plan_still_layer(project: &Project, resolve: &impl Resolve, t: Time, output: &Path) -> Result<Plan, String> {
    still(project, resolve, t, output, true)
}

fn still(project: &Project, resolve: &impl Resolve, t: Time, output: &Path, see_through: bool) -> Result<Plan, String> {
    if t < 0 || t >= project.duration().max(1) {
        return Err(format!(
            "{:.3}s is outside the movie (0 to {:.3}s)",
            secs(t),
            secs(project.duration())
        ));
    }
    // Every clip stays (one drawn on past its edges for a transition shows
    // there too, and the transition needs its neighbours); build draws only
    // what is drawn at `t`.
    let mut moment = project.clone();
    for track in &mut moment.tracks {
        track.muted = true;
    }
    let mut plan = build(&moment, resolve, output, None, Some(t), see_through)?;
    plan.duration = project.frame();
    Ok(plan)
}

/// The size a clip has at scale 1 on a `w` x `h` canvas: media fitted inside
/// the canvas, a drawn picture as drawn.
pub fn natural_size(source: Source, rendered: Option<(u32, u32)>, w: u32, h: u32) -> (f64, f64) {
    match (source, rendered) {
        (Source::Generated(_), Some((rw, rh))) => (rw as f64, rh as f64),
        (Source::Media(m), _) if m.width > 0 && m.height > 0 => {
            let fit = (w as f64 / m.width as f64).min(h as f64 / m.height as f64);
            (m.width as f64 * fit, m.height as f64 * fit)
        }
        _ => (w as f64, h as f64),
    }
}

/// `still_at` renders the single frame at that timeline time instead of the movie.
fn build(
    project: &Project,
    resolve: &impl Resolve,
    output: &Path,
    out: Option<Out>,
    still_at: Option<Time>,
    see_through: bool,
) -> Result<Plan, String> {
    let layer = see_through;
    let frame = project.frame();
    let window = match out {
        Some(Out::Movie(_, w)) => w,
        _ => None,
    };
    let duration = if still_at.is_some() {
        frame
    } else if let Some((_, to)) = window {
        to.max(frame)
    } else {
        project.duration().max(frame)
    };
    let s = project.settings;
    let (w, h, fps) = (s.width, s.height, s.fps);
    let d = fmt(duration);

    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]
        .map(String::from)
        .to_vec();
    args.extend(strings(&[
        "-f",
        "lavfi",
        "-i",
        &if layer {
            format!("color=c=black@0.0:s={w}x{h}:r={fps}:d={d},format=rgba")
        } else {
            format!("color=c=black:s={w}x{h}:r={fps}:d={d}")
        },
    ]));
    args.extend(strings(&["-f", "lavfi", "-t", &d, "-i", "anullsrc=r=48000:cl=stereo"]));
    let mut next_input = 2usize;
    let mut prepare = Prepare {
        commands: Vec::new(),
        files: Vec::new(),
        dir: scratch_dir(),
        used: false,
    };

    let mut filters: Vec<String> = Vec::new();
    let mut picture = "0:v".to_string();
    let mut sounds: Vec<String> = Vec::new();

    for (track_index, track) in project.tracks.iter().enumerate() {
        for clip in &track.clips {
            let source = project.source(clip).ok_or("a clip refers to missing media")?;
            // A still draws only what is there at that moment.
            let drawn = (!track.hidden)
                .then(|| project.drawn(clip, track_index))
                .flatten()
                .filter(|d| still_at.is_none_or(|t| t >= d.from && t < d.to));
            let audio_media = match source {
                Source::Media(m) if m.has_audio => Some(m),
                _ => None,
            };
            let sounds_out =
                still_at.is_none() && !track.muted && audio_media.is_some() && clip.volume > 0.0 && !clip.frozen;
            let tag = format!("t{track_index}c{}", clip.id.0);

            if let Some(drawn) = &drawn {
                let look = &clip.look;
                // Where the picture's frames come from, ready at the frame rate
                // and covering every moment it is drawn, starting at `drawn.from`.
                let Frames {
                    label: frames,
                    rendered,
                    shared,
                } = clip_frames(
                    &mut args,
                    &mut prepare,
                    &mut filters,
                    &mut next_input,
                    project,
                    resolve,
                    clip,
                    drawn,
                    source,
                    still_at,
                    &tag,
                )?;
                let natural = natural_size(source, rendered, w, h);
                // Only media is cropped; titles and colours are drawn to size.
                let ((nw, nh), (cx, cy)) = match source {
                    Source::Media(_) => look.cropped(natural),
                    Source::Generated(_) => (natural, (0.0, 0.0)),
                };
                let pose = &drawn.pose;
                // Clip time, in seconds, of the picture's frames: they start
                // at `drawn.from`. A still knows its one moment.
                let local = |frame: &str| -> String {
                    let head = secs(drawn.head());
                    match still_at {
                        Some(t) => fmt(t - clip.start),
                        None if head > 0.0 => format!("({frame}/{fps}-{head:.6})"),
                        None => format!("({frame}/{fps})"),
                    }
                };
                let mut chain = Vec::new();
                if matches!(source, Source::Media(_))
                    && let Some(pixels) = look.pixel_filters()
                {
                    chain.push(pixels);
                }
                chain.push("format=rgba".into());
                // Coming up out of a colour.
                if let Some((color, a, z)) = drawn.tint {
                    match still_at {
                        Some(t) => {
                            let k = ((t - a) as f64 / (z - a).max(1) as f64).clamp(0.0, 1.0);
                            let (r, g, b) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
                            chain.push(format!(
                                "geq=r='r(X,Y)*{k:.4}+{:.3}':g='g(X,Y)*{k:.4}+{:.3}':b='b(X,Y)*{k:.4}+{:.3}':a='alpha(X,Y)'",
                                r as f64 * (1.0 - k),
                                g as f64 * (1.0 - k),
                                b as f64 * (1.0 - k)
                            ));
                        }
                        None => chain.push(format!(
                            "fade=t=in:st={}:d={}:color=0x{color:06x}",
                            fmt(a - drawn.from),
                            fmt(z - a)
                        )),
                    }
                }
                // A wipe shows part of the picture; outside it, all of it.
                let masks: Vec<String> = [&drawn.reveal, &drawn.conceal]
                    .into_iter()
                    .flatten()
                    .map(|r| r.ffmpeg(&local("N")))
                    .collect();
                if !masks.is_empty() {
                    let cut_window = [project.cut_into(clip.id), project.cut_out_of(clip.id)]
                        .into_iter()
                        .flatten()
                        .filter(|c| c.kind.dip().is_none())
                        .map(|c| (c.start - drawn.from, c.end - drawn.from))
                        .reduce(|a, z| (a.0.min(z.0), a.1.max(z.1)))
                        .unwrap_or((0, drawn.to - drawn.from));
                    let enable = match still_at {
                        Some(_) => String::new(),
                        None => format!(
                            ":enable='between(t,{},{})'",
                            fmt(cut_window.0 - project.frame()),
                            fmt(cut_window.1 + project.frame())
                        ),
                    };
                    chain.push(format!(
                        "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*{}'{enable}",
                        masks.join("*")
                    ));
                }
                // How much shows: a still knows exactly; a movie fades as
                // its motions do, or follows keyframes and transitions frame by frame.
                match (still_at, pose.opacity.constant()) {
                    (Some(t), _) => {
                        let o = drawn.pose_at(t).opacity;
                        if o < 1.0 {
                            chain.push(format!("colorchannelmixer=aa={:.4}", o.max(0.0)));
                        }
                    }
                    (None, Some(o)) => {
                        if o < 1.0 {
                            chain.push(format!("colorchannelmixer=aa={:.4}", o.max(0.0)));
                        }
                    }
                    (None, None)
                        if pose.opacity == look.pose_expr(clip.duration).opacity && look.keys.opacity.is_empty() =>
                    {
                        if look.opacity < 1.0 {
                            chain.push(format!("colorchannelmixer=aa={:.4}", look.opacity.max(0.0)));
                        }
                        for (motion, inward) in [(look.enter, true), (look.exit, false)] {
                            if motion.kind == MotionKind::None {
                                continue;
                            }
                            let fd = secs(motion.duration.min(clip.duration));
                            let head = secs(drawn.head());
                            let st = if inward { head } else { head + secs(clip.duration) - fd };
                            chain.push(format!(
                                "fade=t={}:st={st:.6}:d={fd:.6}:alpha=1",
                                if inward { "in" } else { "out" }
                            ));
                        }
                    }
                    (None, None) => {
                        // Sent to the mixer as each frame arrives.
                        let name = format!("colorchannelmixer@o{tag}");
                        let count = ((drawn.to - drawn.from) as f64 * fps as f64 / SECOND as f64).ceil() as i64;
                        let head = secs(drawn.head());
                        let mut said: Option<String> = None;
                        let mut commands = Vec::new();
                        for n in 0..count.max(1) {
                            let at = n as f64 / fps as f64;
                            let v = format!("{:.4}", pose.opacity.eval(at - head).clamp(0.0, 1.0));
                            if said.as_ref() != Some(&v) {
                                commands.push(format!("{at:.6} {name} aa {v}"));
                                said = Some(v);
                            }
                        }
                        let first = pose.opacity.eval(-head).clamp(0.0, 1.0);
                        // In a file: a long clip has a command for every frame.
                        let file = prepare.dir.join(format!("{tag}-opacity.txt"));
                        prepare.files.push((file.clone(), commands.join(";\n") + ";\n"));
                        prepare.used = true;
                        chain.push(format!("sendcmd=f='{}'", file.to_string_lossy().replace('\'', "")));
                        chain.push(format!("{name}=aa={first:.4}"));
                    }
                }
                let moving = still_at.is_none() && pose.moves();
                // A moving picture keeps one size and is placed anew each
                // frame to a fraction of a pixel. Resizing it every frame
                // rounded its size to whole pixels and moved its centre with
                // the rounding, so a slow zoom or slide jittered.
                let (pw, ph) = (even(nw.round()), even(nh.round()));
                // A clear border all round: perspective fills what lies outside
                // the picture with its edge pixels, so without one a picture
                // that shrinks or slides smeared its edges across the canvas.
                let (fw, fh) = (pw.max(w) + 2 * BORDER, ph.max(h) + 2 * BORDER);
                let fixed = match still_at {
                    Some(t) => Some(drawn.pose_at(t)),
                    None if !moving => Some(drawn.pose_at(clip.start)),
                    None => None,
                };
                match fixed {
                    Some(p) => chain.push(format!(
                        "scale={}:{}",
                        even((nw * p.scale).round()),
                        even((nh * p.scale).round())
                    )),
                    None => {
                        chain.push(format!("scale={pw}:{ph}"));
                        chain.push(format!("pad={fw}:{fh}:(ow-iw)/2:(oh-ih)/2:color=black@0"));
                    }
                }
                if moving {
                    // perspective has no clock of its own; its frame count is one.
                    let t = local("in");
                    let k = format!("max(0.0001,{})", pose.scale.ffmpeg(&t));
                    let along = |by: f64| {
                        if by.abs() < 1e-6 {
                            String::new()
                        } else {
                            format!("+{by:.3}*{k}")
                        }
                    };
                    // The picture's centre, in the frame it moves in.
                    let cx_at = format!("({fw}/2+{}*{w}{})", pose.x.ffmpeg(&t), along(cx));
                    let cy_at = format!("({fh}/2+{}*{h}{})", pose.y.ffmpeg(&t), along(cy));
                    let (hx, hy) = (format!("{}*{k}", fw as f64 / 2.0), format!("{}*{k}", fh as f64 / 2.0));
                    chain.push("format=yuva444p".into());
                    chain.push(format!(
                        "perspective=sense=destination:interpolation=cubic:eval=frame:\
                         x0='{cx_at}-{hx}':y0='{cy_at}-{hy}':x1='{cx_at}+{hx}':y1='{cy_at}-{hy}':\
                         x2='{cx_at}-{hx}':y2='{cy_at}+{hy}':x3='{cx_at}+{hx}':y3='{cy_at}+{hy}'"
                    ));
                }
                let placed_at = if still_at.is_some() { 0 } else { drawn.from };
                let start = fmt(placed_at);
                // Over a see-through canvas the overlay negotiates its format
                // from its inputs: a picture that changes size frame to frame
                // only stays right as RGBA (as yuva420p it froze at its first size).
                chain.push(format!(
                    "format={},setpts=PTS-STARTPTS+{start}/TB",
                    if layer { "rgba" } else { "yuva420p" }
                ));
                filters.push(format!("[{frames}]{}[v{tag}]", chain.join(",")));

                let end = fmt(if still_at.is_some() { frame } else { drawn.to });
                // A cropped picture's remains stay where they were, scaled with it.
                let (x, y) = if moving {
                    // Already placed in its frame.
                    (format!("-{}", (fw - w) / 2), format!("-{}", (fh - h) / 2))
                } else {
                    let p = fixed.expect("a still picture has a pose");
                    let shift = |by: f64| {
                        if by.abs() < 1e-6 {
                            String::new()
                        } else {
                            format!("+{:.3}", by * p.scale)
                        }
                    };
                    (
                        format!("(W-w)/2+{:.6}*W{}", p.x, shift(cx)),
                        format!("(H-h)/2+{:.6}*H{}", p.y, shift(cy)),
                    )
                };
                filters.push(format!(
                    "[{picture}][v{tag}]overlay=x='{x}':y='{y}':{}eof_action=pass:\
                     enable='between(t,{start},{end})'[p{tag}]",
                    // Over a see-through canvas, keep what is see-through.
                    if layer { "format=auto:" } else { "" }
                ));
                picture = format!("p{tag}");

                if sounds_out && let (Some(shared), Some(media)) = (shared, audio_media) {
                    sounds.push(clip_sound(
                        &mut filters,
                        &mut args,
                        &mut next_input,
                        resolve,
                        clip,
                        media,
                        Some(shared),
                        project.sound(clip),
                        &tag,
                    )?);
                }
            } else if sounds_out && let Some(media) = audio_media {
                sounds.push(clip_sound(
                    &mut filters,
                    &mut args,
                    &mut next_input,
                    resolve,
                    clip,
                    media,
                    None,
                    project.sound(clip),
                    &tag,
                )?);
            }

            // A dip's colour, over the clips either side of its cut.
            if let Some(dip) = (!track.hidden)
                .then(|| project.dip_into(clip, track_index))
                .flatten()
                .filter(|d| still_at.is_none_or(|t| t >= d.from && t < d.to))
            {
                let len = if still_at.is_some() { frame } else { dip.to - dip.from };
                args.extend(strings(&[
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=c=0x{:06x}:s={w}x{h}:r={fps}:d={}", dip.color, fmt(len)),
                ]));
                let i = next_input;
                next_input += 1;
                let mut chain = vec!["format=rgba".to_string()];
                match still_at {
                    Some(t) => chain.push(format!("colorchannelmixer=aa={:.4}", dip.opacity(t))),
                    None => {
                        let half = secs(len) / 2.0;
                        chain.push(format!("fade=t=in:st=0:d={half:.6}:alpha=1"));
                        chain.push(format!("fade=t=out:st={half:.6}:d={half:.6}:alpha=1"));
                    }
                }
                let placed = if still_at.is_some() { 0 } else { dip.from };
                chain.push(format!(
                    "format={},setpts=PTS-STARTPTS+{}/TB",
                    if layer { "rgba" } else { "yuva420p" },
                    fmt(placed)
                ));
                filters.push(format!("[{i}:v]{}[d{tag}]", chain.join(",")));
                filters.push(format!(
                    "[{picture}][d{tag}]overlay=x=0:y=0:{}eof_action=pass:enable='between(t,{},{})'[q{tag}]",
                    if layer { "format=auto:" } else { "" },
                    fmt(placed),
                    fmt(placed + len)
                ));
                picture = format!("q{tag}");
            }
        }
    }

    // Just a stretch: cut from the movie as drawn, then start it at zero.
    let (cut, cut_sound) = match window {
        Some((a, b)) => (
            format!("trim=start={}:end={},setpts=PTS-STARTPTS,", fmt(a), fmt(b)),
            format!(",atrim=start={}:end={},asetpts=PTS-STARTPTS", fmt(a), fmt(b)),
        ),
        None => (String::new(), String::new()),
    };
    filters.push(format!(
        "[{picture}]{cut}format={}[vout]",
        if layer { "yuva444p10le" } else { "yuv420p" }
    ));
    if out.is_some() {
        filters.push(format!(
            "[1:a]{}amix=inputs={}:normalize=0:duration=first,{LIMITER}{cut_sound}[aout]",
            sounds.join(""),
            sounds.len() + 1
        ));
    }
    let (duration, d) = match window {
        Some((a, b)) => (b - a, fmt(b - a)),
        None => (duration, d),
    };

    args.extend(strings(&["-filter_complex", &filters.join(";")]));
    let Some(out) = out else {
        args.extend(strings(&["-map", "[vout]", "-frames:v", "1", "-update", "1"]));
        args.push(output.to_string_lossy().to_string());
        return Ok(Plan {
            args,
            duration,
            prepare: prepare.commands,
            files: prepare.files,
            scratch: prepare.used.then_some(prepare.dir),
        });
    };
    let Out::Movie(quality, _) = out else {
        args.extend(strings(&[
            "-map",
            "[vout]",
            "-map",
            "[aout]",
            "-c:v",
            "prores_ks",
            "-profile:v",
            "4444",
            "-pix_fmt",
            "yuva444p10le",
            "-r",
            &fps.to_string(),
            "-c:a",
            "pcm_s16le",
            "-t",
            &d,
            "-progress",
            "pipe:1",
            "-nostats",
        ]));
        args.push(output.to_string_lossy().to_string());
        return Ok(Plan {
            args,
            duration,
            prepare: prepare.commands,
            files: prepare.files,
            scratch: prepare.used.then_some(prepare.dir),
        });
    };
    args.extend(strings(&[
        "-map",
        "[vout]",
        "-map",
        "[aout]",
        "-c:v",
        "libx264",
        "-preset",
        quality.preset(),
        "-crf",
        quality.crf(),
        "-r",
        &fps.to_string(),
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-t",
        &d,
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:1",
        "-nostats",
    ]));
    args.push(output.to_string_lossy().to_string());
    Ok(Plan {
        args,
        duration,
        prepare: prepare.commands,
        files: prepare.files,
        scratch: prepare.used.then_some(prepare.dir),
    })
}

/// A clip's pictures, ready to place: the stream's label, the size of a
/// picture the app drew, and the input its sound can share (with the source
/// time that input starts at).
struct Frames {
    label: String,
    rendered: Option<(u32, u32)>,
    shared: Option<(usize, Time)>,
}

/// A fresh folder for an export's own files, in the system's temporary folder.
fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "splice-export-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The clear margin around a moving picture, in pixels each side.
const BORDER: u32 = 2;

/// How much of a reversed clip's source each of its pieces holds: reversing
/// keeps a whole piece in memory, so a long clip goes backwards in pieces.
const REVERSE_PIECE: Time = 2 * SECOND;

/// Adds the input (or inputs) a visual clip's pictures come from, and the
/// filters that bring them to the frame rate over every moment it is drawn,
/// from `drawn.from` on: played at its speed, backwards when reversed, held
/// still where the media runs out.
#[allow(clippy::too_many_arguments)]
fn clip_frames(
    args: &mut Vec<String>,
    prepare: &mut Prepare,
    filters: &mut Vec<String>,
    next_input: &mut usize,
    project: &Project,
    resolve: &impl Resolve,
    clip: &Clip,
    drawn: &crate::compose::Drawn,
    source: Source,
    still_at: Option<Time>,
    tag: &str,
) -> Result<Frames, String> {
    let fps = project.settings.fps;
    let frame = project.frame();
    let length = match still_at {
        Some(_) => frame,
        None => drawn.to - drawn.from,
    };
    let len = fmt(length);
    let label = format!("f{tag}");
    let mut take = || {
        let i = *next_input;
        *next_input += 1;
        i
    };
    let looped = |args: &mut Vec<String>, path: &str| {
        args.extend(strings(&[
            "-loop",
            "1",
            "-framerate",
            &fps.to_string(),
            "-t",
            &len,
            "-i",
            path,
        ]));
    };
    let media = match source {
        Source::Generated(generator) => {
            let picture = resolve
                .generated(clip, generator)
                .ok_or_else(|| format!("{} could not be drawn", generator.name()))?;
            looped(args, &picture.path.to_string_lossy());
            let i = take();
            filters.push(format!("[{i}:v]fps={fps},setpts=PTS-STARTPTS[{label}]"));
            return Ok(Frames {
                label,
                rendered: Some((picture.width, picture.height)),
                shared: None,
            });
        }
        Source::Media(media) => media,
    };
    let path = resolve
        .media(media)
        .ok_or_else(|| format!("{} is offline", media.name))?
        .to_string_lossy()
        .to_string();
    if media.kind == MediaKind::Image {
        looped(args, &path);
        let i = take();
        filters.push(format!("[{i}:v]fps={fps},setpts=PTS-STARTPTS[{label}]"));
        return Ok(Frames {
            label,
            rendered: None,
            shared: None,
        });
    }
    let last = (media.duration - frame).max(0);
    // One frame (a still, or a held frame): the last frame at or before
    // that moment, read backwards from just after it, so it is found even
    // where the file's sound runs on past its picture.
    if still_at.is_some() || clip.frozen {
        let at = match still_at {
            Some(t) => clip.source_at(t),
            None => clip.source_in,
        }
        .clamp(0, last);
        let from = (at - SECOND).max(0);
        args.extend(strings(&[
            "-ss",
            &fmt(from),
            "-t",
            &fmt(at - from + frame),
            "-i",
            &path,
        ]));
        let i = take();
        let hold = match still_at {
            Some(_) => String::new(),
            None => format!(",tpad=stop_mode=clone:stop_duration={len},trim=duration={len},setpts=PTS-STARTPTS"),
        };
        filters.push(format!(
            "[{i}:v]reverse,trim=end_frame=1,setpts=PTS-STARTPTS,fps={fps}{hold}[{label}]"
        ));
        return Ok(Frames {
            label,
            rendered: None,
            shared: None,
        });
    }
    let speed = clip.speed;
    let source_of = |t: Time| (t as f64 * speed).round() as Time;
    let (head, tail) = (drawn.head(), drawn.tail());
    // The source the drawn stretch wants, and the part of it the media has.
    let (want_lo, want_hi) = if clip.reverse {
        (clip.source_in - source_of(tail), clip.source_out() + source_of(head))
    } else {
        (clip.source_in - source_of(head), clip.source_out() + source_of(tail))
    };
    let lo = want_lo.clamp(0, last);
    let hi = want_hi.clamp(lo + frame, media.duration.max(lo + frame));
    // What the media lacks is held still, before and after.
    let timeline = |t: Time| secs(t.max(0)) / speed;
    let (early, late) = (timeline(lo - want_lo), timeline(want_hi - hi));
    let (before, after) = if clip.reverse { (late, early) } else { (early, late) };
    let retime = if speed == 1.0 {
        "setpts=PTS-STARTPTS".to_string()
    } else {
        format!("setpts=(PTS-STARTPTS)/{speed:.6}")
    };
    // Padding past the end too, so a file a little shorter than it says
    // still fills the clip; the trim makes the length exact.
    let pad = format!(
        "tpad=start_mode=clone:start_duration={before:.6}:stop_mode=clone:stop_duration={:.6}",
        after + 0.5
    );
    let finish = format!("{retime},fps={fps},{pad},trim=duration={len},setpts=PTS-STARTPTS[{label}]");
    if !clip.reverse {
        args.extend(strings(&["-ss", &fmt(lo), "-t", &fmt(hi - lo), "-i", &path]));
        let i = take();
        filters.push(format!("[{i}:v]{finish}"));
        return Ok(Frames {
            label,
            rendered: None,
            shared: Some((i, lo)),
        });
    }
    // Backwards: the latest piece first, each reversed on its own into a
    // file before the export (reversing holds a piece in memory, and every
    // input of one ffmpeg decodes at once), padded to its exact length so
    // the pieces stay in step with the sound. Then joined.
    let see_through = matches!(
        &media.recipe,
        Some(crate::model::Recipe::Symbol(_))
            | Some(crate::model::Recipe::Scene(crate::model::Scene {
                transparent: true,
                ..
            }))
    );
    let mut pieces = Vec::new();
    let mut top = hi;
    while top > lo {
        let bottom = (top - REVERSE_PIECE).max(lo);
        let length = fmt(top - bottom);
        let file = prepare.dir.join(format!("{tag}-{}.mov", pieces.len()));
        let mut command = strings(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
        command.extend(strings(&[
            "-ss",
            &fmt(bottom),
            "-t",
            &length,
            "-i",
            &path,
            "-an",
            "-vf",
        ]));
        command.push(format!(
            "setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration={length},trim=duration={length},reverse"
        ));
        command.extend(if see_through {
            strings(&["-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le"])
        } else {
            strings(&[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "10",
                "-g",
                "1",
                "-pix_fmt",
                "yuv420p",
            ])
        });
        command.push(file.to_string_lossy().to_string());
        prepare.commands.push(command);
        pieces.push(file);
        top = bottom;
    }
    // Joined into one file (copied, not encoded again), so the export
    // opens one input however long the clip.
    let list = prepare.dir.join(format!("{tag}-pieces.txt"));
    let joined = prepare.dir.join(format!("{tag}-reversed.mov"));
    prepare.files.push((
        list.clone(),
        pieces
            .iter()
            .map(|f| format!("file '{}'\n", f.to_string_lossy().replace('\'', "'\\''")))
            .collect(),
    ));
    let mut join = strings(&["-hide_banner", "-nostdin", "-y", "-loglevel", "error"]);
    join.extend(strings(&[
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        &list.to_string_lossy(),
        "-c",
        "copy",
    ]));
    join.push(joined.to_string_lossy().to_string());
    prepare.commands.push(join);
    prepare.used = true;
    args.extend(strings(&["-i", &joined.to_string_lossy()]));
    let i = take();
    filters.push(format!("[{i}:v]setpts=PTS-STARTPTS,{finish}"));
    Ok(Frames {
        label,
        rendered: None,
        shared: None,
    })
}

/// The sound of a clip, at its speed and direction, delayed to its start.
/// `shared` is an input already open on its file (and the source time it
/// starts at); without one, the clip gets its own. Returns the stream's label.
#[allow(clippy::too_many_arguments)]
fn clip_sound(
    filters: &mut Vec<String>,
    args: &mut Vec<String>,
    next_input: &mut usize,
    resolve: &impl Resolve,
    clip: &Clip,
    media: &Media,
    shared: Option<(usize, Time)>,
    sound: crate::compose::Sound,
    tag: &str,
) -> Result<String, String> {
    // A retimed clip plays its own stretch; one as recorded may run on past
    // its edges for a crossfade (`sound`).
    let (source, length) = if clip.is_retimed() {
        (clip.source_in, clip.span())
    } else {
        (sound.source, sound.to - sound.from)
    };
    let (i, trim) = match shared.filter(|_| !clip.reverse) {
        Some((i, from)) => (
            i,
            format!("atrim=start={}:duration={},", fmt((source - from).max(0)), fmt(length)),
        ),
        None => {
            let path = resolve
                .media(media)
                .ok_or_else(|| format!("{} is offline", media.name))?;
            args.extend(strings(&[
                "-ss",
                &fmt(source),
                "-t",
                &fmt(length),
                "-i",
                &path.to_string_lossy(),
            ]));
            let i = *next_input;
            *next_input += 1;
            (i, String::new())
        }
    };
    let mut chain = vec![format!("{trim}aresample=48000"), "asetpts=PTS-STARTPTS".to_string()];
    if clip.reverse {
        chain.push("areverse".into());
    }
    // atempo keeps the pitch; each one stretches by at most a factor of two.
    let mut rest = clip.speed;
    while rest < 0.5 - 1e-9 {
        chain.push("atempo=0.5".into());
        rest /= 0.5;
    }
    while rest > 2.0 + 1e-9 {
        chain.push("atempo=2.0".into());
        rest /= 2.0;
    }
    if (rest - 1.0).abs() > 1e-9 {
        chain.push(format!("atempo={rest:.6}"));
    }
    chain.push(format!("volume={:.3}", clip.volume));
    for (fade, direction) in [(sound.fade_in, "in"), (sound.fade_out, "out")] {
        if let Some((a, z)) = fade {
            chain.push(format!(
                "afade=t={direction}:st={}:d={}:curve=qsin",
                fmt((a - sound.from).max(0)),
                fmt((z - a).max(1))
            ));
        }
    }
    chain.push(format!("adelay={}:all=1", sound.from.max(0) / 1000));
    filters.push(format!("[{i}:a]{}[a{tag}]", chain.join(",")));
    Ok(format!("[a{tag}]"))
}

/// Reads one line of `-progress` output; returns the encoded position.
pub fn parse_progress(line: &str) -> Option<Time> {
    let value = line
        .strip_prefix("out_time_us=")
        .or_else(|| line.strip_prefix("out_time_ms="))?;
    value.trim().parse::<Time>().ok()
}

fn even(v: f64) -> u32 {
    ((v.max(2.0) as u32) / 2) * 2
}

fn fmt(t: Time) -> String {
    format!("{:.6}", secs(t))
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::{Drift, Key, Mask};
    use crate::model::{Generator, MediaId, Title};
    use crate::time::SECOND;

    struct Fake;

    impl Resolve for Fake {
        fn media(&self, media: &Media) -> Option<PathBuf> {
            Some(PathBuf::from(format!("/m/{}", media.hash)))
        }
        fn generated(&self, clip: &Clip, _: &Generator) -> Option<Rendered> {
            Some(Rendered {
                path: PathBuf::from(format!("/g/{}.png", clip.id.0)),
                width: 600,
                height: 120,
            })
        }
    }

    fn video(p: &mut Project, hash: &str, seconds: i64) -> MediaId {
        p.add_media(Media {
            id: MediaId(0),
            name: "v".into(),
            hash: hash.into(),
            ext: "mp4".into(),
            kind: MediaKind::Video,
            duration: seconds * SECOND,
            width: 1920,
            height: 1080,
            has_audio: true,
            source: PathBuf::new(),
            recipe: None,
            content: None,
        })
    }

    fn graph(plan: &Plan) -> &str {
        &plan.args[plan.args.iter().position(|a| a == "-filter_complex").unwrap() + 1]
    }

    #[test]
    fn plans_layers_and_sound() {
        let mut p = Project::default();
        let v = video(&mut p, "a", 4);
        let image = p.add_media(Media {
            id: MediaId(0),
            name: "i".into(),
            hash: "b".into(),
            ext: "png".into(),
            kind: MediaKind::Image,
            duration: 0,
            width: 500,
            height: 500,
            has_audio: false,
            source: PathBuf::new(),
            recipe: None,
            content: None,
        });
        p.append_clip(v);
        p.insert_clip(image, 0, SECOND);
        let plan = plan(&p, &Fake, Path::new("/o.mp4"), Quality::Draft).unwrap();
        let graph = graph(&plan);
        assert_eq!(graph.matches("overlay=").count(), 2);
        assert!(graph.contains("amix=inputs=2"));
        assert!(
            graph.contains("scale=1080:1080"),
            "the square image fits the 1920x1080 canvas: {graph}"
        );
        assert!(plan.args.contains(&"/m/b".to_string()));
        assert_eq!(plan.duration, 6 * SECOND, "the image runs 1s to 6s");
    }

    #[test]
    fn titles_keys_masks_and_motion() {
        let mut p = Project::default();
        let v = video(&mut p, "a", 4);
        let clip = p.append_clip(v).unwrap();
        p.set_look(clip, |look| {
            look.key = Some(Key::default());
            look.mask = Some(Mask::default());
            look.scale = 0.5;
            look.x = 0.25;
        });
        let title = p.insert_generated(Generator::Text(Title::default()), 1, 0, 2 * SECOND);
        p.set_look(title, |look| {
            look.enter.kind = MotionKind::Pop;
            look.exit.kind = MotionKind::Fade;
        });
        let plan = plan(&p, &Fake, Path::new("/o.mp4"), Quality::Draft).unwrap();
        let graph = graph(&plan);
        assert!(graph.contains("chromakey=0x00ff00"), "{graph}");
        assert!(graph.contains("geq="));
        assert!(graph.contains("scale=960:540"), "half of the fitted size");
        assert!(graph.contains("x='(W-w)/2+0.250000*W'"));
        assert!(plan.args.contains(&format!("/g/{}.png", title.0)));
        assert!(graph.contains(":eval=frame"), "the pop animates the title's size");
        assert!(graph.contains("fade=t=in:st=0.000000:d=0.500000:alpha=1"));
        assert!(graph.contains("fade=t=out:st=1.500000:d=0.500000:alpha=1"));
        assert_eq!(graph.matches("amix=inputs=2").count(), 1, "titles make no sound");
    }

    #[test]
    fn a_moving_picture_is_placed_to_a_fraction_of_a_pixel() {
        let mut p = Project::default();
        let v = video(&mut p, "a", 4);
        let clip = p.append_clip(v).unwrap();
        p.set_look(clip, |look| {
            look.drift = Drift::ZoomIn;
            look.x = 0.1;
        });
        let plan = plan(&p, &Fake, Path::new("/o.mp4"), Quality::Draft).unwrap();
        let graph = graph(&plan);
        // Resizing every frame rounds to whole pixels, and a slow zoom jitters.
        assert!(!graph.contains("trunc("), "no per-frame resize: {graph}");
        assert!(
            graph.contains("scale=1920:1080,pad=1924:1084:"),
            "a clear border: {graph}"
        );
        assert!(graph.contains("perspective=sense=destination:interpolation=cubic:eval=frame"));
        assert!(graph.contains("(in/30)"), "the zoom follows the frame count");
        assert!(graph.contains("overlay=x='-2':y='-2'"), "{graph}");
    }

    #[test]
    fn still_shows_what_is_under_the_time() {
        let mut p = Project::default();
        let v = video(&mut p, "a", 4);
        p.append_clip(v);
        let second = p.append_clip(v).unwrap();
        p.set_look(second, |look| look.enter.kind = MotionKind::Fade);
        let plan = plan_still(&p, &Fake, 4 * SECOND + SECOND / 4, Path::new("/o.png")).unwrap();
        let graph = graph(&plan);
        assert_eq!(
            graph.matches("overlay=").count(),
            1,
            "only the second clip is under 4.25s"
        );
        assert!(!graph.contains("amix"), "a still has no sound");
        let seek = plan.args.iter().position(|a| a == "-ss").unwrap();
        // Read backwards from just after a quarter second into the second clip.
        assert_eq!(plan.args[seek + 1], "0.000000");
        assert_eq!(plan.args[seek + 3], "0.283333", "up to a frame past 0.25s");
        assert!(graph.contains("reverse,trim=end_frame=1"));
        assert!(
            graph.contains("colorchannelmixer=aa=0.5000") && !graph.contains("fade="),
            "the fade is half done: {graph}"
        );
        assert!(plan.args.ends_with(&["/o.png".to_string()]));
        assert!(plan_still(&p, &Fake, 9 * SECOND, Path::new("/o.png")).is_err());
    }

    #[test]
    fn a_stretch_is_cut_from_the_movie_as_drawn() {
        let mut p = Project::default();
        let v = video(&mut p, "a", 4);
        p.append_clip(v);
        let plan = plan_range(
            &p,
            &Fake,
            Path::new("/tmp/part.mp4"),
            Quality::Draft,
            SECOND,
            3 * SECOND,
        )
        .unwrap();
        let graph = graph(&plan);
        assert!(graph.contains("trim=start=1.000000:end=3.000000,setpts=PTS-STARTPTS"));
        assert!(graph.contains("atrim=start=1.000000:end=3.000000"));
        assert_eq!(plan.duration, 2 * SECOND);
        assert!(plan_range(&p, &Fake, Path::new("/tmp/x.mp4"), Quality::Draft, 3 * SECOND, SECOND).is_err());
    }

    #[test]
    fn empty_timeline_is_an_error() {
        assert!(plan(&Project::default(), &Fake, Path::new("/o.mp4"), Quality::Draft).is_err());
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress("out_time_us=1500000"), Some(1_500_000));
        assert_eq!(parse_progress("frame=3"), None);
    }
}
