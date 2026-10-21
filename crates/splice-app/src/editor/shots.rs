//! Reference shots on story beats: pictures of what a shot should look like,
//! with a note each. They come from files dropped on a card, a pasted
//! screenshot, the viewer's current frame, or `splice ctl`; each is turned
//! into a PNG, kept in the project file by hash, and stays out of the media
//! panel.

use super::*;
use crate::ctl::fail;
use serde_json::{Value, json};
use splice_core::store::hash_file;
use splice_core::story::{BeatId, ShotId, Still};

/// Reference pictures are kept no wider than this.
const WIDEST: u32 = 1600;

/// Where a reference picture is read from.
pub fn still_path(still: &Still) -> PathBuf {
    dirs().cache.join("media").join(format!("{}.{}", still.hash, still.ext))
}

/// Turns an image (or a frame of a video) into a reference PNG kept in the
/// cache by its hash. Blocking.
pub(super) fn make_still(source: &Path) -> anyhow::Result<Still> {
    let probe = ffmpeg::probe(source)?;
    let dir = dirs().cache.join("media");
    std::fs::create_dir_all(&dir)?;
    let partial = dir.join(format!("reference-{}.png", std::process::id()));
    let mut command = ffmpeg::export_command()?;
    command.args(["-hide_banner", "-loglevel", "error", "-y"]);
    if probe.kind == MediaKind::Video {
        // A moment in, past any fade from black.
        let at = (probe.duration / 3).min(SECOND);
        command.args(["-ss", &format!("{:.3}", secs(at))]);
    }
    let output = command
        .arg("-i")
        .arg(source)
        .args(["-frames:v", "1", "-vf", &format!("scale='min({WIDEST},iw)':-2")])
        .arg(&partial)
        .output()?;
    if !output.status.success() || !partial.exists() {
        anyhow::bail!(
            "{} is not a picture Splice can read",
            source
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        );
    }
    let hash = hash_file(&partial)?;
    let made = ffmpeg::probe(&partial)?;
    let still = Still {
        hash,
        ext: "png".into(),
        width: made.width,
        height: made.height,
    };
    std::fs::rename(&partial, still_path(&still))?;
    Ok(still)
}

impl Editor {
    /// Adds pictures from files to a beat's shots, each named after its file.
    pub(super) fn add_references(
        &mut self,
        beat: BeatId,
        paths: Vec<PathBuf>,
        note: Option<String>,
        reply: Option<std::sync::mpsc::Sender<String>>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.working += 1;
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|p| {
                            let name = p
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            make_still(&p).map(|s| (s, name))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                let mut added = Vec::new();
                let mut errors = Vec::new();
                for result in made {
                    match result {
                        Ok((still, name)) => {
                            let path = still_path(&still);
                            editor.embed_still(&still, path);
                            let note = note.clone().unwrap_or(name);
                            if let Some(id) = editor.edit(cx, |p| p.add_shot(beat, &note, Some(still))) {
                                added.push(id);
                            }
                        }
                        Err(e) => errors.push(e.to_string()),
                    }
                }
                if let Some(last) = added.last() {
                    editor.shown_shots.insert(beat, *last);
                    editor.beat = Some(beat);
                }
                match errors.first() {
                    Some(e) => editor.say_error(e.clone()),
                    None => editor.say(format!(
                        "Added {} reference shot{}",
                        added.len(),
                        if added.len() == 1 { "" } else { "s" }
                    )),
                }
                if let Some(reply) = reply {
                    let answer = json!({
                        "ok": errors.is_empty(),
                        "shots": added.iter().map(|s| s.0).collect::<Vec<_>>(),
                        "errors": errors,
                    });
                    let _ = reply.send(answer.to_string());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keeps a reference picture in the project file.
    pub(super) fn embed_still(&mut self, still: &Still, path: PathBuf) {
        let store_path = self.doc.path().to_path_buf();
        let (hash, ext) = (still.hash.clone(), still.ext.clone());
        std::thread::spawn(move || {
            if let Ok(store) = splice_core::store::Store::open(&store_path)
                && !store.has_media(&hash).unwrap_or(false)
            {
                let _ = store.put_media(&hash, &ext, &path);
            }
        });
    }

    /// A reference picture, from the cache or unpacked from the project file.
    pub(super) fn still_file(&self, still: &Still) -> Option<PathBuf> {
        let path = still_path(still);
        if path.exists() {
            return Some(path);
        }
        self.doc.store.extract_media(&still.hash, &path).ok().map(|()| path)
    }

    /// Asks for pictures to add to a beat.
    pub(super) fn pick_references(&mut self, beat: BeatId, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add reference".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |editor, cx| editor.add_references(beat, paths, None, None, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// The viewer's picture at the playhead becomes a reference shot.
    pub(super) fn grab_reference(&mut self, beat: BeatId, cx: &mut Context<Self>) {
        let at = self.player.position;
        let path = dirs().cache.join(format!("grab-{}.png", std::process::id()));
        let plan = export::plan_still(&self.doc.project, &self.resolver(), at, &path);
        let plan = match plan {
            Ok(plan) => plan,
            Err(e) => return self.say_error(format!("Could not grab the frame: {e}")),
        };
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_spawn(async move {
                    let out = ffmpeg::export_command()?.args(&plan.args).output()?;
                    anyhow::ensure!(out.status.success(), "ffmpeg could not draw the frame");
                    Ok::<_, anyhow::Error>(path)
                })
                .await;
            this.update(cx, |editor, cx| match made {
                Ok(path) => {
                    let note = format!("Frame at {}", timecode(at, editor.doc.project.settings.fps));
                    editor.add_references(beat, vec![path], Some(note), None, cx);
                }
                Err(e) => editor.say_error(format!("Could not grab the frame: {e}")),
            })
            .ok();
        })
        .detach();
    }

    /// ⌘V on the board: a copied picture (a screenshot) becomes a reference
    /// shot on the chosen beat.
    pub(super) fn paste_reference(&mut self, cx: &mut Context<Self>) {
        let Some(beat) = self.beat.filter(|_| self.mode == story::Mode::Story) else {
            return;
        };
        let Some(item) = cx.read_from_clipboard() else { return };
        let image = item.entries().iter().find_map(|e| match e {
            gpui::ClipboardEntry::Image(image) => Some(image.clone()),
            _ => None,
        });
        if let Some(image) = image {
            let ext = match image.format {
                gpui::ImageFormat::Png => "png",
                gpui::ImageFormat::Jpeg => "jpg",
                gpui::ImageFormat::Gif => "gif",
                gpui::ImageFormat::Webp => "webp",
                gpui::ImageFormat::Bmp => "bmp",
                gpui::ImageFormat::Tiff => "tiff",
                _ => "png",
            };
            let path = dirs().cache.join(format!("pasted-{}.{ext}", std::process::id()));
            match std::fs::write(&path, &image.bytes) {
                Ok(()) => self.add_references(beat, vec![path], Some("Pasted".into()), None, cx),
                Err(e) => self.say_error(format!("Could not paste: {e}")),
            }
            return;
        }
        // Copied files from Finder arrive as paths.
        let paths: Vec<PathBuf> = item
            .text()
            .map(|t| t.lines().map(PathBuf::from).filter(|p| p.is_file()).collect())
            .unwrap_or_default();
        if paths.is_empty() {
            self.say("Copy a picture first (⌃⇧⌘4 takes a screenshot to the clipboard)");
        } else {
            self.add_references(beat, paths, None, None, cx);
        }
    }

    pub(super) fn remove_shot(&mut self, shot: ShotId, cx: &mut Context<Self>) {
        self.typing = None;
        self.edit(cx, |p| p.remove_shot(shot));
    }

    /// A shot as `splice ctl story read` shows it.
    pub(super) fn shot_json(&self, shot: &splice_core::story::Shot) -> Value {
        json!({
            "shot": shot.id.0,
            "note": shot.note,
            "image": shot.image.as_ref().and_then(|s| self.still_file(s)),
            "size": shot.image.as_ref().map(|s| [s.width, s.height]),
        })
    }

    /// `splice ctl story shot add|set|remove|move`.
    pub(super) fn ctl_shot(
        &mut self,
        request: &Value,
        reply: std::sync::mpsc::Sender<String>,
        cx: &mut Context<Self>,
    ) -> Option<Value> {
        let text = |key: &str| request.get(key).and_then(Value::as_str).map(str::to_string);
        let action = text("action").unwrap_or_default();
        let shot = request.get("shot").and_then(Value::as_u64).map(ShotId);
        let known = |this: &Self, id: ShotId| this.doc.project.shot_beat(id).is_some();
        match action.as_str() {
            "add" => {
                let Some(beat) = request.get("beat").and_then(Value::as_u64).map(BeatId) else {
                    return Some(fail("name the beat with --beat B"));
                };
                if self.doc.project.story.index(beat).is_none() {
                    return Some(fail(format!("no beat {}", beat.0)));
                }
                match text("image") {
                    Some(image) => {
                        let path = PathBuf::from(&image);
                        if !path.exists() {
                            return Some(fail(format!("{image} does not exist")));
                        }
                        self.add_references(beat, vec![path], text("note"), Some(reply), cx);
                        None
                    }
                    None => {
                        let note = text("note").unwrap_or_default();
                        let id = self.edit(cx, |p| p.add_shot(beat, &note, None));
                        Some(json!({ "ok": true, "shot": id.map(|s| s.0) }))
                    }
                }
            }
            "set" | "remove" | "move" => {
                let Some(id) = shot.filter(|id| known(self, *id)) else {
                    return Some(fail("name an existing shot with --shot S (`story read` lists them)"));
                };
                match action.as_str() {
                    "remove" => self.remove_shot(id, cx),
                    "move" => {
                        let Some(to) = request.get("index").and_then(Value::as_u64) else {
                            return Some(fail("say where with --index I (0 is first)"));
                        };
                        self.edit(cx, |p| p.move_shot(id, to as usize));
                    }
                    _ => {
                        let Some(note) = text("note") else {
                            return Some(fail("give the new note with --note TEXT"));
                        };
                        self.edit(cx, |p| p.set_shot(id, |s| s.note = note));
                    }
                }
                Some(json!({ "ok": true }))
            }
            _ => Some(fail("story shot add | set | move | remove")),
        }
    }
}
