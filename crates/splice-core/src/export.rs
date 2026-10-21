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
use crate::time::{Time, secs};
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
    let mut moment = project.clone();
    for track in &mut moment.tracks {
        track.muted = true;
        track.clips.retain(|c| c.contains(t));
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

    let mut filters: Vec<String> = Vec::new();
    let mut picture = "0:v".to_string();
    let mut sounds: Vec<String> = Vec::new();

    for (track_index, track) in project.tracks.iter().enumerate() {
        for clip in &track.clips {
            let source = project.source(clip).ok_or("a clip refers to missing media")?;
            let shows = !track.hidden && source.is_visual();
            let has_audio = matches!(source, Source::Media(m) if m.has_audio);
            let sounds_out = !track.muted && has_audio && clip.volume > 0.0;
            if !shows && !sounds_out {
                continue;
            }
            // For a still, each clip contributes one frame from `offset` into it.
            let offset = still_at.map_or(0, |t| t - clip.start);
            let length = if still_at.is_some() { frame } else { clip.duration };
            let placed_at = if still_at.is_some() { 0 } else { clip.start };
            let (start, len) = (fmt(placed_at), fmt(length));

            let mut rendered = None;
            match source {
                Source::Generated(generator) => {
                    let picture = resolve
                        .generated(clip, generator)
                        .ok_or_else(|| format!("{} could not be drawn", generator.name()))?;
                    let path = picture.path.to_string_lossy().to_string();
                    rendered = Some((picture.width, picture.height));
                    args.extend(strings(&[
                        "-loop",
                        "1",
                        "-framerate",
                        &fps.to_string(),
                        "-t",
                        &len,
                        "-i",
                        &path,
                    ]));
                }
                Source::Media(media) => {
                    let path = resolve
                        .media(media)
                        .ok_or_else(|| format!("{} is offline", media.name))?;
                    let path = path.to_string_lossy().to_string();
                    if media.kind == MediaKind::Image {
                        args.extend(strings(&[
                            "-loop",
                            "1",
                            "-framerate",
                            &fps.to_string(),
                            "-t",
                            &len,
                            "-i",
                            &path,
                        ]));
                    } else {
                        args.extend(strings(&[
                            "-ss",
                            &fmt(clip.source_in + offset),
                            "-t",
                            &len,
                            "-i",
                            &path,
                        ]));
                    }
                }
            }
            let i = next_input;
            next_input += 1;
            let tag = format!("t{track_index}c{}", clip.id.0);

            if shows {
                let look = &clip.look;
                let natural = natural_size(source, rendered, w, h);
                // Only media is cropped; titles and colours are drawn to size.
                let ((nw, nh), (cx, cy)) = match source {
                    Source::Media(_) => look.cropped(natural),
                    Source::Generated(_) => (natural, (0.0, 0.0)),
                };
                let pose = look.pose_expr(clip.duration);
                // Local time inside the clip, in each filter's own clock.
                let local = if offset > 0 {
                    format!("(t+{})", fmt(offset))
                } else {
                    "t".into()
                };
                let mut chain = vec![format!("fps={fps}")];
                if matches!(source, Source::Media(_))
                    && let Some(pixels) = look.pixel_filters()
                {
                    chain.push(pixels);
                }
                chain.push("format=rgba".into());
                // A still knows its exact opacity; a movie fades over time.
                let opacity = match still_at {
                    Some(_) => look.pose(clip.duration, offset).opacity as f32,
                    None => look.opacity,
                };
                if opacity < 1.0 {
                    chain.push(format!("colorchannelmixer=aa={:.4}", opacity.max(0.0)));
                }
                match pose.scale.constant() {
                    Some(k) => chain.push(format!("scale={}:{}", even((nw * k).round()), even((nh * k).round()))),
                    None => {
                        let k = pose.scale.ffmpeg(&local);
                        chain.push(format!(
                            "scale=w='max(2,trunc({nw:.3}*{k}/2)*2)':h='max(2,trunc({nh:.3}*{k}/2)*2)':eval=frame"
                        ));
                    }
                }
                for (motion, inward) in [(look.enter, true), (look.exit, false)] {
                    if motion.kind == MotionKind::None || still_at.is_some() {
                        continue;
                    }
                    let fd = secs(motion.duration.min(clip.duration));
                    let st = if inward { 0.0 } else { secs(clip.duration) - fd };
                    chain.push(format!(
                        "fade=t={}:st={st:.6}:d={fd:.6}:alpha=1",
                        if inward { "in" } else { "out" }
                    ));
                }
                // Over a see-through canvas the overlay negotiates its format
                // from its inputs: a picture that changes size frame to frame
                // only stays right as RGBA (as yuva420p it froze at its first size).
                chain.push(format!(
                    "format={},setpts=PTS-STARTPTS+{start}/TB",
                    if layer { "rgba" } else { "yuva420p" }
                ));
                filters.push(format!("[{i}:v]{}[v{tag}]", chain.join(",")));

                let end = fmt(placed_at + length);
                let since = if placed_at > 0 {
                    format!("(t-{start})")
                } else {
                    "t".to_string()
                };
                let since = if offset > 0 {
                    format!("({since}+{})", fmt(offset))
                } else {
                    since
                };
                let animated =
                    pose.x.constant().is_none() || pose.y.constant().is_none() || pose.scale.constant().is_none();
                // A cropped picture's remains stay where they were, scaled with it.
                let shift = |by: f64| {
                    if by.abs() < 1e-6 {
                        String::new()
                    } else {
                        format!("+{by:.3}*{}", pose.scale.ffmpeg(&since))
                    }
                };
                let x = format!("(W-w)/2+{}*W{}", pose.x.ffmpeg(&since), shift(cx));
                let y = format!("(H-h)/2+{}*H{}", pose.y.ffmpeg(&since), shift(cy));
                filters.push(format!(
                    "[{picture}][v{tag}]overlay=x='{x}':y='{y}':{}{}eof_action=pass:\
                     enable='between(t,{start},{end})'[p{tag}]",
                    if animated { "eval=frame:" } else { "" },
                    // Over a see-through canvas, keep what is see-through.
                    if layer { "format=auto:" } else { "" }
                ));
                picture = format!("p{tag}");
            }
            if sounds_out {
                let delay = clip.start / 1000;
                filters.push(format!(
                    "[{i}:a]aresample=48000,asetpts=PTS-STARTPTS,volume={:.3},adelay={delay}:all=1[a{tag}]",
                    clip.volume
                ));
                sounds.push(format!("[a{tag}]"));
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
        return Ok(Plan { args, duration });
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
        return Ok(Plan { args, duration });
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
    Ok(Plan { args, duration })
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
    use crate::look::{Key, Mask};
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
        assert_eq!(plan.args[seek + 1], "0.250000", "a quarter second into the second clip");
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
