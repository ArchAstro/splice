//! Export: the timeline as one ffmpeg invocation.
//!
//! The picture starts as a solid black canvas. Each visible clip, bottom
//! track first, is an input trimmed by input seeking, scaled to fit the
//! canvas, shifted to its timeline start and overlaid centered. Sound from
//! every audible clip is delayed to its start and mixed over silence.

use crate::model::{Media, MediaKind, Project};
use crate::time::{Time, secs};
use std::path::{Path, PathBuf};

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

/// Builds the ffmpeg arguments that render `project` to `output` (an `.mp4`).
/// `resolve` finds a readable file for each piece of media.
pub fn plan(
    project: &Project,
    resolve: impl Fn(&Media) -> Option<PathBuf>,
    output: &Path,
    quality: Quality,
) -> Result<Plan, String> {
    let duration = project.duration();
    if duration <= 0 {
        return Err("the timeline is empty".into());
    }
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
        &format!("color=c=black:s={w}x{h}:r={fps}:d={d}"),
    ]));
    args.extend(strings(&["-f", "lavfi", "-t", &d, "-i", "anullsrc=r=48000:cl=stereo"]));
    let mut next_input = 2usize;

    let mut filters: Vec<String> = Vec::new();
    let mut picture = "0:v".to_string();
    let mut sounds: Vec<String> = Vec::new();

    for (track_index, track) in project.tracks.iter().enumerate() {
        for clip in &track.clips {
            let media = project.media(clip.media).ok_or("a clip refers to missing media")?;
            let shows = !track.hidden && media.is_visual();
            let sounds_out = !track.muted && media.has_audio && clip.volume > 0.0;
            if !shows && !sounds_out {
                continue;
            }
            let path = resolve(media).ok_or_else(|| format!("{} is offline", media.name))?;
            let path = path.to_string_lossy().to_string();
            let start = fmt(clip.start);
            let length = fmt(clip.duration);
            match media.kind {
                MediaKind::Image => args.extend(strings(&[
                    "-loop",
                    "1",
                    "-framerate",
                    &fps.to_string(),
                    "-t",
                    &length,
                    "-i",
                    &path,
                ])),
                MediaKind::Video | MediaKind::Audio => {
                    args.extend(strings(&["-ss", &fmt(clip.source_in), "-t", &length, "-i", &path]))
                }
            }
            let i = next_input;
            next_input += 1;
            let tag = format!("t{track_index}c{}", clip.id.0);

            if shows {
                filters.push(format!(
                    "[{i}:v]fps={fps},scale={w}:{h}:force_original_aspect_ratio=decrease,\
                     format=yuva420p,setpts=PTS-STARTPTS+{start}/TB[v{tag}]"
                ));
                let end = fmt(clip.end());
                filters.push(format!(
                    "[{picture}][v{tag}]overlay=x=(W-w)/2:y=(H-h)/2:eof_action=pass:\
                     enable='between(t,{start},{end})'[p{tag}]"
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

    filters.push(format!("[{picture}]format=yuv420p[vout]"));
    filters.push(format!(
        "[1:a]{}amix=inputs={}:normalize=0:duration=first[aout]",
        sounds.join(""),
        sounds.len() + 1
    ));

    args.extend(strings(&["-filter_complex", &filters.join(";")]));
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

fn fmt(t: Time) -> String {
    format!("{:.6}", secs(t))
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Media, MediaId};
    use crate::time::SECOND;

    #[test]
    fn plans_layers_and_sound() {
        let mut p = Project::default();
        let video = p.add_media(Media {
            id: MediaId(0),
            name: "v".into(),
            hash: "a".into(),
            ext: "mp4".into(),
            kind: MediaKind::Video,
            duration: 4 * SECOND,
            width: 1920,
            height: 1080,
            has_audio: true,
            source: PathBuf::new(),
        });
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
        });
        p.append_clip(video);
        p.insert_clip(image, 0, SECOND);
        let plan = plan(
            &p,
            |m| Some(PathBuf::from(format!("/m/{}", m.hash))),
            Path::new("/o.mp4"),
            Quality::Draft,
        )
        .unwrap();
        let graph = &plan.args[plan.args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert_eq!(graph.matches("overlay=").count(), 2);
        assert!(graph.contains("amix=inputs=2"));
        assert!(plan.args.contains(&"/m/b".to_string()));
        assert_eq!(plan.duration, 6 * SECOND, "the image runs 1s to 6s");
    }

    #[test]
    fn empty_timeline_is_an_error() {
        assert!(plan(&Project::default(), |_| None, Path::new("/o.mp4"), Quality::Draft).is_err());
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress("out_time_us=1500000"), Some(1_500_000));
        assert_eq!(parse_progress("frame=3"), None);
    }
}
