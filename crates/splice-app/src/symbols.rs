//! The symbol library: pieces of movies kept for reuse in any project. Each
//! symbol is a small Splice project file in one folder, so editing one is
//! opening it in Splice. What it publishes is its latest *saved* version, so
//! work in progress never reaches the projects that use it. Each file keeps
//! the symbol's identity, so renaming it in Finder or here keeps its uses.
//! Everything here blocks; run it off the main thread.

use crate::ffmpeg;
use crate::library::cache_path;
use crate::session::dirs;
use crate::titles;
use anyhow::{Context, Result, bail};
use splice_core::export;
use splice_core::model::{Media, Project, Recipe, Settings};
use splice_core::store::{EXTENSION, Store, VersionKind};
use splice_core::symbol::{Symbol, version_of};
use splice_core::time::Time;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::SystemTime;

/// Renders of a symbol with this renderer name the same bytes.
pub const ENGINE: &str = "splice-symbol-2|prores4444";

/// The meta key a symbol's identity is kept under in its file.
const ID: &str = "symbol_id";

/// Where the library lives.
pub fn folder() -> PathBuf {
    let dir = dirs().data.join("Symbols");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn file(name: &str) -> PathBuf {
    folder().join(format!("{name}.{EXTENSION}"))
}

/// The poster for one version of a symbol: its content, cropped, on clear.
/// Posters live in the cache, named by version, so the library folder holds
/// only symbols and a changed symbol is never shown with an old picture.
pub fn poster(name: &str, version: &str) -> PathBuf {
    let dir = dirs().cache.join("symbols");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{name}-{version}.png"))
}

/// A fresh identity for a new symbol.
pub fn new_id() -> String {
    let seed = format!(
        "{:?}|{}|{:?}",
        SystemTime::now(),
        std::process::id(),
        std::thread::current().id()
    );
    blake3::hash(seed.as_bytes()).to_hex()[..20].to_string()
}

/// A symbol in the library, as listed (cheap: no reading inside the file).
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub modified: SystemTime,
    /// What reading inside it found, once read.
    pub info: Option<Info>,
}

/// What a symbol's file says about it.
#[derive(Clone, Debug)]
pub struct Info {
    pub id: String,
    /// The published (last saved) version.
    pub version: String,
    /// Every published version, oldest first, so "newer" can be told from
    /// "different".
    pub lineage: Vec<String>,
    pub duration: Time,
    pub settings: Settings,
}

impl Entry {
    /// Its poster, if drawn.
    pub fn poster(&self) -> Option<PathBuf> {
        let info = self.info.as_ref()?;
        Some(poster(&self.name, &info.version)).filter(|p| p.exists())
    }
}

/// The library, newest first.
pub fn list() -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(folder()) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().to_string();
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
            Some(Entry {
                name,
                modified,
                info: None,
            })
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.modified));
    out
}

/// Reads what is inside a library symbol: its identity (given one now if it
/// has none), published version and its history.
pub fn info(name: &str) -> Result<Info> {
    let store = Store::open(&file(name))?;
    let id = match store.meta(ID)? {
        Some(id) => id,
        None => {
            let id = new_id();
            store.set_meta(ID, &id)?;
            id
        }
    };
    let saved = store.saved()?;
    let latest = match saved.last() {
        Some(p) => p.clone(),
        None => store
            .latest()?
            .with_context(|| format!("the symbol {name:?} is empty"))?,
    };
    Ok(Info {
        id,
        version: version_of(&latest),
        lineage: saved.iter().map(version_of).collect(),
        duration: latest.duration(),
        settings: latest.settings,
    })
}

/// A name that is safe as a file name.
pub fn clean_name(name: &str) -> Result<String> {
    let name: String = name
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | ':' | '\\') { '-' } else { c })
        .collect();
    if name.is_empty() || name.starts_with('.') {
        bail!("give the symbol a name");
    }
    Ok(name)
}

/// Whether a symbol of that name is in the library, in any case.
pub fn taken(name: &str) -> bool {
    list().iter().any(|e| e.name.eq_ignore_ascii_case(name))
}

/// The symbol's published timeline and identity.
pub fn read(name: &str) -> Result<(String, Project)> {
    let path = file(name);
    if !path.exists() {
        bail!("there is no symbol called {name:?} in your symbols");
    }
    let info = info(name)?;
    let store = Store::open(&path)?;
    let project = match store.latest_saved()? {
        Some(p) => p,
        None => store
            .latest()?
            .with_context(|| format!("the symbol {name:?} has nothing saved"))?,
    };
    Ok((info.id, project))
}

/// Every piece of media `project` needs bytes for: its own, and that of
/// symbols inside it, at any depth, and what fills them.
fn all_media(project: &Project, out: &mut Vec<Media>) {
    for media in &project.media {
        out.push(media.clone());
        if let Some(Recipe::Symbol(symbol)) = &media.recipe {
            all_media(&symbol.filled(), out);
        }
    }
}

/// Copies the bytes `project` needs from `from` (a project file) into `to`,
/// skipping what `to` has. Media the cache holds is copied from there.
pub fn copy_media(project: &Project, from: &Path, to: &Store) -> Result<()> {
    let source = Store::open(from)?;
    let mut all = Vec::new();
    all_media(project, &mut all);
    for media in all {
        if to.has_media(&media.hash)? {
            continue;
        }
        let cached = cache_path(&dirs().cache, &media);
        if !cached.exists() {
            if !source.has_media(&media.hash)? {
                continue;
            }
            source.extract_media(&media.hash, &cached)?;
        }
        to.put_media(&media.hash, &media.ext, &cached)?;
    }
    Ok(())
}

/// Writes `project` to the library as a new symbol `name`, its media copied
/// from the project file `from`. `id` keeps an identity (a copy of a symbol
/// gets a new one). Never replaces a symbol. Returns its identity.
pub fn save(name: &str, project: &Project, from: &Path, id: Option<&str>) -> Result<String> {
    let path = file(name);
    if path.exists() {
        bail!("there is already a symbol called {name:?}");
    }
    let partial = path.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    let id = id.map_or_else(new_id, str::to_string);
    {
        let store = Store::open(&partial)?;
        copy_media(project, from, &store)?;
        store.set_meta(ID, &id)?;
        store.save_version(project, VersionKind::Save, Some("Made a symbol"))?;
    }
    std::fs::rename(&partial, &path)?;
    let _ = draw_poster(project, from, &poster(name, &version_of(project)));
    Ok(id)
}

/// Renames a library symbol. Its uses follow it, by identity.
pub fn rename(from: &str, to: &str) -> Result<()> {
    let to = clean_name(to)?;
    if !file(from).exists() {
        bail!("there is no symbol called {from:?}");
    }
    if to.eq_ignore_ascii_case(from) && to != from {
        // A change of case only: through a temporary name, for case-blind disks.
        let step = folder().join(format!(".rename-{}.{EXTENSION}", new_id()));
        std::fs::rename(file(from), &step)?;
        std::fs::rename(&step, file(&to))?;
        return Ok(());
    }
    if taken(&to) {
        bail!("there is already a symbol called {to:?}");
    }
    std::fs::rename(file(from), file(&to))?;
    Ok(())
}

/// Copies a library symbol under a new name, as a symbol of its own.
pub fn duplicate(from: &str, to: &str) -> Result<()> {
    let to = clean_name(to)?;
    if taken(&to) {
        bail!("there is already a symbol called {to:?}");
    }
    let (_, project) = read(from)?;
    save(&to, &project, &file(from), None)?;
    Ok(())
}

/// Moves a library symbol to the Trash. Projects that use it keep their copy.
pub fn delete(name: &str) -> Result<PathBuf> {
    let path = file(name);
    if !path.exists() {
        bail!("there is no symbol called {name:?}");
    }
    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .context("no home folder")?;
    let trash = home.join(".Trash");
    std::fs::create_dir_all(&trash)?;
    let mut dest = trash.join(format!("{name}.{EXTENSION}"));
    let mut n = 2;
    while dest.exists() {
        dest = trash.join(format!("{name} {n}.{EXTENSION}"));
        n += 1;
    }
    std::fs::rename(&path, &dest)?;
    Ok(dest)
}

/// Makes sure every file `project` needs is in the media cache: taken from
/// the project file `store`, or, for a symbol's use whose render was never
/// kept, rendered now (at `project`'s canvas).
pub fn unpack(project: &Project, store: &Path) -> Result<()> {
    let mut opened: Option<Store> = None;
    for media in &project.media {
        let cached = cache_path(&dirs().cache, media);
        if cached.exists() {
            continue;
        }
        if opened.is_none() {
            opened = Some(Store::open(store)?);
        }
        let file = opened.as_ref().expect("opened");
        if file.has_media(&media.hash)? {
            file.extract_media(&media.hash, &cached)?;
        } else if let Some(Recipe::Symbol(inner)) = &media.recipe {
            render(inner, project.settings, store, &cached, |_| {})?;
        }
    }
    Ok(())
}

fn resolver(settings: Settings) -> titles::Resolver<impl Fn(&Media) -> Option<PathBuf>> {
    titles::Resolver {
        media: |m: &Media| Some(cache_path(&dirs().cache, m)).filter(|p| p.exists()),
        dir: titles::drawn_dir(),
        canvas: (settings.width, settings.height),
    }
}

/// Where a picture draws anything: (x, y, w, h) in pixels, if anywhere.
fn opaque_box(pixmap: &tiny_skia::Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (pixmap.width(), pixmap.height());
    let data = pixmap.data();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if data[((y * w + x) * 4 + 3) as usize] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

/// A picture of the symbol for the shelf: the moment halfway through, cropped
/// to what it shows (with a margin), see-through around it.
pub fn draw_poster(project: &Project, store: &Path, out: &Path) -> Result<()> {
    unpack(project, store)?;
    let t = (project.duration() / 2).max(0);
    let full = out.with_extension("full.png");
    let plan = export::plan_still_layer(project, &resolver(project.settings), t, &full).map_err(anyhow::Error::msg)?;
    let output = ffmpeg::export_command()?.args(&plan.args).output()?;
    if !output.status.success() {
        bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or("ffmpeg failed")
        );
    }
    let pixmap = tiny_skia::Pixmap::load_png(&full)?;
    let _ = std::fs::remove_file(&full);
    let (w, h) = (pixmap.width(), pixmap.height());
    let (x, y, bw, bh) = opaque_box(&pixmap).unwrap_or((0, 0, w, h));
    // A margin, and no flatter than the shelf's cards so it fills them.
    let margin = (bw.max(bh) as f32 * 0.12) as u32;
    let (mut cw, mut ch) = (bw + 2 * margin, bh + 2 * margin);
    if (cw as f32) < ch as f32 * 1.5 {
        cw = (ch as f32 * 1.5) as u32;
    } else if (ch as f32) < cw as f32 / 1.9 {
        ch = (cw as f32 / 1.9) as u32;
    }
    let (cw, ch) = (cw.min(w).max(2), ch.min(h).max(2));
    let cx = (x + bw / 2).saturating_sub(cw / 2).min(w - cw);
    let cy = (y + bh / 2).saturating_sub(ch / 2).min(h - ch);
    let mut crop = tiny_skia::Pixmap::new(cw, ch).context("poster size")?;
    crop.draw_pixmap(
        -(cx as i32),
        -(cy as i32),
        pixmap.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
    crop.save_png(out)?;
    Ok(())
}

/// Where a render draws anything over its whole length, as fractions of its
/// frame (x, y, w, h), sampled a few times a second.
fn content_of(movie: &Path) -> Option<[f32; 4]> {
    let output = ffmpeg::export_command()
        .ok()?
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(movie)
        .args(["-vf", "fps=6,alphaextract,bbox=min_val=12", "-f", "null", "-"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    let number = |line: &str, key: &str| -> Option<i64> {
        let at = line.find(key)? + key.len();
        line[at..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    };
    for line in text.lines().filter(|l| l.contains("x1:")) {
        if let (Some(a), Some(b), Some(c), Some(d)) = (
            number(line, "x1:"),
            number(line, "y1:"),
            number(line, "x2:"),
            number(line, "y2:"),
        ) {
            x0 = x0.min(a);
            y0 = y0.min(b);
            x1 = x1.max(c + 1);
            y1 = y1.max(d + 1);
        }
    }
    let size = text.lines().find_map(|l| {
        let at = l.find("yuva444p")?;
        l[at..].split([',', ' ']).find_map(|part| {
            let (w, h) = part.split_once('x')?;
            Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        })
    })?;
    (x1 > x0 && y1 > y0).then(|| {
        [
            x0 as f32 / size.0,
            y0 as f32 / size.1,
            (x1 - x0) as f32 / size.0,
            (y1 - y0) as f32 / size.1,
        ]
    })
}

/// Renders one use of a symbol in a movie of canvas `host` to `out` (a
/// see-through `.mov`), taking the media it needs from the project file
/// `store`. `progress` hears 0–1. Returns where it draws anything.
pub fn render(
    symbol: &Symbol,
    host: Settings,
    store: &Path,
    out: &Path,
    mut progress: impl FnMut(f32),
) -> Result<Option<[f32; 4]>> {
    // Inner pieces (other symbols in it) render at its own canvas.
    unpack(&symbol.filled(), store)?;
    let project = symbol.rendered(host);
    let partial = out.with_extension("partial.mov");
    let plan = export::plan_layer(&project, &resolver(project.settings), &partial).map_err(anyhow::Error::msg)?;
    let prepared = ffmpeg::prepare(&plan, || true);
    if prepared.is_err() {
        ffmpeg::tidy(&plan);
    }
    prepared?;
    let mut child = ffmpeg::export_command()?
        .args(&plan.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting ffmpeg")?;
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let duration: Time = plan.duration.max(1);
    for line in BufReader::new(child.stdout.take().expect("piped"))
        .lines()
        .map_while(Result::ok)
    {
        if let Some(t) = export::parse_progress(&line) {
            progress((t as f32 / duration as f32).clamp(0.0, 1.0));
        }
    }
    let status = child.wait();
    ffmpeg::tidy(&plan);
    let status = status?;
    let errors = errors.join().unwrap_or_default();
    if !status.success() {
        bail!("{}", errors.lines().last().unwrap_or("ffmpeg failed"));
    }
    let content = content_of(&partial);
    std::fs::rename(&partial, out)?;
    Ok(content)
}

/// Opens a symbol in its own Splice window, to change it.
pub fn open_window(name: &str) -> Result<()> {
    let path = file(name);
    if !path.exists() {
        bail!("there is no symbol called {name:?} in your symbols");
    }
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use splice_core::look::MotionKind;
    use splice_core::model::{Generator, Title};
    use splice_core::time::SECOND;

    #[test]
    fn names_are_safe_file_names() {
        assert_eq!(clean_name(" Lower third ").unwrap(), "Lower third");
        assert_eq!(clean_name("a/b:c").unwrap(), "a-b-c");
        assert!(clean_name("  ").is_err());
        assert!(clean_name(".hidden").is_err());
    }

    /// A title that pops in, rendered as a use: it must show at full size,
    /// not frozen at the size it starts from.
    #[test]
    fn a_popping_title_renders_whole() {
        if !ffmpeg::available() {
            return;
        }
        let mut project = Project::default();
        project.settings = Settings {
            width: 640,
            height: 360,
            fps: 30,
        };
        let id = project.insert_generated(
            Generator::Text(Title {
                text: "Hello".into(),
                ..Title::default()
            }),
            0,
            0,
            2 * SECOND,
        );
        project.set_look(id, |l| l.enter.kind = MotionKind::Pop);
        let symbol = Symbol::new("t", "Pop", project.clone());
        let dir = std::env::temp_dir().join(format!("splice-pop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("pop.mov");
        let store = dir.join("empty.splice");
        Store::open(&store).unwrap();
        let content = render(&symbol, project.settings, &store, &out, |_| {}).unwrap();
        let [_, _, w, h] = content.expect("it draws something");
        assert!(w * 640.0 > 50.0 && h * 360.0 > 20.0, "{w} x {h}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
