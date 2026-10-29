//! The story: planning a movie before (and while) it is cut. Edit view keeps
//! the story as a lane over the timeline; Story view trades the viewer and
//! inspector for a board of beat cards, each saying what happens and what to
//! capture, showing a frame once footage is laid in, taking media dropped on
//! it, and recording straight into it.

use super::render::{caps, icon, panel, tip};
use super::*;
use gpui::{FontWeight, Hsla, ObjectFit, Stateful, img};
use splice_core::model::Source;
use splice_core::story::{BEAT_DURATION, BeatId, Coverage, CueId};

/// Which view the workspace shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Edit,
    Story,
    /// What the project records, live.
    Record,
}

/// Which words are being typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Title(BeatId),
    Story(BeatId),
    Capture(BeatId),
    Cue(CueId),
    Shot(splice_core::story::ShotId),
    /// A bible entry's name, and its continuity words.
    EntryName(splice_core::bible::EntryId),
    EntryWords(splice_core::bible::EntryId),
}

/// Typing into a beat or cue, and the project before the typing began.
pub struct Typing {
    pub target: Target,
    pub before: Project,
    /// Byte offset of the caret.
    pub cursor: usize,
    /// Everything is selected: the next keystroke replaces it.
    pub all: bool,
}

/// A beat card being dragged to a new place in the story.
#[derive(Clone)]
pub struct BeatDrag {
    pub beat: BeatId,
    pub title: SharedString,
}

impl Render for BeatDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(7.0))
            .bg(Theme::surface1())
            .border_1()
            .border_color(Theme::accent())
            .text_size(px(12.0))
            .text_color(Theme::text())
            .child(self.title.clone())
    }
}

fn rgb(color: u32) -> Hsla {
    gpui::rgb(color).into()
}

fn seconds(t: Time) -> String {
    let s = secs(t);
    if (s - s.round()).abs() < 0.05 {
        format!("{}s", s.round())
    } else {
        format!("{s:.1}s")
    }
}

impl Editor {
    pub(super) fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        if mode != Mode::Edit {
            self.workbench = None;
        }
        self.typing = None;
        // The sources are live while the Record tab is open.
        self.relive(cx);
        cx.notify();
    }

    /// Chooses a beat: the playhead goes to its start.
    pub(super) fn select_beat(&mut self, id: Option<BeatId>, cx: &mut Context<Self>) {
        self.beat = id;
        if let Some((start, _)) = id.and_then(|id| self.doc.project.story.span(id)) {
            let under = self.doc.project.story.beat_at(self.player.position).map(|b| b.id);
            if under != id {
                self.seek(start, cx);
            }
        }
        cx.notify();
    }

    /// A new beat after the chosen one (or at the end), named and ready to type.
    pub(super) fn new_beat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let after = self.beat;
        let n = self.doc.project.story.beats.len() + 1;
        let id = self.edit(cx, |p| p.add_beat(after, &format!("Beat {n}"), BEAT_DURATION));
        self.beat = Some(id);
        if self.mode == Mode::Story {
            self.begin_typing(Target::Title(id), true, window, cx);
        }
    }

    pub(super) fn new_cue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.player.position;
        let n = self.doc.project.story.cues.len() + 1;
        let id = self.edit(cx, |p| p.add_cue(at, &format!("Cue {n}")));
        if self.mode == Mode::Story {
            self.begin_typing(Target::Cue(id), true, window, cx);
        } else {
            self.say("Cue marked at the playhead; name it in Story view (⌘3)");
        }
    }

    /// The chosen beat's brief: its outline to the clipboard, and its
    /// references written to a folder shown in Finder, ready to attach.
    pub(super) fn copy_brief(&mut self, cx: &mut Context<Self>) {
        let Some(beat) = self.beat.filter(|b| self.doc.project.story.index(*b).is_some()) else {
            self.say("Choose a beat to brief");
            return cx.notify();
        };
        let dir = Self::brief_dir(beat);
        self.write_brief(beat, dir, cx, |editor, result, cx| match result {
            Ok(brief) => {
                let outline = brief["outline"].as_str().unwrap_or_default().to_string();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(outline));
                if let Some(dir) = brief["dir"].as_str() {
                    cx.reveal_path(Path::new(dir));
                }
                let n = brief["images"].as_array().map_or(0, Vec::len);
                editor.say(format!(
                    "Brief for “{}”: the prompt is copied; its {n} reference{} are in the folder, numbered as @Image",
                    brief["title"].as_str().unwrap_or(""),
                    if n == 1 { "" } else { "s" }
                ));
            }
            Err(e) => editor.say_error(format!("Could not brief it: {e}")),
        });
    }

    pub(super) fn copy_shot_list(&mut self, cx: &mut Context<Self>) {
        let list = self.doc.project.shot_list();
        if list.trim().is_empty() {
            self.say("The story is empty: add beats first");
        } else {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(list));
            self.say("Shot list copied");
        }
        cx.notify();
    }

    // Typing.

    fn text_of(&self, target: Target) -> Option<String> {
        let story = &self.doc.project.story;
        let beat = |id: BeatId| story.beats.iter().find(|b| b.id == id);
        Some(match target {
            Target::Title(id) => beat(id)?.title.clone(),
            Target::Story(id) => beat(id)?.story.clone(),
            Target::Capture(id) => beat(id)?.capture.clone(),
            Target::Cue(id) => story.cues.iter().find(|c| c.id == id)?.label.clone(),
            Target::Shot(id) => story
                .beats
                .iter()
                .flat_map(|b| &b.shots)
                .find(|s| s.id == id)?
                .note
                .clone(),
            Target::EntryName(id) => self.doc.project.bible.entry(id)?.name.clone(),
            Target::EntryWords(id) => self.doc.project.bible.entry(id)?.words.clone(),
        })
    }

    fn set_text(&mut self, target: Target, text: String) {
        let p = &mut self.doc.project;
        match target {
            Target::Title(id) => p.set_beat(id, |b| b.title = text),
            Target::Story(id) => p.set_beat(id, |b| b.story = text),
            Target::Capture(id) => p.set_beat(id, |b| b.capture = text),
            Target::Cue(id) => p.set_cue(id, |c| c.label = text),
            Target::Shot(id) => p.set_shot(id, |s| s.note = text),
            Target::EntryName(id) => p.set_entry(id, |e| e.name = text),
            Target::EntryWords(id) => p.set_entry(id, |e| e.words = text),
        }
    }

    pub(super) fn begin_typing(&mut self, target: Target, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.typing.as_mut().filter(|t| t.target == target) {
            // A second click of a double-click: take the whole thing.
            t.all |= all;
            window.focus(&self.story_focus);
            cx.notify();
            return;
        }
        self.end_typing(cx);
        let Some(text) = self.text_of(target) else { return };
        self.typing = Some(Typing {
            target,
            before: self.doc.project.clone(),
            cursor: text.len(),
            all,
        });
        if let Target::Title(id) | Target::Story(id) | Target::Capture(id) = target {
            self.beat = Some(id);
        }
        window.focus(&self.story_focus);
        cx.notify();
    }

    /// Ends typing: what was typed becomes one undo step.
    pub(super) fn end_typing(&mut self, cx: &mut Context<Self>) {
        if let Some(typing) = self.typing.take() {
            self.doc.commit(typing.before);
            cx.notify();
        }
    }

    fn with_typing(&mut self, f: impl FnOnce(&mut String, &mut Typing)) {
        let Some(mut typing) = self.typing.take() else { return };
        if let Some(mut text) = self.text_of(typing.target) {
            f(&mut text, &mut typing);
            typing.cursor = typing.cursor.min(text.len());
            let target = typing.target;
            self.set_text(target, text);
        }
        self.typing = Some(typing);
    }

    fn type_story(&mut self, typed: &str) {
        self.with_typing(|text, t| {
            if t.all {
                text.clear();
                t.cursor = 0;
                t.all = false;
            }
            let at = t.cursor.min(text.len());
            text.insert_str(at, typed);
            t.cursor = at + typed.len();
        });
    }

    pub(super) fn story_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.typing.as_ref().map(|t| t.target) else {
            return;
        };
        let k = &event.keystroke;
        let (cmd, alt) = (k.modifiers.platform, k.modifiers.alt);
        let prev = |text: &str, at: usize| text[..at].char_indices().next_back().map_or(0, |(i, _)| i);
        let next = |text: &str, at: usize| text[at..].chars().next().map_or(at, |c| at + c.len_utf8());
        let word_left = |text: &str, at: usize| {
            let trimmed = text[..at].trim_end();
            trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1)
        };
        let one_line = matches!(
            target,
            Target::Title(_) | Target::Cue(_) | Target::Shot(_) | Target::EntryName(_)
        );
        match k.key.as_str() {
            "escape" => {
                self.end_typing(cx);
                window.focus(&self.focus);
            }
            // A title ends with Return and moves on to the story; notes take lines.
            "enter" if one_line || cmd => {
                self.end_typing(cx);
                match target {
                    Target::Title(id) => self.begin_typing(Target::Story(id), false, window, cx),
                    Target::EntryName(id) => self.begin_typing(Target::EntryWords(id), false, window, cx),
                    _ => window.focus(&self.focus),
                }
            }
            "tab" => {
                self.end_typing(cx);
                let next = match target {
                    Target::Title(id) => Some(Target::Story(id)),
                    Target::Story(id) => Some(Target::Capture(id)),
                    Target::EntryName(id) => Some(Target::EntryWords(id)),
                    _ => None,
                };
                match next {
                    Some(next) => self.begin_typing(next, false, window, cx),
                    None => window.focus(&self.focus),
                }
            }
            "enter" => self.type_story("\n"),
            "backspace" => self.with_typing(|text, t| {
                if t.all {
                    text.clear();
                    t.cursor = 0;
                    t.all = false;
                    return;
                }
                let at = t.cursor.min(text.len());
                let from = if cmd {
                    text[..at].rfind('\n').map_or(0, |i| i + 1)
                } else if alt {
                    word_left(text, at)
                } else {
                    prev(text, at)
                };
                text.replace_range(from..at, "");
                t.cursor = from;
            }),
            "delete" => self.with_typing(|text, t| {
                let at = t.cursor.min(text.len());
                let to = next(text, at);
                text.replace_range(at..to, "");
                t.all = false;
            }),
            "left" | "right" | "home" | "end" | "up" | "down" => self.with_typing(|text, t| {
                let at = t.cursor.min(text.len());
                t.cursor = match k.key.as_str() {
                    "left" if t.all => 0,
                    "right" if t.all => text.len(),
                    "left" if cmd => text[..at].rfind('\n').map_or(0, |i| i + 1),
                    "right" if cmd => text[at..].find('\n').map_or(text.len(), |i| at + i),
                    "left" if alt => word_left(text, at),
                    "left" => prev(text, at),
                    "right" => next(text, at),
                    "home" | "up" => 0,
                    _ => text.len(),
                };
                t.all = false;
            }),
            _ if cmd || k.modifiers.control => return,
            _ => match k.key_char.clone().filter(|c| !c.chars().any(char::is_control)) {
                Some(c) => self.type_story(&c),
                None => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Words that can be typed into: shown with a caret while typing.
    pub(super) fn field(
        &mut self,
        target: Target,
        placeholder: &'static str,
        size_: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let text = self.text_of(target).unwrap_or_default();
        let typing = self.typing.as_ref().filter(|t| t.target == target);
        let (cursor, all) = typing.map_or((text.len(), false), |t| (t.cursor.min(text.len()), t.all));
        let active = typing.is_some();
        let id = SharedString::from(format!("{target:?}"));
        // One run of text, so long lines wrap; the caret is a thin bar
        // character in the accent colour.
        let mut shown = if text.is_empty() && !active {
            placeholder.to_string()
        } else {
            text.clone()
        };
        let mut highlights = Vec::new();
        if active && !all {
            const CARET: &str = "\u{258f}";
            shown.insert_str(cursor, CARET);
            highlights.push((
                cursor..cursor + CARET.len(),
                gpui::HighlightStyle {
                    color: Some(Theme::accent().into()),
                    ..Default::default()
                },
            ));
        }
        if shown.is_empty() {
            shown.push(' ');
        }
        div()
            .id(id)
            .rounded(px(5.0))
            .px(px(4.0))
            .mx(px(-4.0))
            .cursor_text()
            .text_size(px(size_))
            .line_height(px(size_ * 1.35))
            .text_color(if text.is_empty() && !active {
                Theme::surface2()
            } else {
                Theme::text()
            })
            .when(active, |d| d.bg(Theme::crust()))
            .when(all && active, |d| d.bg(Theme::accent_soft()))
            .hover(|s| s.bg(Theme::crust()))
            .child(gpui::StyledText::new(shown).with_highlights(highlights))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();
                    this.begin_typing(target, e.click_count >= 2, window, cx);
                }),
            )
    }

    // The board.

    pub(super) fn render_board(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.page == super::plan::Page::Bible {
            return self.render_bible(cx);
        }
        let story = self.doc.project.story.clone();
        let total = story.end();
        let cards: Vec<AnyElement> = story
            .beats
            .iter()
            .zip(story.starts())
            .enumerate()
            .map(|(n, (beat, start))| self.beat_card(n, beat, start, cx))
            .collect();
        let empty = story.beats.is_empty();
        let add = div()
            .id("add-beat")
            .w(px(232.0))
            .h(px(236.0))
            .rounded(px(10.0))
            .border_1()
            .border_dashed()
            .border_color(Theme::surface1())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|s| s.border_color(Theme::accent()).bg(Theme::lane()))
            .child(icon("plus", Theme::subtext()).size(px(18.0)))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(Theme::subtext())
                    .child("Add a beat"),
            )
            .child(div().text_size(px(10.5)).text_color(Theme::overlay()).child("B"))
            .on_click(cx.listener(|this, _, window, cx| this.new_beat(window, cx)));

        let cues: Vec<AnyElement> = story
            .cues
            .iter()
            .map(|cue| {
                let (id, at) = (cue.id, cue.at);
                div()
                    .id(("cue", id.0 as usize))
                    .h(px(28.0))
                    .pl(px(8.0))
                    .pr(px(4.0))
                    .rounded(px(7.0))
                    .bg(Theme::lane())
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id(("cue-time", id.0 as usize))
                            .font_family(MONO_FONT)
                            .text_size(px(10.5))
                            .text_color(Theme::overlay())
                            .cursor_pointer()
                            .hover(|s| s.text_color(Theme::text()))
                            .child(timecode(at, self.doc.project.settings.fps))
                            .on_click(cx.listener(move |this, _, _, cx| this.seek(at, cx))),
                    )
                    .child(self.field(Target::Cue(id), "Name it", 12.0, cx))
                    .child(
                        div()
                            .id(("cue-remove", id.0 as usize))
                            .size(px(20.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(Theme::surface0()))
                            .child(icon("close", Theme::overlay()).size(px(11.0)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.typing = None;
                                this.edit(cx, |p| p.remove_cue(id));
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let header_button = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .border_1()
                .border_color(Theme::edge())
                .hover(|s| s.bg(Theme::lane()))
                .child(icon(glyph, Theme::subtext()).size(px(13.0)))
                .child(div().text_size(px(11.5)).text_color(Theme::text()).child(label))
        };

        panel()
            .id("board")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(52.0))
                    .flex_none()
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(Theme::edge())
                    .child(self.page_switch(cx))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(
                        div().text_size(px(10.5)).text_color(Theme::overlay()).child(if empty {
                            "Plan the movie: beats in order, what happens, what to capture".to_string()
                        } else {
                            format!(
                                "{} beat{} · {}",
                                story.beats.len(),
                                if story.beats.len() == 1 { "" } else { "s" },
                                seconds(total)
                            )
                        }),
                    ))
                    .child(
                        header_button("story-cue", "flag", "Cue")
                            .tooltip(tip("Mark a cue at the playhead", Some("C")))
                            .on_click(cx.listener(|this, _, window, cx| this.new_cue(window, cx))),
                    )
                    .child(
                        header_button("story-brief", "text", "Brief")
                            .tooltip(tip(
                                "The chosen beat's brief: copies a prompt to start from, and shows its references in Finder",
                                None,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.copy_brief(cx))),
                    )
                    .child(
                        header_button("story-copy", "copy", "Shot list")
                            .tooltip(tip("Copy the story as a shot list", None))
                            .on_click(cx.listener(|this, _, _, cx| this.copy_shot_list(cx))),
                    )
                    .child(
                        header_button("story-record", "record", "Record")
                            .tooltip(tip("Record the screen into the chosen beat", Some("⇧⌘R")))
                            .on_click(cx.listener(|this, _, window, cx| this.open_record(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("board-cards")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(div().flex().flex_wrap().gap(px(14.0)).children(cards).child(add))
                    .when(!cues.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(caps("Cues"))
                                .child(div().flex().flex_wrap().gap(px(6.0)).children(cues)),
                        )
                    })
                    .when(empty, |d| {
                        d.child(
                            div()
                                .max_w(px(520.0))
                                .text_size(px(11.5))
                                .line_height(px(17.0))
                                .text_color(Theme::overlay())
                                .child(
                                    "Each beat is a stretch of the movie, end to end along the timeline. Say what \
                                     happens and what to capture; drop media on a beat to lay it in, or record \
                                     the screen straight into it. Drag cards to reorder: their footage follows.",
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// A frame of the footage laid into a beat, from its filmstrip.
    fn beat_frame(&self, start: Time, duration: Time) -> Option<std::sync::Arc<RenderImage>> {
        let mid = start + duration / 2;
        let project = &self.doc.project;
        let clip = project.visual_stack(mid).into_iter().rev().find_map(|(c, s)| match s {
            // A see-through scene shows little on its own: look under it.
            Source::Media(m) if !matches!(&m.recipe, Some(splice_core::model::Recipe::Scene(sc)) if sc.transparent) => {
                Some(c)
            }
            _ => None,
        })?;
        let media = project.media(clip.media)?;
        let assets = self.library.get(&media.hash)?;
        let at = clip.source_at(mid);
        assets
            .strip
            .iter()
            .min_by_key(|(t, _)| (t - at).abs())
            .map(|(_, i)| i.clone())
            .or_else(|| assets.still.clone())
            .or_else(|| assets.thumb.clone())
    }

    fn beat_card(
        &mut self,
        n: usize,
        beat: &splice_core::story::Beat,
        start: Time,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = beat.id;
        let chosen = self.beat == Some(id);
        let coverage = self.doc.project.coverage(id);
        let frame = self.beat_frame(start, beat.duration);
        // The reference shot on show: the one clicked, else the first with a picture.
        let shown = self
            .shown_shots
            .get(&id)
            .and_then(|s| beat.shots.iter().find(|x| x.id == *s))
            .or_else(|| beat.shots.iter().find(|x| x.image.is_some()))
            .cloned();
        let reference = shown
            .as_ref()
            .and_then(|s| s.image.as_ref())
            .and_then(|i| self.still_file(i));
        // Footage wins once there is some, unless a shot was clicked to compare.
        let compare = self.shown_shots.contains_key(&id);
        // Whether the picture area shows a reference (and so its note below).
        let showing_reference = reference.is_some() && (frame.is_none() || compare);
        let color = beat.color;
        let duration = beat.duration;
        let capture = beat.capture.clone();
        let step = |delta: Time| {
            move |this: &mut Editor, _: &ClickEvent, _: &mut Window, cx: &mut Context<Editor>| {
                let d = (duration + delta).max(SECOND);
                this.edit(cx, |p| p.set_beat_duration(id, d));
            }
        };
        let picture = div()
            .relative()
            .w_full()
            .h(px(118.0))
            .rounded(px(7.0))
            .overflow_hidden()
            .bg(Theme::crust())
            .map(|d| match (frame, reference) {
                (Some(image), None) => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
                (Some(image), Some(_)) if !compare => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
                (_, Some(path)) => d.child(img(path).size_full().object_fit(ObjectFit::Contain)),
                (None, None) => d
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .child(icon("film", Theme::surface2()).size(px(18.0)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .line_height(px(14.0))
                            .text_color(Theme::overlay())
                            .text_center()
                            .child(if capture.trim().is_empty() {
                                "Needs footage: drop media here, or record".to_string()
                            } else {
                                format!("To capture: {}", capture.lines().next().unwrap_or(""))
                            }),
                    ),
            })
            .child(
                div()
                    .absolute()
                    .top(px(6.0))
                    .right(px(6.0))
                    .h(px(18.0))
                    .px(px(6.0))
                    .rounded(px(9.0))
                    .bg(Theme::scrim())
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(div().size(px(6.0)).rounded_full().bg(match coverage {
                        Coverage::Full => Theme::wave(),
                        Coverage::Partial => gpui::rgb(0xf2c14e),
                        Coverage::Empty => Theme::surface2(),
                    }))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(Theme::subtext())
                            .child(match coverage {
                                Coverage::Full => "covered",
                                Coverage::Partial => "partly",
                                Coverage::Empty => "empty",
                            }),
                    ),
            );

        let drag = BeatDrag {
            beat: id,
            title: SharedString::from(if beat.title.is_empty() {
                "Untitled".to_string()
            } else {
                beat.title.clone()
            }),
        };
        let fps = self.doc.project.settings.fps;
        div()
            .id(("beat", id.0 as usize))
            .w(px(232.0))
            .min_h(px(236.0))
            .p(px(10.0))
            .rounded(px(10.0))
            .bg(Theme::lane())
            .border_1()
            .border_color(if chosen { rgb(color) } else { Theme::edge().into() })
            .flex()
            .flex_col()
            .gap(px(8.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.end_typing(cx);
                this.select_beat(Some(id), cx);
            }))
            .on_drag(drag, |drag: &BeatDrag, _, _, cx| cx.new(|_| drag.clone()))
            .drag_over::<BeatDrag>(|s, _, _, _| s.border_color(Theme::accent()))
            .drag_over::<MediaDrag>(|s, _, _, _| s.border_color(Theme::accent()).bg(Theme::surface0()))
            .on_drop(cx.listener(move |this, drag: &BeatDrag, _, cx| {
                if drag.beat != id
                    && let Some(to) = this.doc.project.story.index(id)
                {
                    let moving = drag.beat;
                    this.edit(cx, |p| p.move_beat(moving, to));
                    this.beat = Some(moving);
                }
            }))
            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(Theme::accent()).bg(Theme::surface0()))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                // Files from Finder on a card are reference shots, not footage.
                cx.stop_propagation();
                this.ghost = None;
                this.add_references(id, paths.paths().to_vec(), None, None, cx);
            }))
            .on_drop(cx.listener(move |this, drag: &MediaDrag, _, cx| {
                if drag.generator.is_none() && drag.symbol.is_none() {
                    let media = drag.media;
                    if let Some(clip) = this.edit(cx, |p| p.fill_beat(id, media)) {
                        this.selection = vec![clip];
                    }
                    this.beat = Some(id);
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .size(px(20.0))
                            .flex_none()
                            .rounded(px(5.0))
                            .bg(rgb(color))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(Theme::ground())
                            .child((n + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.field(Target::Title(id), "Name this beat", 13.0, cx)),
                    ),
            )
            .child(self.cast_row(beat, cx))
            .child(picture)
            .when_some(self.takes_strip(beat, cx), |d, takes| d.child(takes))
            .child(self.shot_strip(beat, shown.as_ref().map(|s| s.id), cx))
            .when_some(shown.as_ref().filter(|_| showing_reference), |d, shot| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(Theme::overlay())
                                .child(format!(
                                    "Shot {}",
                                    beat.shots.iter().position(|s| s.id == shot.id).map_or(0, |i| i + 1)
                                )),
                        )
                        .child(div().flex_1().min_w_0().child(self.field(
                            Target::Shot(shot.id),
                            "What this shot is",
                            11.0,
                            cx,
                        ))),
                )
            })
            .child(self.field(Target::Story(id), "What happens", 11.5, cx))
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(icon("record", Theme::overlay()).size(px(11.0)).mt(px(3.0)))
                    .child(div().flex_1().min_w_0().child(self.field(
                        Target::Capture(id),
                        "What to capture",
                        11.0,
                        cx,
                    ))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family(MONO_FONT)
                            .text_size(px(10.0))
                            .text_color(Theme::overlay())
                            .child(timecode(start, fps)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(
                                render::icon_button(SharedString::from(format!("beat-less-{}", id.0)), "minus", true)
                                    .tooltip(tip("A second shorter", None))
                                    .on_click(cx.listener(step(-SECOND))),
                            )
                            .child(
                                div()
                                    .w(px(36.0))
                                    .text_center()
                                    .font_family(MONO_FONT)
                                    .text_size(px(11.0))
                                    .text_color(Theme::text())
                                    .child(seconds(duration)),
                            )
                            .child(
                                render::icon_button(SharedString::from(format!("beat-more-{}", id.0)), "plus", true)
                                    .tooltip(tip("A second longer", None))
                                    .on_click(cx.listener(step(SECOND))),
                            )
                            .child(
                                render::icon_button(SharedString::from(format!("beat-remove-{}", id.0)), "trash", true)
                                    .tooltip(tip("Remove the beat (its footage stays)", None))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.typing = None;
                                        this.edit(cx, |p| p.remove_beat(id));
                                        if this.beat == Some(id) {
                                            this.beat = None;
                                        }
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// A beat's reference shots as small pictures, the one on show outlined,
    /// with ways to add more.
    fn shot_strip(
        &mut self,
        beat: &splice_core::story::Beat,
        shown: Option<splice_core::story::ShotId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = beat.id;
        let thumbs: Vec<AnyElement> = beat
            .shots
            .iter()
            .enumerate()
            .map(|(i, shot)| {
                let sid = shot.id;
                let on = shown == Some(sid) && self.shown_shots.contains_key(&id);
                let path = shot.image.as_ref().and_then(|s| self.still_file(s));
                let group: SharedString = format!("shot-{}", sid.0).into();
                div()
                    .id(("shot", sid.0 as usize))
                    .group(group.clone())
                    .relative()
                    .w(px(48.0))
                    .h(px(30.0))
                    .flex_none()
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .bg(Theme::crust())
                    .border_1()
                    .border_color(if on { Theme::text() } else { Theme::edge() })
                    .cursor_pointer()
                    .map(|d| match path {
                        Some(path) => d.child(img(path).size_full().object_fit(ObjectFit::Cover)),
                        None => d
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .text_color(Theme::overlay())
                            .child((i + 1).to_string()),
                    })
                    .child(
                        div()
                            .id(("shot-remove", sid.0 as usize))
                            .absolute()
                            .top(px(1.0))
                            .right(px(1.0))
                            .size(px(14.0))
                            .rounded(px(3.0))
                            .bg(Theme::scrim())
                            .flex()
                            .items_center()
                            .justify_center()
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .child(icon("close", Theme::text()).size(px(9.0)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.shown_shots.remove(&id);
                                this.remove_shot(sid, cx);
                            })),
                    )
                    .tooltip(tip(
                        if shot.note.is_empty() {
                            "Reference shot".to_string()
                        } else {
                            shot.note.clone()
                        },
                        None,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Click again to go back to the footage.
                        if this.shown_shots.get(&id) == Some(&sid) {
                            this.shown_shots.remove(&id);
                        } else {
                            this.shown_shots.insert(id, sid);
                        }
                        this.beat = Some(id);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let tool = |key: String, glyph: &'static str, hint: &'static str| {
            div()
                .id(SharedString::from(key))
                .size(px(30.0))
                .flex_none()
                .rounded(px(4.0))
                .border_1()
                .border_dashed()
                .border_color(Theme::surface1())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.border_color(Theme::accent()).bg(Theme::surface0()))
                .tooltip(tip(hint, None))
                .child(icon(glyph, Theme::subtext()).size(px(12.0)))
        };
        div()
            .flex()
            .flex_wrap()
            .gap(px(4.0))
            .children(thumbs)
            .child(
                tool(
                    format!("shot-add-{}", id.0),
                    "plus",
                    "Add reference pictures (or drop them, or paste with ⌘V)",
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick_references(id, cx))),
            )
            .child(
                tool(
                    format!("shot-grab-{}", id.0),
                    "film",
                    "Use the viewer's frame as a reference",
                )
                .on_click(cx.listener(move |this, _, _, cx| this.grab_reference(id, cx))),
            )
            .into_any_element()
    }

    /// The text field everything typed into the board goes through.
    pub(super) fn story_keys(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .id("story-keys")
            .key_context("TextField")
            .track_focus(&self.story_focus)
            .on_key_down(cx.listener(Self::story_key))
            .on_action(cx.listener(|this, _: &TextSelectAll, _, cx| {
                if let Some(t) = &mut this.typing {
                    t.all = true;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TextPaste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    this.type_story(&text);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &TextDone, window, cx| {
                this.end_typing(cx);
                window.focus(&this.focus);
            }))
            .absolute()
            .size_0()
    }
}

impl Editor {
    /// A beat as `splice ctl story read` shows it.
    fn beat_json(&self, n: usize, beat: &splice_core::story::Beat, start: Time) -> serde_json::Value {
        let project = &self.doc.project;
        serde_json::json!({
            "beat": beat.id.0,
            "number": n + 1,
            "title": beat.title,
            "start": secs(start),
            "end": secs(start + beat.duration),
            "duration": secs(beat.duration),
            "story": beat.story,
            "capture": beat.capture,
            "color": format!("#{:06x}", beat.color),
            "coverage": match project.coverage(beat.id) {
                Coverage::Empty => "empty",
                Coverage::Partial => "partial",
                Coverage::Full => "covered",
            },
            "clips": project.beat_clips(beat.id).iter().map(|c| c.0).collect::<Vec<_>>(),
            "shots": beat.shots.iter().map(|s| self.shot_json(s)).collect::<Vec<_>>(),
            "cast": beat.cast.iter().filter_map(|e| project.bible.entry(*e))
                .map(|e| serde_json::json!({ "entry": e.id.0, "name": e.name, "kind": e.kind }))
                .collect::<Vec<_>>(),
            "takes": beat.takes.iter().map(|t| self.take_json(t)).collect::<Vec<_>>(),
            "chosen_take": project.chosen_take(beat.id).map(|t| t.0),
        })
    }

    pub(super) fn story_json(&self) -> serde_json::Value {
        let story = &self.doc.project.story;
        serde_json::json!({
            "beats": story.beats.iter().zip(story.starts()).enumerate()
                .map(|(n, (b, s))| self.beat_json(n, b, s)).collect::<Vec<_>>(),
            "cues": story.cues.iter().map(|c| serde_json::json!({
                "cue": c.id.0, "at": secs(c.at), "label": c.label,
            })).collect::<Vec<_>>(),
            "length": secs(story.end()),
            "selected": self.beat.map(|b| b.0),
            "view": match self.mode { Mode::Edit => "edit", Mode::Story => "story", Mode::Record => "record" },
        })
    }

    /// `splice ctl story …`, `cue …` and `view`.
    pub(super) fn ctl_story(
        &mut self,
        cmd: &str,
        request: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        use crate::ctl::fail;
        use serde_json::{Value, json};
        let text = |key: &str| request.get(key).and_then(Value::as_str).map(str::to_string);
        let sub = request.get("sub").and_then(Value::as_str).unwrap_or("read");
        let beat = |this: &Self| -> Result<BeatId, String> {
            let id = request
                .get("beat")
                .and_then(Value::as_u64)
                .ok_or("name the beat with --beat B")?;
            let id = BeatId(id);
            this.doc
                .project
                .story
                .index(id)
                .map(|_| id)
                .ok_or(format!("no beat {}; `splice ctl story read` lists them", id.0))
        };
        let done = |this: &mut Self, extra: Value| {
            let mut out = this.edited_with(extra);
            out["story"] = this.story_json();
            out
        };
        let duration = match self.time(request, "duration") {
            Ok(d) => d,
            Err(e) => return fail(e),
        };
        let color = match text("color").map(|c| crate::editor::agent::parse_color(&c)) {
            Some(Some(c)) => Some(c >> 8),
            Some(None) => return fail("--color is #rrggbb or a name: vermilion, gold, mint, sky, plum"),
            None => None,
        };
        match (cmd, sub) {
            ("view", _) => {
                let mode = match text("view").as_deref() {
                    Some("story") => Mode::Story,
                    Some("edit") => Mode::Edit,
                    Some("record") => Mode::Record,
                    _ => return fail("view is edit, story or record"),
                };
                self.set_mode(mode, cx);
                json!({ "ok": true, "view": text("view") })
            }
            ("story", "read") => {
                let mut out = json!({ "ok": true });
                out["story"] = self.story_json();
                out
            }
            ("story", "list") => json!({ "ok": true, "text": self.doc.project.shot_list() }),
            ("story", "add") => {
                let after = request.get("after").and_then(Value::as_u64).map(BeatId);
                if let Some(a) = after
                    && self.doc.project.story.index(a).is_none()
                {
                    return fail(format!("no beat {}", a.0));
                }
                let title = text("title").unwrap_or_else(|| format!("Beat {}", self.doc.project.story.beats.len() + 1));
                let (story, capture) = (text("story"), text("capture"));
                let id = self.edit(cx, |p| {
                    let id = p.add_beat(after, &title, duration.unwrap_or(BEAT_DURATION));
                    p.set_beat(id, |b| {
                        if let Some(s) = story {
                            b.story = s;
                        }
                        if let Some(c) = capture {
                            b.capture = c;
                        }
                        if let Some(c) = color {
                            b.color = c;
                        }
                    });
                    id
                });
                self.beat = Some(id);
                done(self, json!({ "beat": id.0 }))
            }
            ("story", "set") => {
                let id = match beat(self) {
                    Ok(id) => id,
                    Err(e) => return fail(e),
                };
                let (title, story, capture) = (text("title"), text("story"), text("capture"));
                self.typing = None;
                self.edit(cx, |p| {
                    p.set_beat(id, |b| {
                        if let Some(t) = title {
                            b.title = t;
                        }
                        if let Some(s) = story {
                            b.story = s;
                        }
                        if let Some(c) = capture {
                            b.capture = c;
                        }
                        if let Some(c) = color {
                            b.color = c;
                        }
                    });
                    if let Some(d) = duration {
                        p.set_beat_duration(id, d);
                    }
                });
                done(self, json!({ "beat": id.0 }))
            }
            ("story", "move") => {
                let id = match beat(self) {
                    Ok(id) => id,
                    Err(e) => return fail(e),
                };
                let Some(to) = request.get("index").and_then(Value::as_u64) else {
                    return fail("say where with --index I (0 is first)");
                };
                self.edit(cx, |p| p.move_beat(id, to as usize));
                done(self, json!({ "beat": id.0 }))
            }
            ("story", "remove") => {
                let id = match beat(self) {
                    Ok(id) => id,
                    Err(e) => return fail(e),
                };
                self.typing = None;
                self.edit(cx, |p| p.remove_beat(id));
                if self.beat == Some(id) {
                    self.beat = None;
                }
                done(self, json!({}))
            }
            ("story", "fill") => {
                let id = match beat(self) {
                    Ok(id) => id,
                    Err(e) => return fail(e),
                };
                let media = match self.find_media(&request["media"]) {
                    Ok(m) => m,
                    Err(e) => return fail(e),
                };
                match self.edit(cx, |p| p.fill_beat(id, media)) {
                    Some(clip) => {
                        let clip_json = self.clip_json(clip);
                        done(self, json!({ "clip": clip_json }))
                    }
                    None => fail("that media could not be placed"),
                }
            }
            ("cue", "add") => {
                let at = match self.time(request, "at") {
                    Ok(t) => t.unwrap_or(self.player.position),
                    Err(e) => return fail(e),
                };
                let label = text("label").unwrap_or_else(|| format!("Cue {}", self.doc.project.story.cues.len() + 1));
                let id = self.edit(cx, |p| p.add_cue(at, &label));
                done(self, json!({ "cue": id.0 }))
            }
            ("cue", "set" | "remove") => {
                let Some(id) = request.get("cue").and_then(Value::as_u64).map(CueId) else {
                    return fail("name the cue with --cue C");
                };
                if !self.doc.project.story.cues.iter().any(|c| c.id == id) {
                    return fail(format!("no cue {}", id.0));
                }
                let at = match self.time(request, "at") {
                    Ok(t) => t,
                    Err(e) => return fail(e),
                };
                let label = text("label");
                self.typing = None;
                if sub == "remove" {
                    self.edit(cx, |p| p.remove_cue(id));
                } else {
                    self.edit(cx, |p| {
                        p.set_cue(id, |c| {
                            if let Some(t) = at {
                                c.at = t.max(0);
                            }
                            if let Some(l) = label {
                                c.label = l;
                            }
                        })
                    });
                }
                done(self, json!({}))
            }
            _ => fail(format!("unknown `{cmd} {sub}`; see `splice ctl --help`")),
        }
    }
}
