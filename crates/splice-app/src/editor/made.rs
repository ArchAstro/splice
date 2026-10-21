//! Media Splice makes: scores and scenes. Each is a recipe plus a render of
//! it. After any change, `reconcile_made` points the media at the render of
//! its current recipe, finding it in the cache or the project file or
//! rendering it, while the previous render keeps playing.

use super::*;
use crate::{music, scenes, symbols};
use splice_core::model::{Recipe, Scene};
use splice_core::score::Score;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

/// A render under way.
pub struct Render {
    pub media: MediaId,
    /// 0–1, written by the render thread.
    pub progress: Arc<AtomicU32>,
    /// `splice ctl … --wait` callers, answered when it ends.
    pub waiters: Vec<std::sync::mpsc::Sender<String>>,
}

impl Render {
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }
}

/// What the editor knows about made media beyond the project.
#[derive(Default)]
pub struct Made {
    /// By render key.
    pub renders: HashMap<String, Render>,
    /// The last render that failed, per media, with why.
    pub errors: HashMap<MediaId, String>,
    /// When each scene's code file last changed, as last read.
    pub seen: HashMap<MediaId, SystemTime>,
}

/// Where a render of a symbol keeps the part of its frame it draws on.
fn content_file(render: &Path) -> PathBuf {
    render.with_extension("content.json")
}

/// Engine names, so a new synth or painter renders afresh.
pub fn engine(recipe: &Recipe) -> &'static str {
    match recipe {
        Recipe::Score(_) => music::ENGINE,
        Recipe::Scene(_) => scenes::ENGINE,
        Recipe::Symbol(_) => symbols::ENGINE,
    }
}

pub fn ext(recipe: &Recipe) -> &'static str {
    match recipe {
        Recipe::Score(_) => "wav",
        Recipe::Scene(_) | Recipe::Symbol(_) => "mov",
    }
}

impl Editor {
    /// Media for a recipe, not yet rendered, named `name`.
    pub(super) fn made_media(&self, name: String, recipe: Recipe) -> Media {
        let settings = self.doc.project.settings;
        let (kind, width, height, audio) = match &recipe {
            Recipe::Score(_) => (MediaKind::Audio, 0, 0, true),
            Recipe::Scene(_) => (MediaKind::Video, settings.width, settings.height, false),
            // A symbol keeps its own canvas, and fits into this one like a video.
            Recipe::Symbol(symbol) => {
                let own = symbol.project.settings;
                (MediaKind::Video, own.width, own.height, symbol.has_audio())
            }
        };
        Media {
            id: MediaId(0),
            name,
            hash: recipe.key(&settings, engine(&recipe)),
            ext: ext(&recipe).into(),
            kind,
            duration: recipe.duration().unwrap_or(SECOND),
            width,
            height,
            has_audio: audio,
            source: PathBuf::new(),
            recipe: Some(recipe),
            content: None,
        }
    }

    /// Brings every made media's render in line with its recipe.
    pub(super) fn reconcile_made(&mut self, cx: &mut Context<Self>) {
        let settings = self.doc.project.settings;
        let cache = dirs().cache.clone();
        for media in self.doc.project.media.clone() {
            let Some(recipe) = media.recipe.clone() else { continue };
            let key = recipe.key(&settings, engine(&recipe));
            let duration = match recipe.duration() {
                Ok(d) => d,
                Err(e) => {
                    self.made.errors.insert(media.id, e.to_string());
                    continue;
                }
            };
            if let Recipe::Scene(scene) = &recipe {
                self.sync_scene_file(media.id, scene);
            }
            // A use of a symbol is named by what it says, and sounds when
            // anything in it (a fill too) does.
            if let Recipe::Symbol(symbol) = &recipe {
                let (title, audio) = (symbol.title(), symbol.has_audio());
                if media.name != title || media.has_audio != audio {
                    for m in self.doc.project.media.iter_mut().filter(|m| m.id == media.id) {
                        m.name = title.clone();
                        m.has_audio = audio;
                    }
                    self.doc.touch();
                }
            }
            let mut media = media;
            if media.hash != key || media.duration != duration {
                // Keep showing the old render until the new one is ready.
                let old = media.hash.clone();
                if old != key && self.library.get(&key).is_none_or(|a| a.path.is_none()) {
                    self.library.borrow_render(&old, &key);
                }
                self.doc.project.set_render(media.id, &key, duration);
                let size = match &recipe {
                    Recipe::Scene(_) => Some((settings.width, settings.height)),
                    Recipe::Symbol(symbol) => {
                        let at = symbol.render_settings(settings);
                        Some((at.width, at.height))
                    }
                    Recipe::Score(_) => None,
                };
                if let Some((width, height)) = size {
                    for m in self.doc.project.media.iter_mut().filter(|m| m.id == media.id) {
                        m.width = width;
                        m.height = height;
                        // Where it draws comes with the new render.
                        m.content = None;
                    }
                }
                self.doc.touch();
                media = self.doc.project.media(media.id).cloned().expect("still here");
            }
            let ready = self.library.get(&key).is_some_and(|a| {
                a.path
                    .as_ref()
                    .is_some_and(|p| p.ends_with(format!("{key}.{}", media.ext)))
            });
            if ready || self.made.renders.contains_key(&key) {
                continue;
            }
            let cached = library::cache_path(&cache, &media);
            if cached.exists() {
                self.finish_render(media.id, &key, Ok(cached), cx);
            } else if self.doc.store.has_media(&key).unwrap_or(false) {
                self.unpack(media, cached, cx);
            } else {
                self.start_render(media, recipe, cx);
            }
        }
    }

    fn start_render(&mut self, media: Media, recipe: Recipe, cx: &mut Context<Self>) {
        let key = media.hash.clone();
        let out = library::cache_path(&dirs().cache, &media);
        let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
        self.made.renders.insert(
            key.clone(),
            Render {
                media: media.id,
                progress: progress.clone(),
                waiters: Vec::new(),
            },
        );
        self.made.errors.remove(&media.id);
        match &recipe {
            Recipe::Score(_) if !music::has_soundfont() => self.say(format!(
                "Fetching the instrument sounds ({} MB, once)…",
                music::SOUNDFONT_SIZE_MB
            )),
            _ => {}
        }
        if let Some(assets) = self.library.assets.get_mut(&key) {
            assets.loading = true;
        }
        let settings = self.doc.project.settings;
        let store = self.doc.path().to_path_buf();
        let id = media.id;
        // Redraw a few times a second so its progress shows.
        let watched = key.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let going = this
                    .update(cx, |editor, cx| {
                        cx.notify();
                        editor.made.renders.contains_key(&watched)
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let target = out.clone();
            let result = cx
                .background_spawn(async move {
                    match &recipe {
                        Recipe::Score(score) => music::render(score, &target),
                        Recipe::Scene(scene) => scenes::render(&settings, scene, &target, |p| {
                            progress.store(p.to_bits(), Ordering::Relaxed);
                        }),
                        Recipe::Symbol(symbol) => symbols::render(symbol, settings, &store, &target, |p| {
                            progress.store(p.to_bits(), Ordering::Relaxed);
                        })
                        .map(|content| {
                            // Kept beside the render, for when it is found in the cache.
                            if let Some(c) = content {
                                let _ = std::fs::write(
                                    content_file(&target),
                                    serde_json::to_string(&c).unwrap_or_default(),
                                );
                            }
                        }),
                    }
                    .map(|()| target)
                })
                .await;
            this.update(cx, |editor, cx| editor.finish_render(id, &key, result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_render(&mut self, id: MediaId, key: &str, result: anyhow::Result<PathBuf>, cx: &mut Context<Self>) {
        let waiters = self.made.renders.remove(key).map(|r| r.waiters).unwrap_or_default();
        let current = self.doc.project.media(id).is_some_and(|m| m.hash == key);
        let answer = match result {
            Ok(path) => {
                self.made.errors.remove(&id);
                let assets = self.library.entry(key);
                assets.path = Some(path.clone());
                if let Some(media) = self.doc.project.media(id).cloned().filter(|_| current) {
                    self.player.forget(id);
                    // A use of a symbol renders again from what the project
                    // keeps, so its render (large, and new with every fill)
                    // stays in the cache rather than the project file.
                    let symbol = matches!(media.recipe, Some(Recipe::Symbol(_)));
                    if !symbol && !self.doc.store.has_media(key).unwrap_or(false) {
                        self.embed(&media, path.clone(), cx);
                    }
                    if symbol {
                        let content = std::fs::read_to_string(content_file(&path))
                            .ok()
                            .and_then(|t| serde_json::from_str::<[f32; 4]>(&t).ok());
                        for m in self.doc.project.media.iter_mut().filter(|m| m.id == id) {
                            m.content = content;
                        }
                    }
                    self.build_assets(media, path, cx);
                    if self.player.is_playing() {
                        self.audio.stop();
                        self.player.seek(self.player.position);
                    }
                }
                serde_json::json!({ "ok": true, "rendered": key })
            }
            Err(e) => {
                let text = e.to_string();
                if let Some(assets) = self.library.assets.get_mut(key) {
                    assets.loading = false;
                }
                let name = self.doc.project.media(id).map(|m| m.name.clone()).unwrap_or_default();
                self.say_error(format!(
                    "Could not render {name}: {}",
                    text.lines().next().unwrap_or("")
                ));
                self.made.errors.insert(id, text.clone());
                serde_json::json!({ "ok": false, "error": text })
            }
        };
        for waiter in waiters {
            let _ = waiter.send(answer.to_string());
        }
        cx.notify();
    }

    /// Why export must wait: a score or scene on the timeline whose render
    /// is still running or failed.
    pub(super) fn unrendered_in_use(&self) -> Option<String> {
        let project = &self.doc.project;
        let used = |id: MediaId| {
            project
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(|c| c.media == id && c.generator.is_none())
        };
        for media in project.media.iter().filter(|m| m.recipe.is_some() && used(m.id)) {
            if self.render_of(media.id).is_some() {
                return Some(format!("{} is still rendering", media.name));
            }
            if let Some(e) = self.made.errors.get(&media.id) {
                return Some(format!(
                    "{} did not render ({})",
                    media.name,
                    e.lines().next().unwrap_or("")
                ));
            }
        }
        None
    }

    /// The render running for `media`, if any.
    pub(super) fn render_of(&self, media: MediaId) -> Option<&Render> {
        self.made.renders.values().find(|r| r.media == media)
    }

    // Scenes' code on disk.

    /// Writes the scene's code into its folder when that folder exists and
    /// differs, so an open studio or editor shows what the project holds.
    fn sync_scene_file(&mut self, id: MediaId, scene: &Scene) {
        let path = scenes::workspace(scene).join(scenes::SOURCE);
        let modified = || std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        // A save from outside not picked up yet wins; `poll_scenes` takes it next.
        if let (Some(on_disk), Some(seen)) = (modified(), self.made.seen.get(&id))
            && on_disk != *seen
        {
            return;
        }
        if scenes::write_file(scene).is_ok()
            && let Some(m) = modified()
        {
            self.made.seen.insert(id, m);
        }
    }

    /// Picks up saves to scenes' files made outside Splice: a text editor,
    /// an assistant.
    pub(super) fn poll_scenes(&mut self, cx: &mut Context<Self>) {
        let mut changed = Vec::new();
        for media in &self.doc.project.media {
            let Some(Recipe::Scene(scene)) = &media.recipe else {
                continue;
            };
            let Some((source, modified)) = scenes::read_source(scene) else {
                continue;
            };
            if self.made.seen.get(&media.id) == Some(&modified) {
                continue;
            }
            self.made.seen.insert(media.id, modified);
            if source != scene.source && !source.trim().is_empty() {
                changed.push((
                    media.id,
                    Scene {
                        source,
                        ..scene.clone()
                    },
                ));
            }
        }
        for (id, scene) in changed {
            let name = self.doc.project.media(id).map(|m| m.name.clone()).unwrap_or_default();
            self.edit(cx, |p| p.set_recipe(id, Recipe::Scene(scene)));
            self.say(format!("{name}: picked up the new code, rendering…"));
        }
    }

    // Making new ones.

    /// Adds a score (the starter if `score` is None) to the media and, with
    /// `place`, to the timeline. Returns the media and clip.
    pub(super) fn add_score(
        &mut self,
        score: Option<Score>,
        place: Option<(Time, usize)>,
        cx: &mut Context<Self>,
    ) -> (MediaId, Option<ClipId>) {
        let score = score.unwrap_or_else(Score::starter);
        let name = score.title.clone();
        let media = self.made_media(name, Recipe::Score(score));
        self.add_made(media, place, cx)
    }

    pub(super) fn add_scene(
        &mut self,
        name: String,
        source: Option<String>,
        duration: Time,
        transparent: bool,
        place: Option<(Time, usize)>,
        cx: &mut Context<Self>,
    ) -> (MediaId, Option<ClipId>) {
        let scene = Scene {
            source: source.unwrap_or_else(|| splice_core::motion::Spec::starter().into()),
            duration,
            transparent,
            workspace: scenes::new_workspace(),
        };
        let media = self.made_media(name, Recipe::Scene(scene));
        self.add_made(media, place, cx)
    }

    fn add_made(
        &mut self,
        media: Media,
        place: Option<(Time, usize)>,
        cx: &mut Context<Self>,
    ) -> (MediaId, Option<ClipId>) {
        let (id, clip) = self.edit(cx, |p| {
            let id = p.add_media(media);
            let clip = place.and_then(|(t, track)| p.insert_clip(id, track, t));
            (id, clip)
        });
        if let Some(clip) = clip {
            self.selection = vec![clip];
        }
        self.bin_selection = Some(id);
        self.reconcile_made(cx);
        (id, clip)
    }

    /// Opens a scene's JSON in a text editor; each save comes back here.
    pub(super) fn open_scene_code(&mut self, id: MediaId, cx: &mut Context<Self>) {
        let Some(Recipe::Scene(scene)) = self.doc.project.media(id).and_then(|m| m.recipe.clone()) else {
            return;
        };
        match scenes::write_file(&scene) {
            Ok(file) => {
                if let Ok(modified) = std::fs::metadata(&file).and_then(|m| m.modified()) {
                    self.made.seen.insert(id, modified);
                }
                // VS Code when it is here, else the Mac's text editor.
                let code = std::process::Command::new("code").arg(&file).spawn();
                if code.is_err() {
                    let _ = std::process::Command::new("open").arg("-t").arg(&file).spawn();
                }
                self.say("Editing the scene; every save re-renders it here.");
            }
            Err(e) => self.say_error(format!("Could not open the scene: {e}")),
        }
        cx.notify();
    }
}

/// General MIDI drum kit keys worth knowing.
pub const DRUM_KEYS: &[(u8, &str)] = &[
    (35, "acoustic bass drum"),
    (36, "kick"),
    (37, "side stick"),
    (38, "snare"),
    (39, "hand clap"),
    (40, "electric snare"),
    (41, "low floor tom"),
    (42, "closed hi-hat"),
    (43, "high floor tom"),
    (44, "pedal hi-hat"),
    (45, "low tom"),
    (46, "open hi-hat"),
    (47, "low-mid tom"),
    (48, "hi-mid tom"),
    (49, "crash cymbal"),
    (50, "high tom"),
    (51, "ride cymbal"),
    (53, "ride bell"),
    (54, "tambourine"),
    (55, "splash cymbal"),
    (56, "cowbell"),
    (57, "crash cymbal 2"),
    (60, "hi bongo"),
    (61, "low bongo"),
    (62, "mute hi conga"),
    (63, "open hi conga"),
    (64, "low conga"),
    (69, "cabasa"),
    (70, "maracas"),
    (75, "claves"),
    (76, "hi wood block"),
    (77, "low wood block"),
    (81, "open triangle"),
];

impl Editor {
    /// `splice ctl score …`, `scene …` and `open`. `None` answers later.
    pub(super) fn ctl_made(
        &mut self,
        cmd: &str,
        request: &serde_json::Value,
        reply: std::sync::mpsc::Sender<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<serde_json::Value> {
        use crate::ctl::fail;
        use serde_json::{Value, json};
        let text = |key: &str| request.get(key).and_then(Value::as_str);
        let sub = text("sub").unwrap_or("");
        let wait = request.get("wait").and_then(Value::as_bool) == Some(true);
        let place = match self.time(request, "at") {
            Ok(Some(t)) => Some((t, self.track(request).unwrap_or(0))),
            Ok(None) => None,
            Err(e) => return Some(fail(e)),
        };
        let media = |this: &Self, kind: &str| -> Result<Media, String> {
            let id = this.find_media(&request["media"])?;
            let media = this.doc.project.media(id).cloned().ok_or("no such media")?;
            match &media.recipe {
                Some(r) if r.kind() == kind => Ok(media),
                _ => Err(format!("{} is not a {kind}", media.name)),
            }
        };
        // Answers now, or when the render of `id` is done with `wait`.
        let answer = |this: &mut Self, id: MediaId, extra: Value| this.answer_made(id, extra, wait, reply.clone());

        match (cmd, sub) {
            ("score", "instruments") => Some(json!({
                "ok": true,
                "instruments": splice_core::score::GM.iter().enumerate()
                    .map(|(program, name)| json!({ "program": program, "name": name }))
                    .collect::<Vec<_>>(),
                "drums": {
                    "instrument": "drums",
                    "keys": DRUM_KEYS.iter().map(|(k, n)| json!({ "pitch": k, "name": n })).collect::<Vec<_>>(),
                },
                "note": "Use a name as a track's instrument (or set program). A track with \"drums\": true \
                         (or instrument \"drums\") plays the kit: its pitches pick the drum.",
            })),
            ("score", "new") => {
                let score = match request.get("content").and_then(Value::as_str) {
                    Some(content) => match serde_json::from_str::<Score>(content) {
                        Ok(score) => Some(score),
                        Err(e) => return Some(fail(format!("the score is not valid JSON for a score: {e}"))),
                    },
                    None => None,
                };
                let mut score = score.unwrap_or_else(Score::starter);
                if let Some(name) = text("name") {
                    score.title = name.to_string();
                }
                if let Err(e) = score.compile() {
                    return Some(fail(format!("the score does not compile: {e}")));
                }
                let (id, clip) = self.add_score(Some(score), place, cx);
                answer(self, id, json!({ "clip": clip.map(|c| c.0) }))
            }
            ("score", "read") => {
                let media = match media(self, "score") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(Recipe::Score(score)) = &media.recipe else {
                    return None;
                };
                let mut out = json!({ "ok": true, "media": self.made_json(media.id), "score": score });
                if let Some(why) = score.silence() {
                    out["warning"] = json!(why);
                }
                if let Ok(c) = score.compile() {
                    out["compiled"] = json!({
                        "seconds": c.seconds,
                        "music_seconds": c.music_seconds,
                        "bars": c.bars(c.length_ticks).len().saturating_sub(1),
                        "parts": c.parts.iter().map(|p| json!({
                            "id": p.id, "instrument": p.instrument, "program": p.program,
                            "plays": if p.drums { "drums".to_string() } else { splice_core::score::GM[p.program as usize].to_string() },
                            "notes": p.notes.len(),
                            "lowest": p.notes.iter().map(|n| n.key).min().map(splice_core::score::midi_name),
                            "highest": p.notes.iter().map(|n| n.key).max().map(splice_core::score::midi_name),
                        })).collect::<Vec<_>>(),
                    });
                }
                Some(out)
            }
            ("score", "write") => {
                let media = match media(self, "score") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(content) = request.get("content").and_then(Value::as_str) else {
                    return Some(fail("give the score with --file score.json"));
                };
                let score: Score = match serde_json::from_str(content) {
                    Ok(s) => s,
                    Err(e) => return Some(fail(format!("the score is not valid JSON for a score: {e}"))),
                };
                if let Err(e) = score.compile() {
                    return Some(fail(format!("the score does not compile: {e}")));
                }
                let id = media.id;
                let title = score.title.clone();
                let warning = score.silence();
                self.edit(cx, |p| {
                    p.set_recipe(id, Recipe::Score(score));
                    if let Some(m) = p.media.iter_mut().find(|m| m.id == id) {
                        m.name = title;
                    }
                });
                answer(self, id, warning.map_or_else(|| json!({}), |w| json!({ "warning": w })))
            }
            ("scene", "help") => Some(json!({
                "ok": true,
                "format": splice_core::motion::GUIDE,
                "example": serde_json::from_str::<Value>(splice_core::motion::Spec::starter()).unwrap_or(Value::Null),
            })),
            ("scene", "new") => {
                let duration = match self.time(request, "duration") {
                    Ok(d) => d.unwrap_or(5 * SECOND).max(SECOND / 10),
                    Err(e) => return Some(fail(e)),
                };
                let transparent = request.get("transparent").and_then(Value::as_bool).unwrap_or(true);
                let name = text("name").unwrap_or("Scene").to_string();
                let source = request.get("content").and_then(Value::as_str).map(str::to_string);
                if let Some(Err(e)) = source.as_deref().map(splice_core::motion::Spec::parse) {
                    return Some(fail(e.to_string()));
                }
                let (id, clip) = self.add_scene(name, source, duration, transparent, place, cx);
                answer(self, id, json!({ "clip": clip.map(|c| c.0) }))
            }
            ("scene", "check") => {
                let Some(content) = request.get("content").and_then(Value::as_str) else {
                    return Some(fail("give the scene with --file scene.json"));
                };
                let seconds = match self.time(request, "duration") {
                    Ok(d) => splice_core::time::secs(d.unwrap_or(5 * SECOND)),
                    Err(e) => return Some(fail(e)),
                };
                Some(match splice_core::motion::Spec::parse(content) {
                    Ok(spec) => json!({
                        "ok": true,
                        "warnings": self.scene_warnings(&spec, seconds),
                        "outline": splice_core::motion::outline(&spec),
                    }),
                    Err(e) => fail(e.to_string()),
                })
            }
            ("scene", "frame" | "sheet") => {
                let media = match media(self, "scene") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(Recipe::Scene(scene)) = media.recipe.clone() else {
                    return None;
                };
                let Some(path) = text("path").map(PathBuf::from) else {
                    return Some(fail("give a .png to write"));
                };
                let settings = self.doc.project.settings;
                let seconds = splice_core::time::secs(scene.duration);
                if sub == "frame" {
                    let at = match self.time(request, "at") {
                        Ok(t) => t.map_or(0.0, splice_core::time::secs),
                        Err(e) => return Some(fail(e)),
                    };
                    return Some(match scenes::still(&settings, &scene.source, at, &path) {
                        Ok(()) => json!({ "ok": true, "path": path, "at": at, "of": seconds }),
                        Err(e) => fail(e.to_string()),
                    });
                }
                let every = request.get("every").and_then(Value::as_f64);
                Some(match scenes::sheet(&settings, &scene.source, seconds, every, &path) {
                    Ok(times) => json!({ "ok": true, "path": path, "times": times, "of": seconds,
                        "note": "Frames left to right, top to bottom; the checkerboard is see-through." }),
                    Err(e) => fail(e.to_string()),
                })
            }
            ("scene", "patch" | "add" | "remove") => {
                let media = match media(self, "scene") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(Recipe::Scene(mut scene)) = media.recipe.clone() else {
                    return None;
                };
                let parse = |key: &str| -> Result<Value, String> {
                    let raw = text(key).ok_or(format!("give the JSON with --{key}"))?;
                    serde_json::from_str(raw).map_err(|e| format!("--{key} is not JSON: {e}"))
                };
                let edited = match sub {
                    "patch" => parse("json")
                        .map_err(anyhow::Error::msg)
                        .and_then(|p| splice_core::motion::patch(&scene.source, text("layer"), &p)),
                    "add" => parse("add")
                        .map_err(anyhow::Error::msg)
                        .and_then(|l| splice_core::motion::add(&scene.source, l, text("into"))),
                    _ => match text("remove") {
                        Some(name) => splice_core::motion::remove(&scene.source, name),
                        None => Err(anyhow::anyhow!("name the layer with --remove NAME")),
                    },
                };
                match edited {
                    Ok(source) => scene.source = source,
                    Err(e) => return Some(fail(e.to_string())),
                }
                let id = media.id;
                let spec = splice_core::motion::Spec::parse(&scene.source).ok();
                let seconds = splice_core::time::secs(scene.duration);
                self.edit(cx, |p| p.set_recipe(id, Recipe::Scene(scene)));
                let warnings = spec.map(|s| self.scene_warnings(&s, seconds)).unwrap_or_default();
                answer(self, id, json!({ "warnings": warnings }))
            }
            ("scene", "read") => {
                let media = match media(self, "scene") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(Recipe::Scene(scene)) = &media.recipe else {
                    return None;
                };
                let spec = splice_core::motion::Spec::parse(&scene.source).ok();
                let seconds = splice_core::time::secs(scene.duration);
                Some(json!({
                    "ok": true,
                    "media": self.made_json(media.id),
                    "outline": spec.as_ref().map(splice_core::motion::outline),
                    "warnings": spec.as_ref().map(|s| self.scene_warnings(s, seconds)),
                    "source": scene.source,
                    "duration": splice_core::time::secs(scene.duration),
                    "transparent": scene.transparent,
                    "folder": scenes::workspace(scene),
                    "file": scenes::workspace(scene).join(scenes::SOURCE),
                }))
            }
            ("scene", "write") => {
                let media = match media(self, "scene") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(Recipe::Scene(mut scene)) = media.recipe.clone() else {
                    return None;
                };
                if let Some(content) = request.get("content").and_then(Value::as_str) {
                    if let Err(e) = splice_core::motion::Spec::parse(content) {
                        return Some(fail(e.to_string()));
                    }
                    scene.source = content.to_string();
                }
                match self.time(request, "duration") {
                    Ok(Some(d)) => scene.duration = d.max(SECOND / 10),
                    Ok(None) => {}
                    Err(e) => return Some(fail(e)),
                }
                if let Some(t) = request.get("transparent").and_then(Value::as_bool) {
                    scene.transparent = t;
                }
                let id = media.id;
                let name = text("name").map(str::to_string);
                self.edit(cx, |p| {
                    p.set_recipe(id, Recipe::Scene(scene));
                    if let (Some(name), Some(m)) = (name, p.media.iter_mut().find(|m| m.id == id)) {
                        m.name = name;
                    }
                });
                answer(self, id, json!({}))
            }
            ("open", _) => {
                let id = match self.find_media(&request["media"]) {
                    Ok(id) => id,
                    Err(e) => return Some(fail(e)),
                };
                if self.doc.project.media(id).is_none_or(|m| m.recipe.is_none()) {
                    return Some(fail("only scores and scenes have their own editor"));
                }
                self.open_workbench(id, window, cx);
                Some(json!({ "ok": true }))
            }
            _ => Some(fail(format!("unknown command `{cmd} {sub}`; see `splice ctl --help`"))),
        }
    }

    /// What the person has open in the workbench, for the assistant: which
    /// score or scene, and how to read and change it.
    pub(super) fn editing_json(&self, id: MediaId) -> serde_json::Value {
        let mut out = self.made_json(id);
        match self.doc.project.media(id).and_then(|m| m.recipe.as_ref()) {
            Some(Recipe::Score(_)) => {
                out["how"] = serde_json::json!(format!(
                    "splice ctl score read --media {0}; write a new score with splice ctl score write --media {0} --file score.json --wait",
                    id.0
                ));
            }
            Some(Recipe::Scene(scene)) => {
                out["file"] = serde_json::json!(scenes::workspace(scene).join(scenes::SOURCE));
                out["how"] = serde_json::json!(format!(
                    "splice ctl scene read --media {0}; change it with splice ctl scene write --media {0} --file scene.json --wait; splice ctl scene help has the format",
                    id.0
                ));
            }
            Some(Recipe::Symbol(_)) => {
                out["how"] = serde_json::json!(format!(
                    "splice ctl symbol read --media {0}; fill a placeholder with splice ctl symbol fill --media {0} --label L --text WORDS --wait",
                    id.0
                ));
            }
            None => {}
        }
        out
    }

    /// What might be wrong with a scene that still renders.
    fn scene_warnings(&self, spec: &splice_core::motion::Spec, seconds: f64) -> Vec<String> {
        let families: Vec<String> = crate::titles::families().iter().map(|f| f.to_lowercase()).collect();
        splice_core::motion::lint(spec, seconds, |font| families.contains(&font.to_lowercase()))
    }

    /// A score or scene as `read` shows it.
    /// The reply to a change of made media: now, or with `wait` when its
    /// render is done (`None` then; the reply goes to `reply` later).
    pub(super) fn answer_made(
        &mut self,
        id: MediaId,
        extra: serde_json::Value,
        wait: bool,
        reply: std::sync::mpsc::Sender<String>,
    ) -> Option<serde_json::Value> {
        use serde_json::{Value, json};
        let mut reply_json = self.edited_with(extra);
        reply_json["media"] = self.made_json(id);
        let hash = self.doc.project.media(id).map(|m| m.hash.clone()).unwrap_or_default();
        if wait {
            if let Some(render) = self.made.renders.get_mut(&hash) {
                let (tx, rx) = std::sync::mpsc::channel::<String>();
                render.waiters.push(tx);
                let base = reply_json.clone();
                std::thread::spawn(move || {
                    if let Ok(done) = rx.recv() {
                        let done: Value = serde_json::from_str(&done).unwrap_or(json!({}));
                        let mut out = base;
                        out["ok"] = done["ok"].clone();
                        if let Some(e) = done.get("error") {
                            out["error"] = e.clone();
                        }
                        out["media"]["rendering"] = json!(null);
                        let _ = reply.send(out.to_string());
                    }
                });
                return None;
            }
            if let Some(e) = self.made.errors.get(&id) {
                reply_json["ok"] = json!(false);
                reply_json["error"] = json!(e);
            }
        }
        Some(reply_json)
    }

    pub(super) fn made_json(&self, id: MediaId) -> serde_json::Value {
        use serde_json::json;
        let Some(media) = self.doc.project.media(id) else {
            return json!({ "id": id.0, "gone": true });
        };
        let assets = self.library.get(&media.hash);
        json!({
            "id": id.0,
            "name": media.name,
            "made": media.recipe.as_ref().map(|r| r.kind()),
            "duration": splice_core::time::secs(media.duration),
            "ready": assets.is_some_and(|a| a.path.is_some() && !a.loading),
            "rendering": self.render_of(id).map(|r| r.progress()),
            "render_error": self.made.errors.get(&id),
        })
    }
}
