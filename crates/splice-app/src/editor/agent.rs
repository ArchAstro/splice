//! The editor's half of `splice ctl`: answering requests from other programs
//! against the live project, and the conversation with a connected assistant.
//!
//! Edits from outside go through the same `edit` as the mouse and keys, so
//! each is one undo step, autosaves, and shows at once. Nothing here moves
//! the person's selection or playhead unless the request is about those.

use super::*;
use crate::ctl::{Incoming, fail};
use crate::titles;
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

/// `#rrggbb`, or `#rrggbbaa` when not opaque.
fn hex(color: u32) -> String {
    if color & 0xff == 0xff {
        format!("#{:06x}", color >> 8)
    } else {
        format!("#{color:08x}")
    }
}

/// A colour from `#rgb`, `#rrggbb`, `#rrggbbaa` or a few names: 0xRRGGBBAA.
pub fn parse_color(text: &str) -> Option<u32> {
    let named = match text.to_lowercase().as_str() {
        "white" => Some(0xffffffff),
        "black" => Some(0x000000ff),
        "red" => Some(0xe5484dff),
        "green" => Some(0x00ff00ff),
        "blue" => Some(0x0047bbff),
        "yellow" | "gold" => Some(0xf2c14eff),
        "paper" => Some(0xf3eee4ff),
        "ink" => Some(0x12151fff),
        "vermilion" | "accent" => Some(0xef6a55ff),
        "clear" | "transparent" => Some(0),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let digits = text.trim_start_matches('#');
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        3 => {
            let (r, g, b) = ((value >> 8) & 0xf, (value >> 4) & 0xf, value & 0xf);
            Some((r * 17) << 24 | (g * 17) << 16 | (b * 17) << 8 | 0xff)
        }
        6 => Some(value << 8 | 0xff),
        8 => Some(value),
        _ => None,
    }
}

fn look_json(look: &splice_core::look::Look) -> Value {
    json!({
        "x": look.x,
        "y": look.y,
        "scale": look.scale,
        "opacity": look.opacity,
        "enter": { "kind": look.enter.kind.name(), "duration": s(look.enter.duration) },
        "exit": { "kind": look.exit.kind.name(), "duration": s(look.exit.duration) },
        "drift": look.drift,
        "key": look.key.map(|k| json!({
            "color": hex(k.color << 8 | 0xff),
            "similarity": k.similarity,
            "blend": k.blend,
            "despill": k.despill,
        })),
        "mask": look.mask,
        "crop": look.crop,
        "effects": (!look.fx.is_none()).then_some(look.fx),
        "backdrop": look.backdrop,
        "redact": look.redact,
        "grade": (!look.grade.is_none()).then(|| json!({
            "look": look.grade.preset.name(),
            "exposure": look.grade.exposure,
            "contrast": look.grade.contrast,
            "saturation": look.grade.saturation,
            "warmth": look.grade.temperature,
            "tint": look.grade.tint,
        })),
        "keyframes": (!look.keys.is_empty()).then(|| keyframes_json(look)),
    })
}

/// Keyframes per property: clip time (seconds from its start), value, ease.
fn keyframes_json(look: &splice_core::look::Look) -> Value {
    let mut out = serde_json::Map::new();
    for prop in splice_core::look::Prop::ALL {
        let keys = look.keys.of(prop);
        if keys.is_empty() {
            continue;
        }
        out.insert(
            prop.name().into(),
            json!(
                keys.iter()
                    .map(|k| json!({ "at": s(k.at), "value": k.value, "ease": k.ease.name() }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    Value::Object(out)
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
            "score" | "scene" | "open" => return self.ctl_made(cmd, request, reply, window, cx),
            "story" if request["sub"] == json!("shot") => return self.ctl_shot(request, reply, cx),
            "story" if matches!(request["sub"].as_str(), Some("cast" | "brief" | "take")) => {
                return self.ctl_plan(request, reply, cx);
            }
            "bible" => return self.ctl_bible(request, reply, cx),
            "captions" => return self.ctl_captions(request, reply, cx),
            "transition" | "speed" | "freeze" | "keyframe" | "leak" | "zoom" => self.ctl_effects(cmd, request, cx),
            "story" | "cue" | "view" => self.ctl_story(cmd, request, cx),
            "record" => return self.ctl_record(request, reply, cx),
            "symbol" | "placeholder" | "hold" => return self.ctl_symbol(cmd, request, reply, cx),
            "ui" | "click" | "hover" | "drag" | "scroll" | "key" | "type" => {
                return self.ctl_input(cmd, request, reply, window, cx);
            }
            "who" => json!({
                "ok": true,
                "project": self.doc.path(),
                "name": self.doc.name(),
                "pid": std::process::id(),
            }),
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
            "text" | "color" => {
                let generator = if cmd == "text" {
                    let mut title = splice_core::model::Title {
                        text: text("text").unwrap_or("Title").to_string(),
                        ..Default::default()
                    };
                    if let Err(e) = self.apply_style(&mut title, request) {
                        return Some(fail(e));
                    }
                    Generator::Text(title)
                } else {
                    match text("color").map(parse_color) {
                        Some(Some(color)) => Generator::Color { color },
                        _ => {
                            return Some(fail(
                                "`color` is #rrggbb, #rrggbbaa or a name (black, white, paper, ink…)",
                            ));
                        }
                    }
                };
                let at = match self.time(request, "at") {
                    Ok(t) => t.unwrap_or(self.player.position),
                    Err(e) => return Some(fail(e)),
                };
                let duration = match self.time(request, "duration") {
                    Ok(d) => d.unwrap_or(3 * SECOND),
                    Err(e) => return Some(fail(e)),
                };
                let track = self.track(request).unwrap_or_else(|| {
                    // Over whatever is there.
                    self.doc
                        .project
                        .tracks
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| t.clips.iter().any(|c| c.start < at + duration && c.end() > at))
                        .map(|(i, _)| i + 1)
                        .max()
                        .unwrap_or(0)
                });
                let mut look = splice_core::look::Look::default();
                if let Err(e) = apply_look(&mut look, request, 0) {
                    return Some(fail(e));
                }
                if !look.grade.is_none() {
                    return Some(fail(
                        "colour (--grade, --exposure…) is for video and pictures; a title or colour clip takes its own --color",
                    ));
                }
                let id = self.edit(cx, |p| {
                    let id = p.insert_generated(generator, track, at, duration);
                    p.set_look(id, |l| *l = look);
                    id
                });
                self.edited_with(json!({ "clip": self.clip_json(id) }))
            }
            "style" => {
                let ids = match self.clips(request) {
                    Ok(ids) => ids,
                    Err(e) => return Some(fail(e)),
                };
                let mut titles = Vec::new();
                for &id in &ids {
                    match self.doc.project.clip(id).and_then(|c| c.generator.clone()) {
                        Some(Generator::Text(mut title)) => {
                            if let Some(words) = text("text") {
                                title.text = words.to_string();
                            }
                            if let Err(e) = self.apply_style(&mut title, request) {
                                return Some(fail(e));
                            }
                            titles.push((id, Generator::Text(title)));
                        }
                        Some(Generator::Color { .. }) => match text("color").map(parse_color) {
                            Some(Some(color)) => titles.push((id, Generator::Color { color })),
                            _ => return Some(fail("give a colour clip a new `color`")),
                        },
                        None => {
                            return Some(fail(format!(
                                "clip {} is media; `style` is for text and colour clips",
                                id.0
                            )));
                        }
                    }
                }
                self.edit(cx, |p| {
                    for (id, generator) in titles {
                        p.set_generator(id, |g| *g = generator);
                    }
                });
                let clips: Vec<Value> = ids.iter().map(|id| self.clip_json(*id)).collect();
                self.edited_with(json!({ "clips": clips }))
            }
            "look" => {
                // `--all`: every video and picture, for one colour across the movie.
                let ids = if flag("all") == Some(true) {
                    let project = &self.doc.project;
                    project
                        .clips()
                        .filter(|(_, c)| c.generator.is_none())
                        .filter(|(_, c)| project.media(c.media).is_some_and(|m| m.is_visual()))
                        .map(|(_, c)| c.id)
                        .collect()
                } else {
                    match self.clips(request) {
                        Ok(ids) => ids,
                        Err(e) => return Some(fail(e)),
                    }
                };
                let mut looks = Vec::new();
                for &id in &ids {
                    let Some(clip) = self.doc.project.clip(id) else {
                        continue;
                    };
                    let mut look = clip.look.clone();
                    let at = (self.player.position - clip.start).clamp(0, clip.duration);
                    if let Err(e) = apply_look(&mut look, request, at) {
                        return Some(fail(e));
                    }
                    if look.key.is_some() && clip.generator.is_some() {
                        return Some(fail("a green-screen key is for video and pictures, not text or colour"));
                    }
                    let pictures_only = look.backdrop != clip.look.backdrop
                        || look.redact != clip.look.redact
                        || look.fx.blur != clip.look.fx.blur
                        || look.fx.vignette != clip.look.fx.vignette
                        || look.fx.grain != clip.look.fx.grain;
                    if pictures_only && clip.generator.is_some() {
                        return Some(fail(
                            "blur, vignette, grain, --backdrop and --redact are for video and pictures; a title or colour \
                             takes --shake, --glitch and the rest",
                        ));
                    }
                    if look.grade != clip.look.grade && clip.generator.is_some() {
                        return Some(fail(
                            "colour (--grade, --exposure…) is for video and pictures; a title or colour clip takes its own --color",
                        ));
                    }
                    looks.push((id, look));
                }
                self.edit(cx, |p| {
                    for (id, look) in looks {
                        p.set_look(id, |l| *l = look);
                    }
                });
                let clips: Vec<Value> = ids.iter().map(|id| self.clip_json(*id)).collect();
                self.edited_with(json!({ "clips": clips }))
            }
            "fonts" => {
                let query = text("query").unwrap_or("");
                let fonts = Self::font_matches(&titles::families(), query);
                json!({ "ok": true, "count": fonts.len(), "fonts": fonts })
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
                // Black bars: a shape (2.39, 1.85, 2:1, 16:9) or none.
                let letterbox = match text("letterbox") {
                    None => self.doc.project.letterbox,
                    Some("none" | "off") => None,
                    Some(shape) => {
                        let parsed = match shape.split_once(':') {
                            Some((w, h)) => w
                                .trim()
                                .parse::<f32>()
                                .ok()
                                .zip(h.trim().parse::<f32>().ok())
                                .map(|(w, h)| w / h),
                            None => shape.trim().parse::<f32>().ok(),
                        };
                        match parsed.filter(|a| (0.3..=4.0).contains(a)) {
                            Some(a) => Some(a),
                            None => return Some(fail("`letterbox` is a shape like 2.39, 1.85 or 2:1, or none")),
                        }
                    }
                };
                self.edit(cx, |p| {
                    p.settings = settings;
                    p.letterbox = letterbox;
                });
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
                let plan = export::plan_still(&self.doc.project, &self.resolver(), at, &path);
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
                // Just a stretch: --from/--to, a clip's span, or a beat.
                let range = if let Ok(Some(clips)) = self.clips_of(request).map(|c| c.first().copied()) {
                    self.doc.project.clip(clips).map(|c| (c.start, c.end()))
                } else if let Some(beat) = request.get("beat").and_then(Value::as_u64) {
                    match self.doc.project.story.span(splice_core::story::BeatId(beat)) {
                        Some(span) => Some(span),
                        None => return Some(fail(format!("no beat {beat}"))),
                    }
                } else {
                    match (self.time(request, "from"), self.time(request, "to")) {
                        (Ok(None), Ok(None)) => None,
                        (Ok(a), Ok(b)) => {
                            let (a, b) = (a.unwrap_or(0), b.unwrap_or(self.doc.project.duration()));
                            if b <= a {
                                return Some(fail("--to must be after --from"));
                            }
                            Some((a, b))
                        }
                        (Err(e), _) | (_, Err(e)) => return Some(fail(e)),
                    }
                };
                if let Err(e) = self.start_export(path.clone(), quality, false, range, cx) {
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

    /// Text style fields from a request, onto `title`.
    fn apply_style(&self, title: &mut splice_core::model::Title, request: &Value) -> Result<(), String> {
        let text = |key: &str| request.get(key).and_then(Value::as_str);
        let flag = |key: &str| request.get(key).and_then(Value::as_bool);
        if let Some(font) = text("font") {
            let families = titles::families();
            match families.iter().find(|f| f.eq_ignore_ascii_case(font)) {
                Some(found) => title.font = found.clone(),
                None => {
                    return Err(format!(
                        "no font named `{font}` here; `splice ctl fonts --query {font}` searches"
                    ));
                }
            }
        }
        if let Some(size) = request.get("size").and_then(Value::as_f64) {
            // Pixels on the canvas, or a fraction of its height.
            let h = self.doc.project.settings.height as f64;
            title.size = if size > 1.0 { size / h } else { size }.clamp(0.005, 0.6) as f32;
        }
        if let Some(c) = text("color") {
            title.color = parse_color(c).ok_or(format!("`color` {c} is not #rrggbb or a name"))?;
        }
        match text("box") {
            Some("none") => title.background = None,
            Some(c) => title.background = Some(parse_color(c).ok_or(format!("`box` {c} is not #rrggbbaa or a name"))?),
            None => {}
        }
        if let Some(align) = text("align") {
            title.align = match align {
                "left" => splice_core::model::Align::Left,
                "center" | "centre" => splice_core::model::Align::Center,
                "right" => splice_core::model::Align::Right,
                _ => return Err("`align` is left, center or right".into()),
            };
        }
        title.bold = flag("bold").unwrap_or(title.bold);
        title.italic = flag("italic").unwrap_or(title.italic);
        title.shadow = flag("shadow").unwrap_or(title.shadow);
        // Coming in a piece at a time.
        use splice_core::textmotion::{By, Style, TextMotion};
        match request.get("animate").and_then(Value::as_str) {
            Some("none" | "off") => title.animate = None,
            Some(name) => {
                let style = Style::parse(name).ok_or_else(|| {
                    let names: Vec<_> = Style::ALL.iter().map(|s| s.name()).collect();
                    format!("`animate` is none or one of {}", names.join(", "))
                })?;
                title.animate = Some(TextMotion {
                    style,
                    ..title.animate.unwrap_or_default()
                });
            }
            None => {}
        }
        if let Some(m) = &mut title.animate {
            if let Some(name) = request.get("by").and_then(Value::as_str) {
                m.by = By::parse(name).ok_or("`by` is word, letter or line")?;
            }
            if let Some(over) = request.get("over").and_then(Value::as_f64) {
                m.over = splice_core::time::from_secs(over.clamp(0.05, 30.0));
            }
        }
        Ok(())
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
            "editing": self.workbench.as_ref().map(|b| self.editing_json(b.media)),
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

    pub(super) fn edited_with(&self, mut extra: Value) -> Value {
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
    pub(super) fn clip_json(&self, id: ClipId) -> Value {
        let project = &self.doc.project;
        let (Some(slot), Some(clip)) = (project.slot(id), project.clip(id)) else {
            return json!({ "id": id.0, "gone": true });
        };
        let kind = match &clip.generator {
            Some(Generator::Text(_)) => "text",
            Some(Generator::Color { .. }) => "color",
            None => project.media(clip.media).map_or("missing", |m| kind(m.kind)),
        };
        let mut value = json!({
            "id": clip.id.0,
            "name": project.clip_name(clip),
            "kind": kind,
            "track": slot.track + 1,
            "start": s(clip.start),
            "end": s(clip.end()),
            "duration": s(clip.duration),
        });
        if let Some(label) = &clip.placeholder {
            value["placeholder"] = json!(label);
        }
        match &clip.generator {
            None => {
                value["media"] = json!(clip.media.0);
                value["source_in"] = json!(s(clip.source_in));
                value["volume"] = json!(clip.volume);
            }
            Some(Generator::Text(title)) => {
                let h = project.settings.height as f32;
                value["text"] = json!({
                    "text": title.text,
                    "font": title.font,
                    "size": (title.size * h).round(),
                    "bold": title.bold,
                    "italic": title.italic,
                    "color": hex(title.color),
                    "align": title.align,
                    "box": title.background.map(hex),
                    "shadow": title.shadow,
                    "animate": title.animate.map(|m| json!({
                        "style": m.style.name(),
                        "by": m.by.name(),
                        "over": s(m.over),
                    })),
                    "timed": !title.timing.is_empty(),
                });
            }
            Some(Generator::Color { color }) => value["color"] = json!(hex(*color)),
        }
        if clip.look != splice_core::look::Look::default() {
            value["look"] = look_json(&clip.look);
        }
        if clip.frozen {
            value["frozen"] = json!(true);
        }
        if let Some(m) = project
            .media(clip.media)
            .filter(|m| !m.clicks.is_empty() && clip.generator.is_none())
        {
            value["clicks"] = json!(m.clicks.len());
        }
        if clip.speed != 1.0 {
            value["speed"] = json!(clip.speed);
        }
        if clip.reverse {
            value["reverse"] = json!(true);
        }
        if let Some(cut) = project.cut_into(clip.id) {
            value["transition"] = json!({
                "kind": cut.kind.name(),
                "duration": s(cut.duration()),
                "from": cut.from.map(|c| c.0),
                "start": s(cut.start),
                "end": s(cut.end),
            });
        }
        value
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
            "canvas": {
                "width": settings.width,
                "height": settings.height,
                "fps": settings.fps,
                "letterbox": self.doc.project.letterbox,
            },
            "duration": s(project.duration()),
            "playhead": s(self.player.position),
            "playing": self.player.is_playing(),
            "playback": self.player.keeping,
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
                    "made": m.recipe.as_ref().map(|r| r.kind()),
                    "rendering": self.render_of(m.id).map(|r| r.progress()),
                    "render_error": self.made.errors.get(&m.id),
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
    pub(super) fn time(&self, request: &Value, key: &str) -> Result<Option<Time>, String> {
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
    pub(super) fn track(&self, request: &Value) -> Option<usize> {
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

    /// The clips a request names, or none when it names none.
    pub(super) fn clips_of(&self, request: &Value) -> Result<Vec<ClipId>, String> {
        if request.get("clips").is_none() {
            return Ok(Vec::new());
        }
        self.clips(request)
    }

    pub(super) fn find_media(&self, spec: &Value) -> Result<MediaId, String> {
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

/// Look fields from a request, onto `look`.
/// `at` is clip time, where a keyframed value is keyed.
fn apply_look(look: &mut splice_core::look::Look, request: &Value, at: Time) -> Result<(), String> {
    use splice_core::look::{Drift, Key, Mask, MotionKind, Shape};
    let text = |key: &str| request.get(key).and_then(Value::as_str);
    let number = |key: &str| request.get(key).and_then(Value::as_f64).map(|v| v as f32);
    let flag = |key: &str| request.get(key).and_then(Value::as_bool);
    // A keyframed value takes a keyframe at `at`; the rest change throughout.
    use splice_core::look::Prop;
    for prop in Prop::ALL {
        if let Some(v) = number(prop.name()) {
            look.set_value(prop, at, v);
        }
    }
    for (key, motion) in [("enter", &mut look.enter), ("exit", &mut look.exit)] {
        if let Some(kind) = text(key) {
            motion.kind = MotionKind::parse(kind).ok_or_else(|| {
                let names: Vec<_> = MotionKind::ALL.iter().map(|k| k.name()).collect();
                format!("`{key}` is one of {}", names.join(", "))
            })?;
        }
        if let Some(d) = number(&format!("{key}_for")) {
            motion.duration = from_secs(d.clamp(0.05, 10.0) as f64);
        }
    }
    if let Some(drift) = text("drift") {
        look.drift = match drift {
            "none" => Drift::None,
            "zoom-in" => Drift::ZoomIn,
            "zoom-out" => Drift::ZoomOut,
            _ => return Err("`drift` is none, zoom-in or zoom-out".into()),
        };
    }
    match text("key") {
        Some("none") => look.key = None,
        Some(color) => {
            let rgba = parse_color(color).ok_or(format!("`key` {color} is green, blue, #rrggbb or none"))?;
            look.key = Some(Key {
                color: rgba >> 8,
                ..look.key.unwrap_or_default()
            });
        }
        None => {}
    }
    if let Some(key) = &mut look.key {
        key.similarity = number("similarity").map_or(key.similarity, |v| v.clamp(0.01, 0.6));
        key.blend = number("blend").map_or(key.blend, |v| v.clamp(0.0, 0.5));
        key.despill = flag("despill").unwrap_or(key.despill);
    }
    match text("mask") {
        Some("none") => look.mask = None,
        Some(shape) => {
            let shape = match shape {
                "ellipse" | "circle" => Shape::Ellipse,
                "rect" | "box" => Shape::Rect,
                _ => return Err("`mask` is ellipse, rect or none".into()),
            };
            look.mask = Some(Mask {
                shape,
                ..look.mask.unwrap_or_default()
            });
        }
        None => {}
    }
    if let Some(mask) = &mut look.mask {
        mask.x = number("mask_x").unwrap_or(mask.x);
        mask.y = number("mask_y").unwrap_or(mask.y);
        mask.width = number("mask_width").map_or(mask.width, |v| v.max(0.01));
        mask.height = number("mask_height").map_or(mask.height, |v| v.max(0.01));
        mask.feather = number("feather").map_or(mask.feather, |v| v.clamp(0.001, 1.0));
        mask.invert = flag("invert").unwrap_or(mask.invert);
    }
    if let Some(name) = text("grade") {
        look.grade.preset = splice_core::look::Preset::parse(name).ok_or_else(|| {
            let names: Vec<_> = splice_core::look::Preset::ALL.iter().map(|p| p.name()).collect();
            format!("`grade` is one of {}", names.join(", "))
        })?;
    }
    let grade = &mut look.grade;
    grade.exposure = number("exposure").unwrap_or(grade.exposure);
    grade.contrast = number("contrast").unwrap_or(grade.contrast);
    grade.saturation = number("saturation").unwrap_or(grade.saturation);
    grade.temperature = number("warmth").unwrap_or(grade.temperature);
    grade.tint = number("tint").unwrap_or(grade.tint);
    *grade = grade.clamped();
    let fx = &mut look.fx;
    fx.blur = number("blur").unwrap_or(fx.blur);
    fx.vignette = number("vignette").unwrap_or(fx.vignette);
    fx.grain = number("grain").unwrap_or(fx.grain);
    fx.shake = number("shake").unwrap_or(fx.shake);
    fx.glitch = number("glitch").unwrap_or(fx.glitch);
    *fx = fx.clamped();
    match text("backdrop") {
        Some("on" | "yes" | "blur") => look.backdrop = Some(look.backdrop.unwrap_or_default()),
        Some("off" | "no" | "none") => look.backdrop = None,
        Some(other) => return Err(format!("`backdrop` is on or off, not {other}")),
        None => {}
    }
    if let Some(b) = &mut look.backdrop {
        b.blur = number("backdrop_blur").map_or(b.blur, |v| v.clamp(0.0, 1.0));
        b.dim = number("backdrop_dim").map_or(b.dim, |v| v.clamp(0.0, 1.0));
    }
    match text("redact") {
        Some("none" | "off") => look.redact = None,
        Some(name) => {
            let style = splice_core::look::RedactStyle::parse(name)
                .ok_or(format!("`redact` is blur, pixelate, black or none, not {name}"))?;
            look.redact = Some(splice_core::look::Redact {
                style,
                ..look.redact.unwrap_or_default()
            });
        }
        None => {}
    }
    if let Some(r) = &mut look.redact {
        r.x = number("redact_x").map_or(r.x, |v| v.clamp(0.0, 1.0));
        r.y = number("redact_y").map_or(r.y, |v| v.clamp(0.0, 1.0));
        r.width = number("redact_width").map_or(r.width, |v| v.clamp(0.02, 1.0));
        r.height = number("redact_height").map_or(r.height, |v| v.clamp(0.02, 1.0));
    }
    if text("crop") == Some("none") {
        look.crop = None;
    }
    let edges = ["crop_left", "crop_top", "crop_right", "crop_bottom"];
    if edges.iter().any(|k| number(k).is_some()) {
        let mut c = look.crop.unwrap_or_default();
        c.left = number("crop_left").unwrap_or(c.left);
        c.top = number("crop_top").unwrap_or(c.top);
        c.right = number("crop_right").unwrap_or(c.right);
        c.bottom = number("crop_bottom").unwrap_or(c.bottom);
        let c = c.clamped();
        look.crop = (!c.is_none()).then_some(c);
    }
    Ok(())
}
