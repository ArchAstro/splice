//! The editor's half of `splice ctl`: answering requests from other programs
//! against the live project, and the conversation with a connected assistant.
//!
//! Edits from outside go through the same `edit` as the mouse and keys, so
//! each is one undo step, autosaves, and shows at once. Nothing here moves
//! the person's selection or playhead unless the request is about those.

use super::*;
use crate::ctl::{Incoming, fail};
use serde_json::{Value, json};
use splice_core::time::from_secs;
use std::collections::VecDeque;
use std::sync::mpsc::Sender;

/// An assistant blocked in `splice ctl wait`.
struct Waiter {
    id: u64,
    name: String,
    reply: Sender<String>,
}

#[derive(Default)]
pub struct Agent {
    waiters: VecDeque<Waiter>,
    /// Messages sent while the assistant was busy with an earlier one.
    outbox: VecDeque<String>,
    /// The assistant took a message and has not come back for another yet.
    working: Option<String>,
}

/// How the title bar shows the assistant.
pub enum Presence {
    Listening(String),
    Working(String),
}

impl Agent {
    pub fn presence(&self) -> Option<Presence> {
        if let Some(name) = &self.working {
            return Some(Presence::Working(name.clone()));
        }
        self.waiters.front().map(|w| Presence::Listening(w.name.clone()))
    }
}

fn s(t: Time) -> f64 {
    (secs(t) * 1000.0).round() / 1000.0
}

fn kind(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Video => "video",
        MediaKind::Image => "image",
        MediaKind::Audio => "audio",
    }
}

impl Editor {
    pub(super) fn ctl_incoming(&mut self, incoming: Incoming, window: &mut Window, cx: &mut Context<Self>) {
        match incoming {
            Incoming::Gone(id) => {
                self.agent.waiters.retain(|w| w.id != id);
                cx.notify();
            }
            Incoming::Request { id, line, reply } => {
                let request: Value = match serde_json::from_str(&line) {
                    Ok(request) => request,
                    Err(_) => {
                        let _ = reply.send(fail("the request is not JSON").to_string());
                        return;
                    }
                };
                // Requests that answer later keep the reply; the rest answer now.
                if let Some(answer) = self.ctl_request(id, &request, reply.clone(), window, cx) {
                    let _ = reply.send(answer.to_string());
                }
                cx.notify();
            }
        }
    }

    fn ctl_request(
        &mut self,
        id: u64,
        request: &Value,
        reply: Sender<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Value> {
        let text = |key: &str| request.get(key).and_then(Value::as_str);
        let flag = |key: &str| request.get(key).and_then(Value::as_bool);
        let cmd = text("cmd").unwrap_or("");
        let answer = match cmd {
            "status" => self.ctl_status(),
            "read" => self.ctl_read(),
            "versions" => {
                let list: Vec<Value> = self
                    .doc
                    .versions()
                    .iter()
                    .map(|v| {
                        json!({
                            "id": v.id,
                            "kind": match v.kind { VersionKind::Save => "saved", VersionKind::Auto => "autosave" },
                            "label": v.label,
                            "ago": render::ago(SystemTime::now(), v.created),
                        })
                    })
                    .collect();
                json!({ "ok": true, "versions": list })
            }
            "restore" => match request.get("version").and_then(Value::as_i64) {
                Some(version) => {
                    self.restore(version, window, cx);
                    self.edited()
                }
                None => fail("name the version with `version` (an id from `splice ctl versions`)"),
            },
            "import" => {
                let files: Vec<PathBuf> = request["files"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).map(PathBuf::from).collect())
                    .unwrap_or_default();
                if files.is_empty() {
                    return Some(fail("`files` is missing"));
                }
                if let Some(missing) = files.iter().find(|f| !f.exists()) {
                    return Some(fail(format!("{} does not exist", missing.display())));
                }
                let place = match self.time(request, "at") {
                    Ok(Some(t)) => Some((t, self.track(request).unwrap_or(0))),
                    Ok(None) => None,
                    Err(e) => return Some(fail(e)),
                };
                self.import_paths(files, place, Some(reply), cx);
                return None;
            }
            "add" => {
                let media = match self.find_media(&request["media"]) {
                    Ok(media) => media,
                    Err(e) => return Some(fail(e)),
                };
                let track = self.track(request).unwrap_or(0);
                let at = match self.time(request, "at") {
                    Ok(Some(t)) => t,
                    Ok(None) => self.doc.project.tracks.get(track).map_or(0, |t| t.end()),
                    Err(e) => return Some(fail(e)),
                };
                match self.edit(cx, |p| p.insert_clip(media, track, at)) {
                    Some(clip) => self.edited_with(json!({ "clip": self.clip_json(clip) })),
                    None => fail("that media has nothing to place"),
                }
            }
            "move" => {
                let ids = match self.clips(request) {
                    Ok(ids) => ids,
                    Err(e) => return Some(fail(e)),
                };
                let first = self.doc.project.slot(ids[0]).expect("checked");
                let start = self.doc.project.clip(ids[0]).map_or(0, |c| c.start);
                let dt = match self.time(request, "to") {
                    Ok(Some(to)) => to - start,
                    Ok(None) => 0,
                    Err(e) => return Some(fail(e)),
                };
                let dtrack = self.track(request).map_or(0, |t| t as isize - first.track as isize);
                self.edit(cx, |p| p.move_clips(&ids, dt, dtrack));
                let clips: Vec<Value> = ids.iter().map(|id| self.clip_json(*id)).collect();
                self.edited_with(json!({ "clips": clips }))
            }
            "trim" => {
                let ids = match self.clips(request) {
                    Ok(ids) => ids,
                    Err(e) => return Some(fail(e)),
                };
                let (start, end) = match (self.time(request, "start"), self.time(request, "end")) {
                    (Ok(start), Ok(end)) => (start, end),
                    (Err(e), _) | (_, Err(e)) => return Some(fail(e)),
                };
                if start.is_none() && end.is_none() {
                    return Some(fail("give `start`, `end` or both: where the clip's edges go"));
                }
                self.edit(cx, |p| {
                    for &id in &ids {
                        if let Some(t) = start {
                            p.trim_start(id, t);
                        }
                        if let Some(t) = end {
                            p.trim_end(id, t);
                        }
                    }
                });
                let clips: Vec<Value> = ids.iter().map(|id| self.clip_json(*id)).collect();
                self.edited_with(json!({ "clips": clips }))
            }
            "split" => {
                let at = match self.time(request, "at") {
                    Ok(t) => t.unwrap_or(self.player.position),
                    Err(e) => return Some(fail(e)),
                };
                let ids = if request.get("clips").is_some() {
                    match self.clips(request) {
                        Ok(ids) => ids,
                        Err(e) => return Some(fail(e)),
                    }
                } else {
                    Vec::new()
                };
                let made = self.edit(cx, |p| p.split_at(at, &ids));
                if made.is_empty() {
                    fail(format!("nothing to split at {:.3}s", secs(at)))
                } else {
                    let clips: Vec<Value> = made.iter().map(|id| self.clip_json(*id)).collect();
                    self.edited_with(json!({ "new": clips }))
                }
            }
            "delete" => {
                let ids = match self.clips(request) {
                    Ok(ids) => ids,
                    Err(e) => return Some(fail(e)),
                };
                if flag("ripple") == Some(true) {
                    self.edit(cx, |p| p.ripple_delete(&ids));
                } else {
                    self.edit(cx, |p| p.delete(&ids));
                }
                self.edited()
            }
            "duplicate" => {
                let ids = match self.clips(request) {
                    Ok(ids) => ids,
                    Err(e) => return Some(fail(e)),
                };
                let copies = self.edit(cx, |p| p.duplicate(&ids));
                let clips: Vec<Value> = copies.iter().map(|id| self.clip_json(*id)).collect();
                self.edited_with(json!({ "new": clips }))
            }
            "volume" => {
                let ids = match self.clips(request) {
                    Ok(ids) => ids,
                    Err(e) => return Some(fail(e)),
                };
                let Some(volume) = request.get("value").and_then(Value::as_f64) else {
                    return Some(fail("`value` is missing: 1 is as recorded, 0 silent"));
                };
                self.edit(cx, |p| p.set_volume(&ids, volume as f32));
                self.edited()
            }
            "track" => {
                let Some(track) = self.track(request).filter(|t| *t < self.doc.project.tracks.len()) else {
                    return Some(fail(format!(
                        "name a track from 1 to {} with `track`",
                        self.doc.project.tracks.len()
                    )));
                };
                let (muted, hidden) = (flag("muted"), flag("hidden"));
                self.edit(cx, |p| {
                    let t = &mut p.tracks[track];
                    t.muted = muted.unwrap_or(t.muted);
                    t.hidden = hidden.unwrap_or(t.hidden);
                });
                self.edited()
            }
            "canvas" => {
                let mut settings = self.doc.project.settings;
                if let Some(aspect) = text("aspect") {
                    match ASPECTS.iter().find(|(name, _, _)| *name == aspect) {
                        Some((_, w, h)) => (settings.width, settings.height) = (*w, *h),
                        None => {
                            let names: Vec<&str> = ASPECTS.iter().map(|(n, _, _)| *n).collect();
                            return Some(fail(format!("`aspect` is one of {}", names.join(", "))));
                        }
                    }
                }
                if let Some(size) = text("size") {
                    let parsed = size
                        .split_once(['x', 'X', '×'])
                        .and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)));
                    match parsed {
                        Some((w, h)) if (16..=8192).contains(&w) && (16..=8192).contains(&h) => {
                            (settings.width, settings.height) = (w & !1, h & !1)
                        }
                        _ => return Some(fail("`size` is WIDTHxHEIGHT, from 16 to 8192 each")),
                    }
                }
                if let Some(fps) = request.get("fps").and_then(Value::as_u64) {
                    if !(1..=120).contains(&fps) {
                        return Some(fail("`fps` is from 1 to 120"));
                    }
                    settings.fps = fps as u32;
                }
                self.edit(cx, |p| p.settings = settings);
                self.edited()
            }
            "undo" | "redo" => {
                let changed = if cmd == "undo" {
                    self.doc.undo()
                } else {
                    self.doc.redo()
                };
                self.after_edit();
                if changed {
                    self.edited()
                } else {
                    fail(format!("nothing to {cmd}"))
                }
            }
            "seek" => match self.time(request, "at") {
                Ok(Some(t)) => {
                    self.seek(t, cx);
                    self.follow_playhead();
                    json!({ "ok": true, "playhead": s(self.player.position) })
                }
                Ok(None) => fail("`at` is missing"),
                Err(e) => fail(e),
            },
            "play" | "pause" => {
                if self.player.is_playing() != (cmd == "play") {
                    self.play_pause(&PlayPause, window, cx);
                }
                json!({ "ok": true, "playing": self.player.is_playing(), "playhead": s(self.player.position) })
            }
            "select" => {
                let ids = if request.get("clips").is_some() {
                    match self.clips(request) {
                        Ok(ids) => ids,
                        Err(e) => return Some(fail(e)),
                    }
                } else {
                    Vec::new()
                };
                self.selection = ids;
                json!({ "ok": true, "selection": self.selection.iter().map(|c| c.0).collect::<Vec<_>>() })
            }
            "command" => {
                let words = text("command").unwrap_or("");
                let found = Self::palette_matches(words).into_iter().next();
                match found {
                    Some((label, _, action)) if !words.trim().is_empty() => {
                        window.dispatch_action(action, cx);
                        json!({ "ok": true, "ran": label })
                    }
                    _ => fail(format!(
                        "no command matches `{words}`; the command menu (Cmd-/) lists them"
                    )),
                }
            }
            "save" => match self.doc.save(text("label")) {
                Ok(()) => {
                    self.say("Saved a version");
                    json!({ "ok": true, "path": self.doc.path() })
                }
                Err(e) => fail(format!("save failed: {e}")),
            },
            "frame" => {
                let Some(path) = text("path").map(PathBuf::from) else {
                    return Some(fail("`path` is missing"));
                };
                let at = match self.time(request, "at") {
                    Ok(t) => t.unwrap_or(self.player.position),
                    Err(e) => return Some(fail(e)),
                };
                let library = &self.library;
                let plan = export::plan_still(
                    &self.doc.project,
                    |m| library.path(&m.hash).map(Path::to_path_buf),
                    at,
                    &path,
                );
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(e) => return Some(fail(e)),
                };
                let (w, h) = (self.doc.project.settings.width, self.doc.project.settings.height);
                cx.background_spawn(async move {
                    let result = ffmpeg::export_command()
                        .map_err(|e| e.to_string())
                        .and_then(|mut command| {
                            let out = command.args(&plan.args).output().map_err(|e| e.to_string())?;
                            if out.status.success() {
                                Ok(())
                            } else {
                                let err = String::from_utf8_lossy(&out.stderr);
                                Err(err.lines().last().unwrap_or("ffmpeg failed").to_string())
                            }
                        });
                    let answer = match result {
                        Ok(()) => json!({ "ok": true, "path": path, "at": s(at), "width": w, "height": h }),
                        Err(e) => fail(e),
                    };
                    let _ = reply.send(answer.to_string());
                })
                .detach();
                return None;
            }
            "export" => {
                let Some(path) = text("path").map(PathBuf::from) else {
                    return Some(fail("`path` is missing"));
                };
                if self.export.is_some() {
                    return Some(fail("an export is already running"));
                }
                let quality = match text("quality") {
                    Some("draft") => Quality::Draft,
                    Some("high") => Quality::High,
                    _ => Quality::Standard,
                };
                if let Err(e) = self.start_export(path.clone(), quality, false, cx) {
                    return Some(fail(e));
                }
                if flag("wait") == Some(true) {
                    self.export_replies.push(reply);
                    return None;
                }
                json!({ "ok": true, "path": path, "started": true })
            }
            "say" => match text("text").map(str::trim).filter(|t| !t.is_empty()) {
                Some(said) => {
                    let name = text("name").or(self.agent_name()).unwrap_or("Claude").to_string();
                    self.say(format!("{name}: {}", said.lines().next().unwrap_or("")));
                    json!({ "ok": true })
                }
                None => fail("`text` is missing"),
            },
            "wait" => {
                let name = text("name")
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or("Claude")
                    .to_string();
                self.agent.working = None;
                match self.agent.outbox.pop_front() {
                    Some(message) => {
                        let _ = reply.send(message);
                        self.agent.working = Some(name);
                    }
                    None => self.agent.waiters.push_back(Waiter { id, name, reply }),
                }
                return None;
            }
            other => fail(format!("unknown command `{other}`; `splice ctl --help` lists them")),
        };
        Some(answer)
    }

    fn agent_name(&self) -> Option<&str> {
        self.agent
            .working
            .as_deref()
            .or_else(|| self.agent.waiters.front().map(|w| w.name.as_str()))
    }

    /// Hands the person's message to the assistant, or queues it while the
    /// assistant is busy with the last one.
    pub(super) fn send_to_agent(&mut self, text: String, cx: &mut Context<Self>) {
        let selection: Vec<Value> = self.selection.iter().map(|id| self.clip_json(*id)).collect();
        let message = json!({
            "ok": true,
            "message": text,
            "project": self.doc.path(),
            "revision": self.doc.revision,
            "playhead": s(self.player.position),
            "selection": selection,
            "media_selected": self.bin_selection.and_then(|id| self.doc.project.media(id)).map(|m| json!({ "id": m.id.0, "name": m.name })),
        })
        .to_string();
        let mut message = Some(message);
        while let (Some(waiter), Some(text)) = (self.agent.waiters.pop_front(), message.take()) {
            match waiter.reply.send(text) {
                Ok(()) => self.agent.working = Some(waiter.name),
                Err(unsent) => message = Some(unsent.0),
            }
        }
        match (message, self.agent.working.clone()) {
            (None, Some(name)) => self.say(format!("Sent to {name}")),
            (Some(message), Some(name)) => {
                self.agent.outbox.push_back(message);
                self.say(format!("{name} is busy; your message is queued"));
            }
            _ => self.say_error("No assistant is connected — one connects with `splice ctl wait`"),
        }
        cx.notify();
    }

    fn edited(&self) -> Value {
        self.edited_with(json!({}))
    }

    fn edited_with(&self, mut extra: Value) -> Value {
        let base = json!({
            "ok": true,
            "revision": self.doc.revision,
            "duration": s(self.doc.project.duration()),
            "tracks": self.doc.project.tracks.len(),
        });
        if let (Some(extra), Value::Object(base)) = (extra.as_object_mut(), base) {
            for (k, v) in base {
                extra.entry(k).or_insert(v);
            }
        }
        extra
    }

    /// A clip as `ctl` shows it: times in seconds, its track counted from 1.
    fn clip_json(&self, id: ClipId) -> Value {
        let project = &self.doc.project;
        let (Some(slot), Some(clip)) = (project.slot(id), project.clip(id)) else {
            return json!({ "id": id.0, "gone": true });
        };
        let media = project.media(clip.media);
        json!({
            "id": clip.id.0,
            "media": clip.media.0,
            "name": media.map(|m| m.name.as_str()),
            "kind": media.map(|m| kind(m.kind)),
            "track": slot.track + 1,
            "start": s(clip.start),
            "end": s(clip.end()),
            "duration": s(clip.duration),
            "source_in": s(clip.source_in),
            "volume": clip.volume,
        })
    }

    fn ctl_status(&self) -> Value {
        let project = &self.doc.project;
        let settings = project.settings;
        let export = self.export.as_ref().map(|job| {
            let job = job.lock().unwrap();
            json!({ "path": job.output, "progress": (job.progress * 100.0).round() / 100.0 })
        });
        json!({
            "ok": true,
            "project": {
                "name": self.doc.name(),
                "path": self.doc.path(),
                "untitled": self.doc.is_untitled(),
                "saved": !self.doc.edited,
                "revision": self.doc.revision,
            },
            "canvas": { "width": settings.width, "height": settings.height, "fps": settings.fps },
            "duration": s(project.duration()),
            "playhead": s(self.player.position),
            "playing": self.player.is_playing(),
            "selection": self.selection.iter().map(|c| c.0).collect::<Vec<_>>(),
            "media": project.media.len(),
            "tracks": project.tracks.len(),
            "clips": project.clips().count(),
            "preparing": self.working > 0,
            "export": export,
        })
    }

    fn ctl_read(&self) -> Value {
        let project = &self.doc.project;
        let media: Vec<Value> = project
            .media
            .iter()
            .map(|m| {
                let assets = self.library.get(&m.hash);
                json!({
                    "id": m.id.0,
                    "name": m.name,
                    "kind": kind(m.kind),
                    "duration": m.is_timed().then(|| s(m.duration)),
                    "width": m.is_visual().then_some(m.width),
                    "height": m.is_visual().then_some(m.height),
                    "audio": m.has_audio,
                    "file": assets.and_then(|a| a.path.as_ref()),
                    "ready": assets.is_some_and(|a| a.path.is_some() && !a.loading),
                })
            })
            .collect();
        let tracks: Vec<Value> = project
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                json!({
                    "track": i + 1,
                    "muted": t.muted,
                    "hidden": t.hidden,
                    "clips": t.clips.iter().map(|c| self.clip_json(c.id)).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut status = self.ctl_status();
        status["media"] = json!(media);
        status["tracks"] = json!(tracks);
        status.as_object_mut().map(|o| o.remove("clips"));
        status
    }

    /// A time field: seconds, or "end" for the end of the movie.
    fn time(&self, request: &Value, key: &str) -> Result<Option<Time>, String> {
        match request.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(word)) if word == "end" => Ok(Some(self.doc.project.duration())),
            Some(Value::String(text)) => crate::ctl::parse_time(text)
                .map(|t| Some(from_secs(t)))
                .ok_or(format!("`{key}` is a time in seconds")),
            Some(v) => v
                .as_f64()
                .filter(|t| *t >= 0.0)
                .map(|t| Some(from_secs(t)))
                .ok_or(format!("`{key}` is a time in seconds")),
        }
    }

    /// The track named in a request, as an index (requests count from 1).
    fn track(&self, request: &Value) -> Option<usize> {
        request
            .get("track")
            .and_then(Value::as_u64)
            .map(|t| (t as usize).saturating_sub(1))
    }

    fn clips(&self, request: &Value) -> Result<Vec<ClipId>, String> {
        let ids: Vec<ClipId> = match &request["clips"] {
            Value::Array(ids) => ids.iter().filter_map(Value::as_u64).map(ClipId).collect(),
            Value::Number(id) => id.as_u64().map(ClipId).into_iter().collect(),
            _ => Vec::new(),
        };
        if ids.is_empty() {
            return Err("name clips with `clips` (ids from `splice ctl read`)".into());
        }
        match ids.iter().find(|id| self.doc.project.slot(**id).is_none()) {
            Some(missing) => Err(format!("there is no clip {}", missing.0)),
            None => Ok(ids),
        }
    }

    fn find_media(&self, spec: &Value) -> Result<MediaId, String> {
        let media = &self.doc.project.media;
        let found = match spec {
            Value::Number(id) => media.iter().find(|m| Some(m.id.0) == id.as_u64()),
            Value::String(name) => media.iter().find(|m| m.name == *name).or_else(|| {
                let lower = name.to_lowercase();
                let mut close = media.iter().filter(|m| m.name.to_lowercase().contains(&lower));
                match (close.next(), close.next()) {
                    (Some(only), None) => Some(only),
                    _ => None,
                }
            }),
            _ => return Err("name the media with `media`: its id or file name".into()),
        };
        found
            .map(|m| m.id)
            .ok_or(format!("no media matches {spec}; `splice ctl read` lists it"))
    }
}
