//! Headless commands, for scripts and for checking a project without the UI.
//!
//! ```text
//! splice --new <project.splice> <media>...   make a project, media end to end
//! splice --export <project.splice> <out.mp4> [--draft]
//! splice --info <project.splice>             versions and media
//! splice --still <scene.json> <out.png> [--size WxH] [--at T]
//!                                             one frame of a scene, drawn here
//! splice --version
//! ```

use crate::ffmpeg::{self, file_name};
use crate::library;
use crate::session::dirs;
use splice_core::export::{self, Quality};
use splice_core::model::{Media, MediaId, Project};
use splice_core::store::{Store, VersionKind, hash_file};
use splice_core::time::{secs, timecode};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub fn run(args: &[String]) -> Option<i32> {
    let result = match args.first().map(String::as_str) {
        Some("--new") if args.len() >= 3 => new(Path::new(&args[1]), &args[2..]),
        Some("--export") if args.len() >= 3 => {
            let quality = if args.iter().any(|a| a == "--draft") {
                Quality::Draft
            } else {
                Quality::Standard
            };
            export_project(Path::new(&args[1]), Path::new(&args[2]), quality)
        }
        Some("--info") if args.len() >= 2 => info(Path::new(&args[1])),
        Some("--still") if args.len() >= 3 => still(Path::new(&args[1]), Path::new(&args[2]), &args[3..]),
        Some("--version") | Some("-V") => {
            println!("splice {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") => {
            println!(
                "splice [project.splice]\nsplice --new <project.splice> <media>...\nsplice --export <project.splice> <out.mp4> [--draft]\nsplice --info <project.splice>\nsplice --still <scene.json> <out.png> [--size WxH] [--at SECONDS]\nsplice --version\nsplice ctl --help"
            );
            Ok(())
        }
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("splice: {e}");
            1
        }
    })
}

/// Draws one frame of a scene file to a PNG: no window, no project.
fn still(scene: &Path, out: &Path, rest: &[String]) -> anyhow::Result<()> {
    let mut size = (1280u32, 720u32);
    let mut at = 0.0f64;
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))?;
        match flag.as_str() {
            "--size" => {
                let (w, h) = value
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .ok_or_else(|| anyhow::anyhow!("--size is WIDTHxHEIGHT"))?;
                size = (w, h);
            }
            "--at" => at = value.parse().map_err(|_| anyhow::anyhow!("--at is seconds"))?,
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    let source = std::fs::read_to_string(scene)?;
    crate::scenes::still_png(&source, size, at, out)?;
    println!("{}", out.display());
    Ok(())
}

fn new(path: &Path, media: &[String]) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!("{} already exists", path.display());
    }
    let store = Store::open(path)?;
    let mut project = Project::default();
    for file in media {
        let file = PathBuf::from(file);
        let probe = ffmpeg::probe(&file)?;
        let hash = hash_file(&file)?;
        let ext = file
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        store.put_media(&hash, &ext, &file)?;
        let id = project.add_media(Media {
            id: MediaId(0),
            name: file_name(&file),
            hash,
            ext,
            kind: probe.kind,
            duration: probe.duration,
            width: probe.width,
            height: probe.height,
            has_audio: probe.has_audio,
            source: std::fs::canonicalize(&file).unwrap_or(file.clone()),
            recipe: None,
            content: None,
            clicks: Vec::new(),
        });
        project.append_clip(id);
    }
    store.save_version(&project, VersionKind::Save, Some("created"))?;
    println!(
        "{}: {} clips, {}",
        path.display(),
        project.clips().count(),
        timecode(project.duration(), 30)
    );
    Ok(())
}

/// A readable path for each piece of media, unpacking from the project when needed.
fn resolve_all(store: &Store, project: &Project) -> anyhow::Result<std::collections::HashMap<String, PathBuf>> {
    let mut paths = std::collections::HashMap::new();
    for media in &project.media {
        let cached = library::cache_path(&dirs().cache, media);
        if !cached.exists() {
            if store.has_media(&media.hash)? {
                store.extract_media(&media.hash, &cached)?;
            } else if media.source.exists() {
                paths.insert(media.hash.clone(), media.source.clone());
                continue;
            } else {
                anyhow::bail!("{} is missing", media.name);
            }
        }
        paths.insert(media.hash.clone(), cached);
    }
    Ok(paths)
}

fn export_project(path: &Path, out: &Path, quality: Quality) -> anyhow::Result<()> {
    let store = Store::open(path)?;
    let project = store.latest()?.unwrap_or_default();
    let paths = resolve_all(&store, &project)?;
    let resolver = crate::titles::Resolver {
        media: |m: &Media| paths.get(&m.hash).cloned(),
        dir: crate::titles::drawn_dir(),
        canvas: (project.settings.width, project.settings.height),
    };
    let plan = export::plan(&project, &resolver, out, quality).map_err(|e| anyhow::anyhow!(e))?;
    let prepared = ffmpeg::prepare(&plan, || true);
    if prepared.is_err() {
        ffmpeg::tidy(&plan);
    }
    prepared?;
    let mut child = ffmpeg::export_command()?
        .args(&plan.args)
        .stdout(Stdio::piped())
        .spawn()?;
    for line in BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
    {
        if let Some(t) = export::parse_progress(&line) {
            print!("\rexporting {:>5.1}%", 100.0 * t as f64 / plan.duration as f64);
            let _ = std::io::stdout().flush();
        }
    }
    let status = child.wait();
    ffmpeg::tidy(&plan);
    let status = status?;
    println!();
    if !status.success() {
        anyhow::bail!("ffmpeg failed");
    }
    println!("{} ({:.1}s)", out.display(), secs(plan.duration));
    Ok(())
}

fn info(path: &Path) -> anyhow::Result<()> {
    let store = Store::open(path)?;
    let project = store.latest()?.unwrap_or_default();
    let s = project.settings;
    println!(
        "{} · {}×{} {} fps · {}",
        path.display(),
        s.width,
        s.height,
        s.fps,
        timecode(project.duration(), s.fps)
    );
    for media in &project.media {
        let embedded = if store.has_media(&media.hash)? {
            "embedded"
        } else {
            "not embedded"
        };
        println!(
            "  media {:?} {} {} {}",
            media.kind,
            media.name,
            timecode(media.duration, s.fps),
            embedded
        );
    }
    for (i, track) in project.tracks.iter().enumerate() {
        println!(
            "  track {} · {} clips{}{}",
            i + 1,
            track.clips.len(),
            if track.hidden { " hidden" } else { "" },
            if track.muted { " muted" } else { "" }
        );
    }
    for v in store.versions()? {
        println!("  version {} {:?} {}", v.id, v.kind, v.label.unwrap_or_default());
    }
    Ok(())
}
