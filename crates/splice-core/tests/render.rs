//! Export plans run through ffmpeg: transitions, speed, reverse, held
//! frames, keyframes and colour. Each renders a tiny movie and measures it.
//! Skipped when ffmpeg is not installed.

use splice_core::export::{self, Quality, Rendered, Resolve};
use splice_core::look::{Ease, Preset, Prop};
use splice_core::model::{Clip, Generator, Media, MediaId, MediaKind, Project};
use splice_core::time::{SECOND, Time, secs};
use splice_core::transition::{Kind, Transition};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tool(name: &str) -> Option<PathBuf> {
    ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.exists())
}

struct Files(PathBuf);

impl Resolve for Files {
    fn media(&self, media: &Media) -> Option<PathBuf> {
        Some(self.0.join(format!("{}.{}", media.hash, media.ext)))
    }
    fn generated(&self, _: &Clip, _: &Generator) -> Option<Rendered> {
        None
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("splice-render-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 6 s clip whose brightness climbs with time (about 40 levels a second),
/// with a tone, so where in it a frame is from can be measured.
fn ramp(dir: &Path, hash: &str, colour: bool) -> Option<Media> {
    ramp_with_sound(dir, hash, colour, 6.0)
}

/// Like `ramp`, its sound running `sound` seconds (past the picture's 6 when longer).
fn ramp_with_sound(dir: &Path, hash: &str, colour: bool, sound: f64) -> Option<Media> {
    let ffmpeg = tool("ffmpeg")?;
    let path = dir.join(format!("{hash}.mp4"));
    let picture = if colour {
        "color=c=0x3060c0:s=160x90:r=30:d=6,format=yuv420p".to_string()
    } else {
        "color=c=black:s=160x90:r=30:d=6,format=gray,geq=lum='16+T*36',format=yuv420p".to_string()
    };
    let ok = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &picture,
        ])
        .args(["-f", "lavfi", "-i", "sine=f=440:d=6:sample_rate=48000"])
        .args([
            "-c:v",
            "libx264",
            "-g",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&path)
        .status()
        .ok()?
        .success();
    ok.then(|| Media {
        id: MediaId(0),
        name: hash.into(),
        hash: hash.into(),
        ext: "mp4".into(),
        kind: MediaKind::Video,
        duration: (sound.max(6.0) * SECOND as f64) as Time,
        width: 160,
        height: 90,
        has_audio: true,
        source: path,
        recipe: None,
        content: None,
    })
}

fn project() -> Project {
    let mut p = Project::default();
    p.settings.width = 160;
    p.settings.height = 90;
    p
}

fn render(p: &Project, dir: &Path, name: &str) -> PathBuf {
    let out = dir.join(format!("{name}.mp4"));
    let plan = export::plan(p, &Files(dir.to_path_buf()), &out, Quality::Draft).unwrap();
    if std::env::var("SHOW_PLAN").is_ok() {
        eprintln!("{name}: {}", plan.args.join(" "));
    }
    run_plan(&plan, name);
    out
}

/// Runs a plan's preparation, then the plan, then tidies up.
fn run_plan(plan: &export::Plan, name: &str) {
    if let Some(dir) = &plan.scratch {
        std::fs::create_dir_all(dir).unwrap();
    }
    for (path, text) in &plan.files {
        std::fs::write(path, text).unwrap();
    }
    for command in &plan.prepare {
        run(command, &format!("{name} (preparing)"));
    }
    run(&plan.args, name);
    if let Some(dir) = &plan.scratch {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn run(args: &[String], name: &str) {
    let output = Command::new(tool("ffmpeg").unwrap()).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{name} failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        args.join(" ")
    );
}

fn duration(path: &Path) -> f64 {
    let out = Command::new(tool("ffprobe").unwrap())
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

/// Average brightness (0–255) and saturation of the frame at `t` seconds.
fn measure(path: &Path, t: f64) -> (f64, f64) {
    let out = Command::new(tool("ffmpeg").unwrap())
        .args(["-hide_banner", "-ss", &format!("{t:.3}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", "signalstats,metadata=print", "-f", "null", "-"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    let tag = |name: &str| {
        text.lines()
            .find_map(|l| {
                l.split_once(&format!("lavfi.signalstats.{name}="))
                    .map(|(_, v)| v.trim().parse::<f64>().unwrap())
            })
            .unwrap_or_else(|| panic!("no {name} at {t}s of {path:?}: {text}"))
    };
    (tag("YAVG"), tag("SATAVG"))
}

/// Like `measure`, of one part of the frame (an ffmpeg crop).
fn measure_crop(path: &Path, t: f64, crop: &str) -> (f64, f64) {
    let out = Command::new(tool("ffmpeg").unwrap())
        .args(["-hide_banner", "-ss", &format!("{t:.3}"), "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("{crop},signalstats,metadata=print"),
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    let tag = |name: &str| {
        text.lines()
            .find_map(|l| {
                l.split_once(&format!("lavfi.signalstats.{name}="))
                    .map(|(_, v)| v.trim().parse::<f64>().unwrap())
            })
            .unwrap_or_else(|| panic!("no {name} at {t}s of {path:?}: {text}"))
    };
    (tag("YAVG"), tag("SATAVG"))
}

/// Loudness (RMS, dB) of 40 ms of sound from `t` seconds.
fn loudness(path: &Path, t: f64) -> f64 {
    let out = Command::new(tool("ffmpeg").unwrap())
        .args(["-hide_banner", "-ss", &format!("{t:.3}"), "-t", "0.04", "-i"])
        .arg(path)
        .args([
            "-vn",
            "-af",
            "astats=metadata=1:reset=0,ametadata=print:key=lavfi.astats.Overall.RMS_level",
            "-f",
            "null",
            "-",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .rev()
        .filter_map(|l| {
            l.split_once("RMS_level=")
                .map(|(_, v)| v.trim().parse::<f64>().unwrap_or(-200.0))
        })
        .next()
        .unwrap_or(-200.0)
}

fn brightness(path: &Path, t: f64) -> f64 {
    measure(path, t).0
}

/// What the source ramp measures `s` seconds in.
fn level(s: f64) -> f64 {
    30.0 + 30.6 * s
}

fn near(a: f64, b: f64, by: f64) -> bool {
    (a - b).abs() <= by
}

macro_rules! needs_ffmpeg {
    ($dir:expr, $hash:expr, $colour:expr) => {
        match ramp(&$dir, $hash, $colour) {
            Some(media) => media,
            None => {
                eprintln!("ffmpeg not found; skipped");
                return;
            }
        }
    };
}

#[test]
fn speed_reverse_and_held_frames_play_the_right_moments() {
    let dir = scratch("speed");
    let media = needs_ffmpeg!(dir, "ramp", false);
    let mut p = project();
    let m = p.add_media(media);

    // Twice as fast: 6 s of source in 3 s, one second showing source second two.
    let fast = p.append_clip(m).unwrap();
    p.set_speed(&[fast], 2.0);
    assert_eq!(p.clip(fast).unwrap().duration, 3 * SECOND);
    let out = render(&p, &dir, "fast");
    assert!(near(duration(&out), 3.0, 0.1), "{}", duration(&out));
    assert!(
        near(brightness(&out, 1.0), level(2.0), 8.0),
        "{}",
        brightness(&out, 1.0)
    );

    // Backwards: bright first, dark last, every piece in order.
    p.set_speed(&[fast], 1.0);
    p.set_reverse(&[fast], true);
    let out = render(&p, &dir, "reverse");
    assert!(near(duration(&out), 6.0, 0.1));
    assert!(
        near(brightness(&out, 0.5), level(5.5), 8.0),
        "{}",
        brightness(&out, 0.5)
    );
    assert!(
        near(brightness(&out, 3.0), level(3.0), 8.0),
        "{}",
        brightness(&out, 3.0)
    );
    assert!(
        near(brightness(&out, 5.5), level(0.5), 8.0),
        "{}",
        brightness(&out, 5.5)
    );

    // A held frame: two seconds of source second one, then the rest moves along.
    p.set_reverse(&[fast], false);
    let held = p.freeze_frame(fast, SECOND, 2 * SECOND).unwrap();
    assert!(p.clip(held).unwrap().frozen);
    let out = render(&p, &dir, "held");
    assert!(near(duration(&out), 8.0, 0.1), "{}", duration(&out));
    for t in [1.2, 2.0, 2.8] {
        assert!(
            near(brightness(&out, t), level(1.0), 8.0),
            "{t}: {}",
            brightness(&out, t)
        );
    }
    assert!(
        near(brightness(&out, 4.0), level(2.0), 8.0),
        "{}",
        brightness(&out, 4.0)
    );
}

#[test]
fn transitions_render_and_stills_inside_them_match() {
    let dir = scratch("transitions");
    let dark = needs_ffmpeg!(dir, "ramp", false);
    let blue = needs_ffmpeg!(dir, "blue", true);
    let mut p = project();
    let a = p.add_media(dark);
    let b = p.add_media(blue);
    let first = p.append_clip(a).unwrap();
    p.trim_end(first, 3 * SECOND);
    let second = p.append_clip(b).unwrap();
    for kind in Kind::ALL {
        p.set_transition(&[second], Some(Transition::new(kind)));
        let out = render(&p, &dir, kind.name());
        assert!(near(duration(&out), 9.0, 0.1), "{}: {}", kind.name(), duration(&out));
        // Long after the cut, only the blue clip.
        let (_, sat) = measure(&out, 5.0);
        assert!(sat > 30.0, "{}: {sat}", kind.name());
    }
    // Halfway through a push, the old picture fills the left half and the new the right.
    p.set_transition(&[second], Some(Transition::new(Kind::PushLeft)));
    let out = render(&p, &dir, "push-check");
    let (_, left) = measure_crop(&out, 3.0, "crop=60:90:0:0");
    let (_, right) = measure_crop(&out, 3.0, "crop=60:90:100:0");
    assert!(left < 5.0 && right > 30.0, "left {left}, right {right}");
    // A dip is black right at the cut, and quiet.
    p.set_transition(&[second], Some(Transition::new(Kind::DipBlack)));
    let out = render(&p, &dir, "dip-check");
    assert!(brightness(&out, 3.0) < 25.0, "{}", brightness(&out, 3.0));
    assert!(loudness(&out, 2.98) < -35.0, "{}", loudness(&out, 2.98));
    // A dissolve halfway through mixes the two, and a still there says the same.
    p.set_transition(&[second], Some(Transition::new(Kind::Dissolve)));
    let out = render(&p, &dir, "dissolve-check");
    let (_, movie_sat) = measure(&out, 3.0);
    // The sound crossfades: as loud at the cut as either side of it.
    let (at_cut, before) = (loudness(&out, 2.98), loudness(&out, 1.5));
    assert!(
        (at_cut - before).abs() < 4.0,
        "at the cut {at_cut} dB, before {before} dB"
    );
    let still = dir.join("still.png");
    let plan = export::plan_still(&p, &Files(dir.clone()), 3 * SECOND, &still).unwrap();
    run_plan(&plan, "still");
    let (_, still_sat) = measure(&still, 0.0);
    assert!(movie_sat > 5.0 && movie_sat < 40.0, "half blue: {movie_sat}");
    assert!(near(movie_sat, still_sat, 6.0), "movie {movie_sat}, still {still_sat}");
    // A stretch cut across the transition plays it too.
    let part = dir.join("part.mp4");
    let plan = export::plan_range(&p, &Files(dir.clone()), &part, Quality::Draft, 2 * SECOND, 4 * SECOND).unwrap();
    run_plan(&plan, "range");
    let (_, part_sat) = measure(&part, 1.0);
    assert!(near(part_sat, movie_sat, 6.0), "range {part_sat}, movie {movie_sat}");
}

#[test]
fn keyframes_and_colour_reach_the_file() {
    let dir = scratch("keys");
    let media = needs_ffmpeg!(dir, "blue", true);
    let mut p = project();
    let m = p.add_media(media);
    let clip = p.append_clip(m).unwrap();
    // Fades up from nothing over two seconds, then holds.
    p.set_look(clip, |look| {
        look.keys.set(Prop::Opacity, 0, 0.0, Some(Ease::Linear));
        look.keys.set(Prop::Opacity, 2 * SECOND, 1.0, None);
        look.keys.set(Prop::Scale, 0, 1.0, None);
        look.keys.set(Prop::Scale, 4 * SECOND, 0.5, None);
    });
    let out = render(&p, &dir, "keys");
    // In the middle, which the picture always covers.
    let middle = "crop=20:20:70:35";
    let (early, _) = measure_crop(&out, 0.2, middle);
    let (half, _) = measure_crop(&out, 1.0, middle);
    let (full, _) = measure_crop(&out, 3.0, middle);
    assert!(early < half && half < full, "{early} {half} {full}");
    // Half size by the end, and over to the right: the corners go black.
    let (corner, _) = measure_crop(&out, 3.9, "crop=20:20:0:0");
    assert!(corner < 20.0, "the shrunk picture leaves the corner black: {corner}");
    let look = &p.clip(clip).unwrap().look;
    assert!((look.value(Prop::Opacity, SECOND) - 0.5).abs() < 1e-6);
    // Black and white leaves no colour.
    p.set_look(clip, |look| {
        look.keys = Default::default();
        look.grade.preset = Preset::Mono;
    });
    let out = render(&p, &dir, "mono");
    let (_, sat) = measure(&out, 1.0);
    assert!(sat < 2.0, "{sat}");
    let _: Time = 0;
    let _ = secs(0);
}

#[test]
fn files_whose_sound_runs_past_their_picture() {
    let dir = scratch("long-sound");
    let Some(media) = ramp_with_sound(&dir, "long", false, 6.5) else {
        eprintln!("ffmpeg not found; skipped");
        return;
    };
    let mut p = project();
    let m = p.add_media(media);
    let clip = p.append_clip(m).unwrap();
    assert_eq!(p.clip(clip).unwrap().duration, 6_500_000, "the container's length");
    // Backwards, the picture stays in step with the source it shows.
    p.set_reverse(&[clip], true);
    let out = render(&p, &dir, "reverse");
    assert!(
        near(brightness(&out, 3.0), level(3.5), 8.0),
        "{}",
        brightness(&out, 3.0)
    );
    // A frame held past the picture's end is its last frame, not nothing.
    p.set_reverse(&[clip], false);
    let held = p.freeze_frame(clip, 6_400_000, SECOND).unwrap();
    let at = p.clip(held).unwrap().start;
    let out = render(&p, &dir, "held");
    let t = secs(at) + 0.5;
    assert!(
        near(brightness(&out, t), level(5.95), 8.0),
        "{t}: {}",
        brightness(&out, t)
    );
}

#[test]
fn transitions_never_flash_more_than_their_clips() {
    let dir = scratch("cover");
    let blue = needs_ffmpeg!(dir, "blue", true);
    let dark = needs_ffmpeg!(dir, "ramp", false);
    let mut p = project();
    let b = p.add_media(blue);
    let d = p.add_media(dark);
    let first = p.append_clip(b).unwrap();
    // A small picture coming in: the blue one must fade, not vanish at the end.
    let second = p.append_clip(d).unwrap();
    p.set_look(second, |l| l.scale = 0.5);
    p.set_transition(&[second], Some(Transition::new(Kind::Dissolve)));
    let out = render(&p, &dir, "dissolve-small");
    let corner = "crop=20:20:0:0";
    let (_, before) = measure_crop(&out, 6.1, corner);
    let (_, after) = measure_crop(&out, 6.35, corner);
    assert!(
        before > 5.0 && before < 60.0 && after < 15.0,
        "fades out: {before} then {after}"
    );
    // A dip into a picture in picture over the blue one dims only the picture.
    p.delete(&[second]);
    p.trim_end(first, 6 * SECOND);
    let pip = p.insert_clip(d, 1, 2 * SECOND).unwrap();
    p.set_look(pip, |l| l.scale = 0.3);
    p.set_transition(&[pip], Some(Transition::new(Kind::DipBlack)));
    let out = render(&p, &dir, "dip-pip");
    let (_, sat) = measure_crop(&out, 2.05, corner);
    assert!(sat > 30.0, "the blue around it stays blue: {sat}");
    let _ = first;
}
