//! Media Splice makes: scores and scenes. Each is a recipe plus a render of
//! it. After any change, `reconcile_made` points the media at the render of
//! its current recipe, finding it in the cache or the project file or
//! rendering it, while the previous render keeps playing.

use super::*;
use crate::{music, scenes};
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
    /// Clapper studios open on scenes.
    pub studios: HashMap<MediaId, scenes::Studio>,
    /// When each scene's code file last changed, as last read.
    pub seen: HashMap<MediaId, SystemTime>,
}

/// Engine names, so a new synth or Clapper renders afresh.
pub fn engine(recipe: &Recipe) -> &'static str {
    match recipe {
        Recipe::Score(_) => music::ENGINE,
        Recipe::Scene(_) => scenes::ENGINE,
    }
}

pub fn ext(recipe: &Recipe) -> &'static str {
    match recipe {
        Recipe::Score(_) => "wav",
        Recipe::Scene(_) => "mov",
    }
}

impl Editor {
    /// Media for a recipe, not yet rendered, named `name`.
    pub(super) fn made_media(&self, name: String, recipe: Recipe) -> Media {
        let settings = self.doc.project.settings;
        let (kind, width, height, audio) = match &recipe {
            Recipe::Score(_) => (MediaKind::Audio, 0, 0, true),
            Recipe::Scene(_) => (MediaKind::Video, settings.width, settings.height, false),
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
            let mut media = media;
            if media.hash != key || media.duration != duration {
                // Keep showing the old render until the new one is ready.
                let old = media.hash.clone();
                if old != key && self.library.get(&key).is_none_or(|a| a.path.is_none()) {
                    self.library.borrow_render(&old, &key);
                }
                self.doc.project.set_render(media.id, &key, duration);
                if matches!(recipe, Recipe::Scene(_)) {
                    for m in self.doc.project.media.iter_mut().filter(|m| m.id == media.id) {
                        m.width = settings.width;
                        m.height = settings.height;
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
            Recipe::Scene(_) if scenes::launcher().is_none() => {
                self.say("Fetching Clapper to render scenes (a few hundred MB, once)…")
            }
            _ => {}
        }
        if let Some(assets) = self.library.assets.get_mut(&key) {
            assets.loading = true;
        }
        let settings = self.doc.project.settings;
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
                    if !self.doc.store.has_media(key).unwrap_or(false) {
                        self.embed(&media, path.clone(), cx);
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
        if !path.exists() {
            return;
        }
        if std::fs::read_to_string(&path).ok().as_deref() != Some(scene.source.as_str()) {
            let _ = std::fs::write(&path, &scene.source);
        }
        if let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            self.made.seen.insert(id, modified);
        }
    }

    /// Picks up saves to scenes' code made outside Splice: a text editor,
    /// Clapper's studio, an agent.
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
            source: source.unwrap_or_else(|| scenes::STARTER.into()),
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

    /// Opens Clapper's studio on a scene in the browser.
    pub(super) fn open_studio(&mut self, id: MediaId, cx: &mut Context<Self>) {
        if let Some(studio) = self.made.studios.get(&id) {
            cx.open_url(&studio.url);
            return;
        }
        let Some(Recipe::Scene(scene)) = self.doc.project.media(id).and_then(|m| m.recipe.clone()) else {
            return;
        };
        let settings = self.doc.project.settings;
        self.say("Starting the Clapper studio…");
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { scenes::studio(&settings, &scene) })
                .await;
            this.update(cx, |editor, cx| {
                match result {
                    Ok(studio) => {
                        cx.open_url(&studio.url);
                        editor.say("The studio is open in your browser; saves there come back here.");
                        editor.made.studios.insert(id, studio);
                    }
                    Err(e) => editor.say_error(format!("Could not start the studio: {e}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens a scene's code in the system's editor for .tsx files.
    pub(super) fn open_scene_code(&mut self, id: MediaId, cx: &mut Context<Self>) {
        let Some(Recipe::Scene(scene)) = self.doc.project.media(id).and_then(|m| m.recipe.clone()) else {
            return;
        };
        let settings = self.doc.project.settings;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let launcher = scenes::ensure_launcher()?;
                    let dir = scenes::prepare(&launcher, &settings, &scene)?;
                    Ok::<_, anyhow::Error>(dir)
                })
                .await;
            this.update(cx, |editor, cx| {
                match result {
                    Ok(dir) => {
                        let file = dir.join(scenes::SOURCE);
                        // VS Code when it is here (it knows TSX), else the default app.
                        let code = std::process::Command::new("code").arg(&dir).arg(&file).spawn();
                        if code.is_err() {
                            let _ = std::process::Command::new("open").arg("-t").arg(&file).spawn();
                        }
                        editor.say("Editing the scene's code; every save re-renders it here.");
                    }
                    Err(e) => editor.say_error(format!("Could not open the scene: {e}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let answer = |this: &mut Self, id: MediaId, extra: Value| -> Option<Value> {
            let mut reply_json = this.edited_with(extra);
            reply_json["media"] = this.made_json(id);
            let hash = this.doc.project.media(id).map(|m| m.hash.clone()).unwrap_or_default();
            if wait {
                if let Some(render) = this.made.renders.get_mut(&hash) {
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
                if let Some(e) = this.made.errors.get(&id) {
                    reply_json["ok"] = json!(false);
                    reply_json["error"] = json!(e);
                }
            }
            Some(reply_json)
        };

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
                self.edit(cx, |p| {
                    p.set_recipe(id, Recipe::Score(score));
                    if let Some(m) = p.media.iter_mut().find(|m| m.id == id) {
                        m.name = title;
                    }
                });
                answer(self, id, json!({}))
            }
            ("scene", "new") => {
                let duration = match self.time(request, "duration") {
                    Ok(d) => d.unwrap_or(5 * SECOND).max(SECOND / 10),
                    Err(e) => return Some(fail(e)),
                };
                let transparent = request.get("transparent").and_then(Value::as_bool).unwrap_or(true);
                let name = text("name").unwrap_or("Scene").to_string();
                let source = request.get("content").and_then(Value::as_str).map(str::to_string);
                let (id, clip) = self.add_scene(name, source, duration, transparent, place, cx);
                answer(self, id, json!({ "clip": clip.map(|c| c.0) }))
            }
            ("scene", "read") => {
                let media = match media(self, "scene") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(Recipe::Scene(scene)) = &media.recipe else {
                    return None;
                };
                Some(json!({
                    "ok": true,
                    "media": self.made_json(media.id),
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
            ("scene", "studio") => {
                let media = match media(self, "scene") {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                self.open_studio(media.id, cx);
                Some(json!({ "ok": true, "opening": true }))
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
                    "splice ctl scene read --media {0}; change it with splice ctl scene write --media {0} --file scene.tsx --wait",
                    id.0
                ));
            }
            None => {}
        }
        out
    }

    /// A score or scene as `read` shows it.
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
