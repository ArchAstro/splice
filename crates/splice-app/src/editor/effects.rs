//! Commands for transitions, speed and keyframes: what the menu and keys do
//! beyond the inspector's own controls.

use super::*;
use crate::actions::{AddCaptions, AddKeyframe, AddTransition, FreezeFrame, ReverseClip};
use splice_core::look::{Ease, Prop};
use splice_core::model::Clip;
use splice_core::transition::{Kind, Transition};

/// How long a frozen frame holds when first made.
const HOLD: Time = 2 * SECOND;

impl Editor {
    /// The clip a playhead command works on: the selected one under the
    /// playhead, or else the topmost picture there.
    fn clip_at_playhead(&self, want: impl Fn(&Clip) -> bool) -> Option<ClipId> {
        let project = &self.doc.project;
        let t = self.player.position;
        self.selection
            .iter()
            .filter_map(|id| project.clip(*id))
            .find(|c| c.contains(t) && want(c))
            .or_else(|| {
                project
                    .visual_stack(t)
                    .into_iter()
                    .rev()
                    .map(|(c, _)| c)
                    .find(|c| want(c))
            })
            .map(|c| c.id)
    }

    /// Holds the frame at the playhead for two seconds.
    pub(super) fn freeze_frame_at_playhead(&mut self, cx: &mut Context<Self>) {
        let t = self.player.position;
        let video = |c: &Clip| {
            c.generator.is_none()
                && !c.frozen
                && self
                    .doc
                    .project
                    .media(c.media)
                    .is_some_and(|m| m.kind == MediaKind::Video)
        };
        let Some(id) = self.clip_at_playhead(video) else {
            return self.say_error("Put the playhead over a video clip to hold its frame");
        };
        let held = self.edit(cx, |p| p.freeze_frame(id, t, HOLD));
        match held {
            Some(held) => {
                self.selection = vec![held];
                self.say("Holding this frame for 2s; drag its end to hold it longer");
            }
            None => self.say_error("That frame could not be held"),
        }
    }

    pub(super) fn freeze_frame_action(&mut self, _: &FreezeFrame, _: &mut Window, cx: &mut Context<Self>) {
        self.freeze_frame_at_playhead(cx);
    }

    /// A dissolve into each selected clip, or else at the cut nearest the playhead.
    pub(super) fn add_transition_action(&mut self, _: &AddTransition, _: &mut Window, cx: &mut Context<Self>) {
        let project = &self.doc.project;
        let visual = |id: ClipId| {
            project
                .clip(id)
                .and_then(|c| project.source(c))
                .is_some_and(|s| s.is_visual())
        };
        let mut targets: Vec<ClipId> = self
            .selection
            .iter()
            .copied()
            .filter(|id| visual(*id) && project.clip(*id).is_some_and(|c| c.transition.is_none()))
            .collect();
        if targets.is_empty() {
            // The nearest cut between two clips, on a visible track.
            let t = self.player.position;
            let nearest = project
                .tracks
                .iter()
                .filter(|lane| !lane.hidden)
                .flat_map(|lane| lane.clips.iter())
                .filter(|c| project.clip_before(c.id).is_some() && visual(c.id))
                .min_by_key(|c| (c.start - t).abs());
            match nearest {
                Some(c) if c.transition.is_none() => targets.push(c.id),
                Some(c) => {
                    self.selection = vec![c.id];
                    self.folds.insert((c.id, "Transition"), true);
                    self.layout.inspector_open = true;
                    self.say("That cut has a transition already");
                    return cx.notify();
                }
                None => return self.say_error("No cut between two clips to put a transition on"),
            }
        }
        self.edit(cx, |p| {
            p.set_transition(&targets, Some(Transition::new(Kind::Dissolve)))
        });
        self.selection = targets.clone();
        for id in &targets {
            self.folds.insert((*id, "Transition"), true);
        }
        self.layout.inspector_open = true;
        self.say(match targets.len() {
            1 => "Dissolve added; choose another in the inspector".to_string(),
            n => format!("Dissolves into {n} clips"),
        });
    }

    /// Plays the selected clips backwards, or forwards again.
    pub(super) fn reverse_action(&mut self, _: &ReverseClip, _: &mut Window, cx: &mut Context<Self>) {
        let project = &self.doc.project;
        let ids: Vec<ClipId> = self
            .selection
            .iter()
            .copied()
            .filter(|id| {
                project
                    .clip(*id)
                    .filter(|c| c.generator.is_none() && !c.frozen)
                    .and_then(|c| project.media(c.media))
                    .is_some_and(|m| m.kind == MediaKind::Video || m.kind == MediaKind::Audio)
            })
            .collect();
        if ids.is_empty() {
            return self.say_error("Select a video or sound clip to play it backwards");
        }
        let back = !ids.iter().all(|id| project.clip(*id).is_some_and(|c| c.reverse));
        self.edit(cx, |p| p.set_reverse(&ids, back));
        self.say(if back { "Playing backwards" } else { "Playing forwards" });
    }

    /// Keyframes where and how big the selected picture is, at the playhead.
    pub(super) fn add_keyframe_action(&mut self, _: &AddKeyframe, _: &mut Window, cx: &mut Context<Self>) {
        let visual = |c: &Clip| self.doc.project.source(c).is_some_and(|s| s.is_visual());
        let Some(id) = self.clip_at_playhead(visual) else {
            return self.say_error("Put the playhead over a picture to keyframe it");
        };
        let Some(clip) = self.doc.project.clip(id) else { return };
        let at = self.clip_time(clip);
        self.edit(cx, |p| {
            p.set_look(id, |look| {
                for prop in [Prop::X, Prop::Y, Prop::Scale] {
                    let v = look.value(prop, at);
                    look.keys.set(prop, at, v, None);
                }
            });
        });
        self.selection = vec![id];
        self.layout.inspector_open = true;
        self.say("Keyframed its place and size here; move the playhead and drag it to animate");
    }
}

impl Editor {
    /// `splice ctl transition | speed | freeze | keyframe`.
    pub(super) fn ctl_effects(
        &mut self,
        cmd: &str,
        request: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        use crate::ctl::fail;
        use serde_json::{Value, json};
        let text = |key: &str| request.get(key).and_then(Value::as_str);
        let number = |key: &str| request.get(key).and_then(Value::as_f64);
        let flag = |key: &str| request.get(key).and_then(Value::as_bool) == Some(true);
        let named = match self.clips_of(request) {
            Ok(ids) => ids,
            Err(e) => return fail(e),
        };
        let reply = |this: &Self, ids: &[ClipId]| {
            let clips: Vec<Value> = ids.iter().map(|id| this.clip_json(*id)).collect();
            this.edited_with(json!({ "clips": clips }))
        };
        match cmd {
            "transition" => {
                let transition = if flag("none") {
                    None
                } else {
                    let Some(name) = text("kind") else {
                        return fail(
                            "say which with --kind (dissolve, dip-black, wipe-left, push-left, zoom…) or --none",
                        );
                    };
                    let Some(kind) = Kind::parse(name) else {
                        let names: Vec<_> = Kind::ALL.iter().map(|k| k.name()).collect();
                        return fail(format!("`kind` is one of {}", names.join(", ")));
                    };
                    let duration = number("for").map_or(Transition::DEFAULT_LENGTH, splice_core::time::from_secs);
                    Some(Transition { kind, duration })
                };
                let project = &self.doc.project;
                let ids: Vec<ClipId> = if flag("all") {
                    let track = self.track(request);
                    project
                        .tracks
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| track.is_none_or(|t| t == *i))
                        .flat_map(|(_, lane)| lane.clips.iter())
                        .filter(|c| project.clip_before(c.id).is_some())
                        .map(|c| c.id)
                        .collect()
                } else {
                    named
                };
                if ids.is_empty() {
                    return fail(if flag("all") {
                        "no cuts between two clips there"
                    } else {
                        "name the clip the transition leads into with --clip (or use --all)"
                    });
                }
                let pictures: Vec<ClipId> = ids
                    .iter()
                    .copied()
                    .filter(|id| {
                        project
                            .clip(*id)
                            .and_then(|c| project.source(c))
                            .is_some_and(|s| s.is_visual())
                    })
                    .collect();
                if pictures.is_empty() {
                    return fail("transitions are for pictures: video, images, titles, colours, scenes");
                }
                self.edit(cx, |p| p.set_transition(&pictures, transition));
                reply(self, &pictures)
            }
            "speed" => {
                if named.is_empty() {
                    return fail("name clips with --clip");
                }
                let project = &self.doc.project;
                for id in &named {
                    let clip = project.clip(*id).expect("named clips exist");
                    let ok = clip.generator.is_none()
                        && project.media(clip.media).is_some_and(|m| {
                            m.is_timed() && !matches!(m.recipe, Some(splice_core::model::Recipe::Symbol(_)))
                        });
                    if !ok {
                        return fail(format!(
                            "clip {} has no speed: only video and sound play faster or slower (a symbol stretches: `symbol length`)",
                            id.0
                        ));
                    }
                    if clip.frozen {
                        return fail(format!("clip {} is a held frame; drag or trim it to hold longer", id.0));
                    }
                }
                let rate = number("rate");
                if let Some(r) = rate
                    && !(splice_core::model::SLOWEST..=splice_core::model::FASTEST).contains(&r)
                {
                    return fail(format!(
                        "`rate` is from {} to {}",
                        splice_core::model::SLOWEST,
                        splice_core::model::FASTEST
                    ));
                }
                let reverse = request.get("reverse").and_then(Value::as_bool);
                if rate.is_none() && reverse.is_none() {
                    return fail("say how: --rate R and/or --reverse | --forwards");
                }
                self.edit(cx, |p| {
                    if let Some(r) = rate {
                        p.set_speed(&named, r);
                    }
                    if let Some(back) = reverse {
                        p.set_reverse(&named, back);
                    }
                });
                reply(self, &named)
            }
            "freeze" => {
                let [id] = named[..] else {
                    return fail("name one video clip with --clip");
                };
                let at = match self.time(request, "at") {
                    Ok(t) => t.unwrap_or(self.player.position),
                    Err(e) => return fail(e),
                };
                let hold = number("for").map_or(HOLD, splice_core::time::from_secs);
                match self.edit(cx, |p| p.freeze_frame(id, at, hold)) {
                    Some(held) => reply(self, &[held]),
                    None => fail("hold a frame of a video clip, at a time inside it"),
                }
            }
            "keyframe" => {
                let [id] = named[..] else {
                    return fail("name one clip with --clip");
                };
                let props: Vec<Prop> = match text("prop") {
                    Some(name) => match Prop::parse(name) {
                        Some(p) => vec![p],
                        None => return fail("`prop` is x, y, scale or opacity"),
                    },
                    None => Prop::ALL.to_vec(),
                };
                let Some(clip) = self.doc.project.clip(id).cloned() else {
                    return fail("there is no such clip");
                };
                if flag("clear") {
                    let at = self.clip_time(&clip);
                    self.edit(cx, |p| {
                        p.set_look(id, |look| {
                            for prop in &props {
                                let v = look.value(*prop, at);
                                look.keys.of_mut(*prop).clear();
                                look.set_value(*prop, at, v);
                            }
                        })
                    });
                    return reply(self, &[id]);
                }
                let at = match self.time(request, "at") {
                    Ok(Some(t)) => t - clip.start,
                    Ok(None) => return fail("say when with --at T (timeline time)"),
                    Err(e) => return fail(e),
                };
                if at < 0 || at > clip.duration {
                    return fail(format!(
                        "--at is outside clip {} ({:.2}s to {:.2}s)",
                        id.0,
                        splice_core::time::secs(clip.start),
                        splice_core::time::secs(clip.end())
                    ));
                }
                if flag("drop") {
                    let mut gone = 0;
                    self.edit(cx, |p| {
                        p.set_look(id, |look| {
                            for prop in &props {
                                let v = look.value(*prop, at);
                                if look.keys.remove(*prop, at) {
                                    gone += 1;
                                    if look.keys.of(*prop).is_empty() {
                                        look.set_value(*prop, at, v);
                                    }
                                }
                            }
                        })
                    });
                    if gone == 0 {
                        return fail("no keyframe there");
                    }
                    return reply(self, &[id]);
                }
                let ease = match text("ease") {
                    Some(name) => match Ease::parse(name) {
                        Some(e) => Some(e),
                        None => return fail("`ease` is linear, in, out, in-out or hold"),
                    },
                    None => None,
                };
                let values: Vec<(Prop, f32)> = Prop::ALL
                    .iter()
                    .filter_map(|p| number(p.name()).map(|v| (*p, v as f32)))
                    .collect();
                if values.is_empty() && ease.is_none() {
                    return fail("give values to keyframe: --x, --y, --scale, --opacity (and --ease)");
                }
                self.edit(cx, |p| {
                    p.set_look(id, |look| {
                        for (prop, v) in &values {
                            look.keys.set(*prop, at, *v, ease);
                        }
                        // An ease alone changes the keyframes already there.
                        if values.is_empty()
                            && let Some(e) = ease
                        {
                            for prop in &props {
                                for k in look.keys.of_mut(*prop) {
                                    if (k.at - at).abs() <= splice_core::look::KEY_NEAR {
                                        k.ease = e;
                                    }
                                }
                            }
                        }
                    })
                });
                reply(self, &[id])
            }
            "zoom" => {
                let [id] = named[..] else {
                    return fail("name one screen recording clip with --clip");
                };
                if flag("none") {
                    self.edit(cx, |p| p.clear_zoom(id));
                    return reply(self, &[id]);
                }
                let zoom = number("to").unwrap_or(2.0) as f32;
                match self.edit(cx, |p| p.zoom_to_clicks(id, zoom)) {
                    0 => fail(
                        "no clicks noted in the stretch of recording this clip plays (record with Splice to note them)",
                    ),
                    _ => reply(self, &[id]),
                }
            }
            "leak" => {
                let names: Vec<&str> = splice_core::leaks::LEAKS.iter().map(|l| l.name).collect();
                let Some(leak) = text("kind").and_then(splice_core::leaks::find) else {
                    return fail(format!("say which with --kind: {}", names.join(", ")));
                };
                let at = match self.time(request, "at") {
                    Ok(t) => t,
                    Err(e) => return fail(e),
                };
                match self.add_leak(leak, at, cx) {
                    Some(clip) => reply(self, &[clip]),
                    None => fail("the light leak could not be placed"),
                }
            }
            _ => fail(format!("unknown command {cmd}")),
        }
    }
}

impl Editor {
    /// Hears what is said in clip `id` and lays captions over it, on a new
    /// track: in the background, downloading the speech model first if it
    /// is not here. `reply` (a `splice ctl … --wait`) hears how it went.
    pub(super) fn caption_clip(
        &mut self,
        id: ClipId,
        layout: splice_core::captions::Layout,
        reply: Option<std::sync::mpsc::Sender<String>>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let project = &self.doc.project;
        let clip = project.clip(id).ok_or("there is no such clip")?.clone();
        let media = project
            .media(clip.media)
            .filter(|m| m.has_audio && clip.generator.is_none())
            .ok_or("that clip has no sound to caption")?;
        if clip.reverse || clip.frozen {
            return Err("a clip playing backwards or held still has no speech to caption".into());
        }
        if crate::speech::whisper().is_none() {
            return Err(crate::speech::INSTALL.into());
        }
        let path = self
            .library
            .path(&media.hash)
            .ok_or("its media is not ready yet")?
            .to_path_buf();
        let (from, span, start, speed) = (clip.source_in, clip.span(), clip.start, clip.speed);
        self.say(if crate::speech::has_model() {
            "Listening for words…".to_string()
        } else {
            format!(
                "Downloading the speech model ({} MB, once), then listening…",
                crate::speech::MODEL_SIZE_MB
            )
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let heard = cx
                .background_spawn(async move { crate::speech::transcribe(&path, from, span) })
                .await;
            this.update(cx, |editor, cx| {
                let answer = match heard {
                    Ok(heard) if heard.is_empty() => {
                        editor.say_error("No speech heard in that clip");
                        serde_json::json!({ "ok": false, "error": "no speech heard" })
                    }
                    Ok(heard) => {
                        // From source time to timeline time, at the clip's speed.
                        let at = |t: Time| start + (t as f64 / speed).round() as Time;
                        let words: Vec<splice_core::captions::Word> = heard
                            .into_iter()
                            .map(|w| splice_core::captions::Word {
                                text: w.text,
                                start: at(w.start),
                                end: at(w.end),
                            })
                            .collect();
                        let ids = editor.edit(cx, |p| p.add_captions(&words, &layout));
                        editor.selection = ids.clone();
                        editor.say(format!(
                            "{} captions from {} words; each is a title you can edit",
                            ids.len(),
                            words.len()
                        ));
                        let clips: Vec<_> = ids.iter().map(|id| editor.clip_json(*id)).collect();
                        editor.edited_with(serde_json::json!({ "clips": clips, "words": words.len() }))
                    }
                    Err(e) => {
                        editor.say_error(format!("Captions: {e}"));
                        serde_json::json!({ "ok": false, "error": e.to_string() })
                    }
                };
                if let Some(reply) = reply {
                    let _ = reply.send(answer.to_string());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Ok(())
    }

    /// Captions the selected clip (or the first with sound under the playhead).
    pub(super) fn add_captions_action(&mut self, _: &AddCaptions, _: &mut Window, cx: &mut Context<Self>) {
        let project = &self.doc.project;
        let sounds = |c: &Clip| c.generator.is_none() && project.media(c.media).is_some_and(|m| m.has_audio);
        let t = self.player.position;
        let target = self
            .selection
            .iter()
            .filter_map(|id| project.clip(*id))
            .find(|c| sounds(c))
            .or_else(|| project.clips().map(|(_, c)| c).find(|c| c.contains(t) && sounds(c)))
            .map(|c| c.id);
        let Some(id) = target else {
            return self.say_error("Select a clip with speech in it to caption");
        };
        if let Err(e) = self.caption_clip(id, Default::default(), None, cx) {
            self.say_error(e);
        }
    }
}

impl Editor {
    /// `splice ctl captions`: from speech (`--clip`, answered when heard with
    /// `--wait`) or from a subtitle file (`--file`).
    pub(super) fn ctl_captions(
        &mut self,
        request: &serde_json::Value,
        reply: std::sync::mpsc::Sender<String>,
        cx: &mut Context<Self>,
    ) -> Option<serde_json::Value> {
        use crate::ctl::fail;
        use serde_json::{Value, json};
        if let Some(text) = request.get("content").and_then(Value::as_str) {
            let cues = splice_core::captions::parse_subtitles(text);
            if cues.is_empty() {
                return Some(fail("no captions found: give an .srt or .vtt file"));
            }
            let ids = self.edit(cx, |p| p.add_subtitles(&cues));
            self.selection = ids.clone();
            let clips: Vec<Value> = ids.iter().map(|id| self.clip_json(*id)).collect();
            return Some(self.edited_with(json!({ "clips": clips })));
        }
        let ids = match self.clips_of(request) {
            Ok(ids) => ids,
            Err(e) => return Some(fail(e)),
        };
        let [id] = ids[..] else {
            return Some(fail(
                "name one clip with speech in it (--clip), or give --file captions.srt",
            ));
        };
        let mut layout = splice_core::captions::Layout::default();
        if let Some(n) = request.get("words").and_then(Value::as_f64) {
            layout.words = (n as usize).clamp(1, 20);
        }
        if request.get("plain").and_then(Value::as_bool) == Some(true) {
            layout.karaoke = false;
        }
        let wait = request.get("wait").and_then(Value::as_bool) == Some(true);
        match self.caption_clip(id, layout, wait.then_some(reply), cx) {
            Err(e) => Some(fail(e)),
            Ok(()) if wait => None,
            Ok(()) => {
                Some(json!({ "ok": true, "listening": true, "note": "captions arrive when heard; --wait waits" }))
            }
        }
    }
}
