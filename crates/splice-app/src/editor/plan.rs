//! Planning beyond the beats: the bible (characters, places, things and the
//! look, each with pictures and continuity words), who is cast in each beat,
//! the takes tried for a beat (generated or shot) with where each came from,
//! and briefs that gather all of it for whoever makes a beat's footage.
//!
//! The bible is a page of the Story view, built like the board: cards with
//! pictures you drop in, words you type in place. Cast shows on each beat
//! card as faces; takes as a strip, the one laid in ringed, a pending one
//! breathing until its footage arrives.

use super::render::{caps, icon, panel, tip};
use super::story::Target;
use super::*;
use gpui::{Animation, AnimationExt, FontWeight, ObjectFit, img, pulsating_between};
use splice_core::bible::{Entry, EntryId, Kind};
use splice_core::story::{Beat, BeatId, Origin, Take, TakeId};

/// Which page of the Story view shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Beats,
    Bible,
}

/// How many frames at the start of a generated extension repeat what it
/// extends: skipped when such a take is laid in.
const SEAM_FRAMES: i64 = 3;

/// A pending take this old has probably been lost.
const STALE: u64 = 30 * 60;

/// Seconds since 1970, now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What footage being imported becomes: a new take of a beat, or a pending
/// take landing.
#[derive(Clone, Debug)]
pub enum Landing {
    New {
        beat: BeatId,
        origin: Origin,
        skip: Time,
        choose: bool,
    },
    Land {
        take: TakeId,
        origin: Origin,
        skip: Time,
        choose: bool,
    },
}

impl Landing {
    /// Makes or lands the take with `media`, inside an import's edit.
    /// Returns what happened, for the reply.
    pub fn land(self, project: &mut Project, media: MediaId) -> Option<serde_json::Value> {
        let (take, choose) = match self {
            Landing::New {
                beat,
                origin,
                skip,
                choose,
            } => {
                let id = project.add_take(beat, Some(media), origin, now())?;
                project.set_take(id, |t| t.skip = skip);
                // A beat with nothing laid in takes its first take.
                let choose = choose || project.chosen_take(beat).is_none();
                (id, choose)
            }
            Landing::Land {
                take,
                origin,
                skip,
                choose,
            } => {
                let beat = project.take_beat(take)?;
                project.set_take(take, |t| {
                    t.media = Some(media);
                    t.skip = skip.max(t.skip);
                    // What the landing adds to what was known.
                    let o = &mut t.origin;
                    for (field, value) in [
                        (&mut o.tool, origin.tool),
                        (&mut o.model, origin.model),
                        (&mut o.mode, origin.mode),
                        (&mut o.prompt, origin.prompt),
                        (&mut o.job, origin.job),
                        (&mut o.note, origin.note),
                    ] {
                        if !value.is_empty() {
                            *field = value;
                        }
                    }
                    if !origin.references.is_empty() {
                        o.references = origin.references;
                    }
                });
                (take, choose || project.chosen_take(beat).is_none())
            }
        };
        let beat = project.take_beat(take)?;
        let clip = if choose { project.choose_take(take) } else { None };
        let (start, end) = project.story.span(beat)?;
        let room = end - start;
        let length = project.media(media).map_or(0, |m| m.duration);
        let skip = project.take(take).map_or(0, |t| t.skip);
        let usable = (length - skip).max(0);
        Some(serde_json::json!({
            "take": take.0,
            "beat": beat.0,
            "clip": clip.map(|c| c.0),
            "chosen": clip.is_some(),
            // A take shorter than its beat leaves a gap; a longer one is cut.
            "short_by": (usable < room).then(|| secs(room - usable)),
            "trimmed_by": (usable > room).then(|| secs(usable - room)),
        }))
    }
}

fn rgb(color: u32) -> gpui::Hsla {
    gpui::rgb(color).into()
}

/// An entry's initials, for a face without a picture.
fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect();
    if letters.is_empty() {
        "?".into()
    } else {
        letters.to_uppercase()
    }
}

impl Editor {
    // The page switch.

    pub(super) fn page_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.doc.project.bible.entries.len();
        let tab = |id: &'static str, label: String, page: Page, on: bool| {
            div()
                .id(id)
                .h(px(24.0))
                .px(px(10.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if on { Theme::text() } else { Theme::overlay() })
                .when(on, |d| d.bg(Theme::surface0()))
                .hover(|s| s.text_color(Theme::text()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.end_typing(cx);
                    this.page = page;
                    cx.notify();
                }))
        };
        div()
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(Theme::crust())
            .flex()
            .gap(px(2.0))
            .child(tab("page-beats", "Beats".into(), Page::Beats, self.page == Page::Beats))
            .child(tab(
                "page-bible",
                if count == 0 {
                    "Bible".into()
                } else {
                    format!("Bible · {count}")
                },
                Page::Bible,
                self.page == Page::Bible,
            ))
            .into_any_element()
    }

    // The bible.

    /// A new entry of `kind`, named and ready to type.
    pub(super) fn new_entry(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.doc.project.bible.of(kind).count() + 1;
        let name = match kind {
            Kind::Character => format!("Character {n}"),
            Kind::Place => format!("Place {n}"),
            Kind::Thing => format!("Thing {n}"),
            Kind::Look => "The look".to_string(),
        };
        let id = self.edit(cx, |p| p.add_entry(kind, &name));
        self.page = Page::Bible;
        self.begin_typing(Target::EntryName(id), true, window, cx);
    }

    pub(super) fn render_bible(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let bible = self.doc.project.bible.clone();
        let empty = bible.is_empty();
        let mut sections: Vec<AnyElement> = Vec::new();
        // The look first: it is said once, before every shot.
        for kind in [Kind::Look, Kind::Character, Kind::Place, Kind::Thing] {
            let entries: Vec<Entry> = bible.of(kind).cloned().collect();
            let cards: Vec<AnyElement> = entries.iter().map(|e| self.entry_card(e, cx)).collect();
            // A full card to start a kind; once it has some, a small tile.
            let first = entries.is_empty();
            let add = div()
                .id(SharedString::from(format!("add-{}", kind.name())))
                .w(px(if first && kind != Kind::Look { 200.0 } else { 180.0 }))
                .h(px(if first && kind != Kind::Look { 260.0 } else { 40.0 }))
                .rounded(px(10.0))
                .border_1()
                .border_dashed()
                .border_color(Theme::surface1())
                .flex()
                .when(first && kind != Kind::Look, |d| d.flex_col())
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .cursor_pointer()
                .hover(|s| s.border_color(rgb(kind.color())).bg(Theme::lane()))
                .child(icon("plus", Theme::subtext()).size(px(if first { 16.0 } else { 12.0 })))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(Theme::subtext())
                        .child(match kind {
                            Kind::Character => "Add a character",
                            Kind::Place => "Add a place",
                            Kind::Thing => "Add a thing",
                            Kind::Look => "Describe the look",
                        }),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.new_entry(kind, window, cx)));
            // The look is one thing for the whole movie.
            let show_add = kind != Kind::Look || entries.is_empty();
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().size(px(8.0)).rounded_full().bg(rgb(kind.color())))
                            .child(caps(kind.plural()))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(Theme::overlay())
                                    .child(match kind {
                                        Kind::Character => "who appears: their face and what never changes",
                                        Kind::Place => "where it happens",
                                        Kind::Thing => "props and products that must look the same",
                                        Kind::Look => "light, colour, lens, grain: said once, for every shot",
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(14.0))
                            .children(cards)
                            .when(show_add, |d| d.child(add)),
                    )
                    .into_any_element(),
            );
        }
        panel()
            .id("bible")
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.5))
                            .text_color(Theme::overlay())
                            .truncate()
                            .child(if empty {
                                "Who and what the movie keeps coming back to, so every shot matches"
                            } else {
                                "Cast them into beats from each beat card; briefs carry their pictures and words"
                            }),
                    ),
            )
            .child(
                div()
                    .id("bible-cards")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(22.0))
                    .children(sections),
            )
            .into_any_element()
    }

    fn entry_card(&mut self, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let id = entry.id;
        let kind = entry.kind;
        let color = kind.color();
        let pictures: Vec<PathBuf> = entry.pictures.iter().filter_map(|s| self.still_file(s)).collect();
        let shown = self
            .shown_pictures
            .get(&id)
            .copied()
            .filter(|i| *i < pictures.len())
            .unwrap_or(0);
        let beats = self.doc.project.cast_in(id).len();
        let face = pictures.get(shown).cloned();
        // A face wants room for a head; a place, the width of a view. The
        // look spans the page, its picture beside its words.
        let wide = kind == Kind::Look;
        let picture = div()
            .relative()
            .when(wide, |d| d.w(px(220.0)).flex_none())
            .when(!wide, |d| d.w_full())
            .h(px(match kind {
                Kind::Character => 180.0,
                Kind::Look => 124.0,
                _ => 132.0,
            }))
            .rounded(px(7.0))
            .overflow_hidden()
            .bg(Theme::crust())
            .map(|d| match face {
                Some(path) => d.child(img(path).size_full().object_fit(ObjectFit::Cover)),
                None => d
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .size(px(44.0))
                            .rounded_full()
                            .bg(rgb(color))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(Theme::ground())
                            .map(|d| {
                                if kind == Kind::Look {
                                    d.child(icon("swatch", Theme::ground()).size(px(20.0)))
                                } else {
                                    d.child(initials(&entry.name))
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(Theme::overlay())
                            .child("Drop pictures here"),
                    ),
            });
        // The pictures as a strip, the one on show outlined, with ways to add.
        let thumbs: Vec<AnyElement> = pictures
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let group: SharedString = format!("pic-{}-{i}", id.0).into();
                div()
                    .id(SharedString::from(format!("pic-{}-{i}", id.0)))
                    .group(group.clone())
                    .relative()
                    .w(px(40.0))
                    .h(px(28.0))
                    .flex_none()
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(if i == shown { Theme::text() } else { Theme::edge() })
                    .cursor_pointer()
                    .child(img(path.clone()).size_full().object_fit(ObjectFit::Cover))
                    .child(
                        div()
                            .id(SharedString::from(format!("pic-remove-{}-{i}", id.0)))
                            .absolute()
                            .top(px(1.0))
                            .right(px(1.0))
                            .size(px(13.0))
                            .rounded(px(3.0))
                            .bg(Theme::scrim())
                            .flex()
                            .items_center()
                            .justify_center()
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .child(icon("close", Theme::text()).size(px(8.0)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.typing = None;
                                this.shown_pictures.remove(&id);
                                this.edit(cx, |p| {
                                    p.set_entry(id, |e| {
                                        if i < e.pictures.len() {
                                            e.pictures.remove(i);
                                        }
                                    })
                                });
                            })),
                    )
                    .tooltip(tip(
                        if i == 0 {
                            "The face (first picture)"
                        } else {
                            "Click to show; drag to the first place with ⌘-click"
                        },
                        None,
                    ))
                    .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                        // ⌘-click makes it the first: the face briefs lead with.
                        if e.modifiers().platform && i > 0 {
                            this.edit(cx, |p| {
                                p.set_entry(id, |e| {
                                    let pic = e.pictures.remove(i);
                                    e.pictures.insert(0, pic);
                                })
                            });
                            this.shown_pictures.insert(id, 0);
                        } else {
                            this.shown_pictures.insert(id, i);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let tool = |key: String, glyph: &'static str, hint: &'static str| {
            div()
                .id(SharedString::from(key))
                .w(px(28.0))
                .h(px(28.0))
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
                .child(icon(glyph, Theme::subtext()).size(px(11.0)))
        };
        let strip = div()
            .flex()
            .flex_wrap()
            .gap(px(4.0))
            .children(thumbs)
            .child(
                tool(
                    format!("pic-add-{}", id.0),
                    "plus",
                    "Add pictures (or drop them on the card)",
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick_entry_pictures(id, cx))),
            )
            .child(
                tool(format!("pic-grab-{}", id.0), "film", "Use the viewer's frame")
                    .on_click(cx.listener(move |this, _, _, cx| this.grab_entry_picture(id, cx))),
            );
        let hint = kind.hint();
        // What would make its briefs weak, said where it can be fixed.
        let missing: Vec<&str> = [
            (pictures.is_empty(), "No picture yet: briefs have nothing to show"),
            (entry.words.trim().is_empty(), "No words yet: say what never changes"),
        ]
        .iter()
        .filter(|(m, _)| *m)
        .map(|(_, t)| *t)
        .collect();
        let warnings = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(missing.into_iter().map(|t| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(div().size(px(5.0)).flex_none().rounded_full().bg(Theme::accent()))
                    .child(div().text_size(px(10.0)).text_color(Theme::overlay()).child(t))
            }));
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(div().size(px(6.0)).rounded_full().bg(rgb(color)))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(Theme::overlay())
                            .child(match (kind, beats) {
                                (Kind::Look, _) => "every beat".to_string(),
                                (_, 0) => "in no beat yet".to_string(),
                                (_, 1) => "in 1 beat".to_string(),
                                (_, n) => format!("in {n} beats"),
                            }),
                    ),
            )
            .child(
                render::icon_button(SharedString::from(format!("entry-remove-{}", id.0)), "trash", true)
                    .tooltip(tip("Remove from the bible (and from every beat)", None))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.typing = None;
                        this.edit(cx, |p| p.remove_entry(id));
                    })),
            );
        let words = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .when(wide, |d| d.flex_1().min_w_0())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.field(Target::EntryName(id), "Name", 13.0, cx)),
            )
            .child(self.field(Target::EntryWords(id), hint, 11.5, cx))
            .child(warnings);
        let card = div()
            .id(("entry", id.0 as usize))
            .when(wide, |d| d.w_full().max_w(px(640.0)).flex_row())
            .when(!wide, |d| d.w(px(200.0)).min_h(px(300.0)).flex_col())
            .p(px(10.0))
            .rounded(px(10.0))
            .bg(Theme::lane())
            .border_1()
            .border_color(Theme::edge())
            .flex()
            .gap(px(if wide { 14.0 } else { 8.0 }))
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.border_color(rgb(color)).bg(Theme::surface0()))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                cx.stop_propagation();
                this.add_entry_pictures(id, paths.paths().to_vec(), None, cx);
            }));
        if wide {
            card.child(div().flex().flex_col().gap(px(8.0)).child(picture).child(strip))
                .child(words.child(div().flex_1()).child(footer))
                .into_any_element()
        } else {
            card.child(picture)
                .child(strip)
                .child(words)
                .child(div().flex_1())
                .child(footer)
                .into_any_element()
        }
    }

    /// Adds pictures from files to a bible entry.
    pub(super) fn add_entry_pictures(
        &mut self,
        entry: EntryId,
        paths: Vec<PathBuf>,
        reply: Option<std::sync::mpsc::Sender<String>>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.working += 1;
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_spawn(async move { paths.iter().map(|p| super::shots::make_still(p)).collect::<Vec<_>>() })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                let mut added = 0;
                let mut errors = Vec::new();
                for result in made {
                    match result {
                        Ok(still) => {
                            editor.embed_still(&still, super::shots::still_path(&still));
                            editor.edit(cx, |p| p.set_entry(entry, |e| e.pictures.push(still)));
                            added += 1;
                        }
                        Err(e) => errors.push(e.to_string()),
                    }
                }
                if let Some(n) = editor.doc.project.bible.entry(entry).map(|e| e.pictures.len())
                    && added > 0
                {
                    editor.shown_pictures.insert(entry, n - 1);
                }
                match errors.first() {
                    Some(e) => editor.say_error(e.clone()),
                    None => editor.say(format!("Added {added} picture{}", if added == 1 { "" } else { "s" })),
                }
                if let Some(reply) = reply {
                    let _ = reply.send(
                        serde_json::json!({ "ok": errors.is_empty(), "added": added, "errors": errors }).to_string(),
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn pick_entry_pictures(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add pictures".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |editor, cx| editor.add_entry_pictures(entry, paths, None, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// The viewer's picture at the playhead becomes one of an entry's pictures.
    fn grab_entry_picture(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let at = self.player.position;
        let path = dirs()
            .cache
            .join(format!("grab-{}-{}.png", std::process::id(), entry.0));
        let planned = export::plan_still(&self.doc.project, &self.resolver(), at, &path);
        let plan = match planned {
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
                Ok(path) => editor.add_entry_pictures(entry, vec![path], None, cx),
                Err(e) => editor.say_error(format!("Could not grab the frame: {e}")),
            })
            .ok();
        })
        .detach();
    }

    // Cast, on a beat card.

    /// The faces of who and what is in a beat, and a way to change it. One
    /// face carries its name; past five, the rest are counted.
    pub(super) fn cast_row(&mut self, beat: &Beat, cx: &mut Context<Self>) -> AnyElement {
        const SHOWN: usize = 5;
        let id = beat.id;
        let entries: Vec<Entry> = beat
            .cast
            .iter()
            .filter_map(|e| self.doc.project.bible.entry(*e).cloned())
            .collect();
        let lone = (entries.len() == 1).then(|| entries[0].name.clone());
        let more = entries.len().saturating_sub(SHOWN);
        let faces: Vec<AnyElement> = entries
            .iter()
            .take(SHOWN)
            .map(|entry| self.face(entry, 22.0, format!("face-{}-{}", id.0, entry.id.0)))
            .collect();
        let has_cast = !faces.is_empty();
        div()
            .id(SharedString::from(format!("cast-{}", id.0)))
            .flex()
            .items_center()
            .gap(px(3.0))
            .min_h(px(24.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .children(faces)
            .when_some(lone, |d, name| {
                d.child(
                    div()
                        .pl(px(3.0))
                        .max_w(px(120.0))
                        .truncate()
                        .text_size(px(11.0))
                        .text_color(Theme::subtext())
                        .child(name),
                )
            })
            .when(more > 0, |d| {
                d.child(
                    div()
                        .h(px(22.0))
                        .px(px(6.0))
                        .rounded(px(11.0))
                        .bg(Theme::surface0())
                        .flex()
                        .items_center()
                        .text_size(px(10.0))
                        .text_color(Theme::subtext())
                        .child(format!("+{more}")),
                )
            })
            .child(
                div()
                    .h(px(22.0))
                    .px(px(if has_cast { 5.0 } else { 7.0 }))
                    .rounded(px(11.0))
                    .border_1()
                    .border_dashed()
                    .border_color(Theme::surface1())
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .hover(|s| s.border_color(Theme::accent()))
                    .child(icon("plus", Theme::overlay()).size(px(10.0)))
                    .when(!has_cast, |d| {
                        d.child(div().text_size(px(10.5)).text_color(Theme::overlay()).child("Cast"))
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.end_typing(cx);
                this.beat = Some(id);
                this.overlay = Overlay::Cast { beat: id, index: 0 };
                window.focus(&this.palette_focus);
                cx.notify();
            }))
            .into_any_element()
    }

    /// An entry's face: its first picture in a ring of its kind's colour,
    /// or its initials (the look: a swatch).
    fn face(&self, entry: &Entry, size: f32, key: String) -> AnyElement {
        let face = entry.pictures.first().and_then(|s| self.still_file(s));
        let words = if entry.words.trim().is_empty() {
            entry.name.clone()
        } else {
            format!("{}: {}", entry.name, entry.words.trim())
        };
        div()
            .id(SharedString::from(key))
            .size(px(size))
            .flex_none()
            .rounded_full()
            .overflow_hidden()
            .border_2()
            .border_color(rgb(entry.kind.color()))
            .bg(Theme::crust())
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match face {
                Some(path) => d.child(img(path).size_full().object_fit(ObjectFit::Cover)),
                None if entry.kind == Kind::Look => d.child(icon("swatch", Theme::text()).size(px(size * 0.5))),
                None => d
                    .text_size(px(size * 0.38))
                    .font_weight(FontWeight::BOLD)
                    .text_color(Theme::text())
                    .child(initials(&entry.name)),
            })
            .tooltip(tip(words, None))
            .into_any_element()
    }

    /// The bible's castable entries, in order: characters, places, things.
    pub(super) fn castable(&self) -> Vec<Entry> {
        [Kind::Character, Kind::Place, Kind::Thing]
            .iter()
            .flat_map(|k| self.doc.project.bible.of(*k).cloned().collect::<Vec<_>>())
            .collect()
    }

    /// Row `index` of the cast sheet: an entry to toggle in or out, or one
    /// of the last three: a new character, place or thing in this beat.
    pub(super) fn cast_pick(&mut self, beat: BeatId, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.castable();
        match entries.get(index) {
            Some(e) => {
                let entry = e.id;
                self.edit(cx, |p| p.toggle_cast(beat, entry));
            }
            None => {
                let kind = [Kind::Character, Kind::Place, Kind::Thing][(index - entries.len()).min(2)];
                self.close_overlay(window, cx);
                let n = self.doc.project.bible.of(kind).count() + 1;
                let name = match kind {
                    Kind::Character => format!("Character {n}"),
                    Kind::Place => format!("Place {n}"),
                    _ => format!("Thing {n}"),
                };
                let entry = self.edit(cx, |p| {
                    let id = p.add_entry(kind, &name);
                    p.toggle_cast(beat, id);
                    id
                });
                self.page = Page::Bible;
                self.begin_typing(Target::EntryName(entry), true, window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn cast_overlay(&self, beat: BeatId, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.castable();
        let found = self.doc.project.story.beats.iter().find(|b| b.id == beat);
        let cast = found.map(|b| b.cast.clone()).unwrap_or_default();
        let title = found.map_or_else(String::new, |b| b.title.clone());
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut kind = None;
        for (i, e) in entries.iter().enumerate() {
            // A small heading where each kind begins.
            if kind != Some(e.kind) {
                kind = Some(e.kind);
                rows.push(
                    div()
                        .px(px(10.0))
                        .pt(px(if i == 0 { 2.0 } else { 8.0 }))
                        .pb(px(2.0))
                        .child(caps(e.kind.plural()))
                        .into_any_element(),
                );
            }
            let on = cast.contains(&e.id);
            rows.push(
                div()
                    .id(("cast-row", i))
                    .h(px(36.0))
                    .px(px(10.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .cursor_pointer()
                    .when(i == index, |d| d.bg(Theme::lane()))
                    .hover(|s| s.bg(Theme::lane()))
                    .child(self.face(e, 24.0, format!("cast-face-{}", e.id.0)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(Theme::text())
                                    .child(e.name.clone()),
                            )
                            .when(!e.words.trim().is_empty(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.5))
                                        .text_color(Theme::overlay())
                                        .child(e.words.trim().to_string()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .size(px(16.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(on, |d| d.child(icon("check", Theme::accent()).size(px(15.0)))),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.cast_pick(beat, i, window, cx)))
                    .into_any_element(),
            );
        }
        let base = entries.len();
        for (n, (label, glyph)) in [
            ("New character in this beat", "plus"),
            ("New place", "plus"),
            ("New thing", "plus"),
        ]
        .into_iter()
        .enumerate()
        {
            let at = base + n;
            rows.push(
                div()
                    .id(("cast-new", n))
                    .h(px(30.0))
                    .px(px(10.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .cursor_pointer()
                    .when(index == at, |d| d.bg(Theme::lane()))
                    .hover(|s| s.bg(Theme::lane()))
                    .child(icon(glyph, Theme::overlay()).size(px(12.0)))
                    .child(div().text_size(px(12.0)).text_color(Theme::subtext()).child(label))
                    .on_click(cx.listener(move |this, _, window, cx| this.cast_pick(beat, at, window, cx)))
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(44.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(Theme::rule())
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.0))
                            .text_color(Theme::text())
                            .child(format!("In “{title}”")),
                    ),
            )
            .when(entries.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(16.0))
                        .pt(px(12.0))
                        .text_size(px(11.5))
                        .line_height(px(16.0))
                        .text_color(Theme::overlay())
                        .child("The bible is empty. Start with who is in this beat; give them pictures and words on the Bible page."),
                )
            })
            .child(
                div()
                    .id("cast-rows")
                    .max_h(px(380.0))
                    .overflow_y_scroll()
                    .p(px(6.0))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .child(
                div()
                    .h(px(30.0))
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(Theme::rule())
                    .text_size(px(10.5))
                    .text_color(Theme::overlay())
                    .child("↑↓ move  ·  ⏎ in or out  ·  esc done"),
            )
            .into_any_element()
    }

    // Takes, on a beat card.

    /// A beat's takes: each a picture with what made it beneath, the one
    /// laid in ringed, a pending one breathing with how long it has run.
    pub(super) fn takes_strip(&mut self, beat: &Beat, cx: &mut Context<Self>) -> Option<AnyElement> {
        if beat.takes.is_empty() {
            return None;
        }
        let chosen = self.doc.project.chosen_take(beat.id);
        let now = now();
        let tiles: Vec<AnyElement> = beat
            .takes
            .iter()
            .enumerate()
            .map(|(i, take)| {
                let tid = take.id;
                let on = chosen == Some(tid);
                let thumb = take
                    .media
                    .and_then(|m| self.doc.project.media(m))
                    .and_then(|m| self.library.get(&m.hash))
                    .and_then(|a| a.thumb.clone());
                let pending = take.media.is_none();
                let waited = (take.started > 0).then(|| now.saturating_sub(take.started));
                let stale = pending && waited.is_some_and(|w| w > STALE);
                let group: SharedString = format!("take-{}", tid.0).into();
                let origin = &take.origin;
                // Beneath: the model, shortened; or how long it has run.
                let caption = if pending {
                    match waited {
                        Some(_) if stale => "lost?".to_string(),
                        Some(w) => format!("{}:{:02}", w / 60, w % 60),
                        None => "making".to_string(),
                    }
                } else if !origin.model.is_empty() {
                    origin.model.replace("seedance_", "S").replace('_', ".")
                } else {
                    format!("Take {}", i + 1)
                };
                let about = {
                    let mut lines = vec![format!(
                        "Take {}{}",
                        i + 1,
                        if stale {
                            " · no footage after 30 minutes: land it, or remove it"
                        } else if pending {
                            " · being made"
                        } else if on {
                            " · laid in"
                        } else {
                            " · click to lay it in"
                        }
                    )];
                    let made = [origin.tool.as_str(), origin.model.as_str(), origin.mode.as_str()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    if !made.is_empty() {
                        lines.push(made);
                    }
                    if !origin.prompt.is_empty() {
                        let p: String = origin.prompt.chars().take(200).collect();
                        lines.push(if origin.prompt.chars().count() > 200 {
                            format!("{p}…")
                        } else {
                            p
                        });
                    }
                    lines.join("\n")
                };
                let picture = div()
                    .id(("take", tid.0 as usize))
                    .group(group.clone())
                    .relative()
                    .w(px(64.0))
                    .h(px(40.0))
                    .rounded(px(5.0))
                    .overflow_hidden()
                    .bg(if pending { Theme::surface0() } else { Theme::crust() })
                    .border_2()
                    .border_color(if on {
                        Theme::accent()
                    } else if pending {
                        Theme::surface2()
                    } else {
                        Theme::edge()
                    })
                    .cursor_pointer()
                    .map(|d| match thumb {
                        Some(image) => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
                        None if pending => d
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(if stale { "close" } else { "film" }, Theme::subtext()).size(px(13.0))),
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
                            .id(("take-remove", tid.0 as usize))
                            .absolute()
                            .top(px(2.0))
                            .right(px(2.0))
                            .size(px(14.0))
                            .rounded(px(3.0))
                            .bg(Theme::scrim())
                            .flex()
                            .items_center()
                            .justify_center()
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .child(icon("close", Theme::text()).size(px(8.0)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.edit(cx, |p| p.remove_take(tid));
                            })),
                    )
                    .tooltip(tip(about, None))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(clip) = this.edit(cx, |p| p.choose_take(tid)) {
                            this.selection = vec![clip];
                        }
                        cx.notify();
                    }));
                // Breathing while it is made (until it looks lost).
                let picture = if pending && !stale {
                    picture
                        .with_animation(
                            ("take-pulse", tid.0 as usize),
                            Animation::new(Duration::from_millis(1600))
                                .repeat()
                                .with_easing(pulsating_between(0.55, 1.0)),
                            |d, delta| d.opacity(delta),
                        )
                        .into_any_element()
                } else {
                    picture.into_any_element()
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(picture)
                    .child(
                        div()
                            .w(px(64.0))
                            .truncate()
                            .text_size(px(9.5))
                            .font_family(MONO_FONT)
                            .text_color(if on { Theme::text() } else { Theme::overlay() })
                            .child(caption),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(caps("Takes"))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(Theme::overlay())
                                .child("click one to lay it in"),
                        ),
                )
                .child(div().flex().flex_wrap().gap(px(5.0)).children(tiles))
                .into_any_element(),
        )
    }

    pub(super) fn take_json(&self, take: &Take) -> serde_json::Value {
        serde_json::json!({
            "take": take.id.0,
            "media": take.media.map(|m| m.0),
            "pending": take.media.is_none(),
            "started": take.started,
            "stale": take.media.is_none() && take.started > 0 && now().saturating_sub(take.started) > STALE,
            "skip": secs(take.skip),
            "origin": take.origin,
        })
    }

    // Briefs.

    /// Writes a beat's brief to `dir` off the main thread (its reference
    /// pictures, numbered as the outline names them, and the frames just
    /// before, at the start, at the end and just after), then calls `done`
    /// with the brief as JSON, files in place.
    pub(super) fn write_brief(
        &mut self,
        beat: BeatId,
        dir: PathBuf,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Editor, Result<serde_json::Value, String>, &mut Context<Editor>) + 'static,
    ) {
        use serde_json::json;
        let Some(brief) = self.doc.project.brief(beat) else {
            return done(self, Err("no such beat".into()), cx);
        };
        let Some((start, end)) = self.doc.project.story.span(beat) else {
            return done(self, Err("no such beat".into()), cx);
        };
        // What to copy: from the cache (or the project file) to the folder.
        let mut copies: Vec<(Option<PathBuf>, PathBuf)> = Vec::new();
        for p in &brief.images {
            let n = p.tag.trim_start_matches("@Image");
            let to = dir.join(format!("{n:0>2}-{}.{}", slug(&p.of), p.still.ext));
            copies.push((self.still_file(&p.still), to));
        }
        for (i, p) in brief.more_pictures.iter().enumerate() {
            let to = dir.join(format!("more-{:02}-{}.{}", i + 1, slug(&p.of), p.still.ext));
            copies.push((self.still_file(&p.still), to));
        }
        // The frames around the beat, planned here and drawn in the background.
        let frame = self.doc.project.frame();
        let length = self.doc.project.duration();
        let mut stills = Vec::new();
        for (key, t) in [
            ("before", start - frame),
            ("first", start),
            ("last", end - frame),
            ("after", end),
        ] {
            if t < 0 || t >= length || self.doc.project.visual_stack(t).is_empty() {
                continue;
            }
            let file = dir.join(format!("frame-{key}.png"));
            if let Ok(plan) = export::plan_still(&self.doc.project, &self.resolver(), t, &file) {
                stills.push((key, plan, file));
            }
        }
        let mut out = match serde_json::to_value(&brief) {
            Ok(v) => v,
            Err(e) => return done(self, Err(e.to_string()), cx),
        };
        self.working += 1;
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_spawn(async move {
                    // A fresh folder: nothing left from an earlier brief.
                    let _ = std::fs::remove_dir_all(&dir);
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let copied: Vec<Option<PathBuf>> = copies
                        .into_iter()
                        .map(|(from, to)| from.and_then(|f| std::fs::copy(&f, &to).ok().map(|_| to)))
                        .collect();
                    let mut frames = json!({});
                    for (key, plan, file) in stills {
                        let drawn = ffmpeg::export_command()
                            .ok()
                            .and_then(|mut c| c.args(&plan.args).output().ok())
                            .is_some_and(|o| o.status.success());
                        if drawn {
                            frames[key] = json!(file);
                        }
                    }
                    Ok::<_, String>((copied, frames, dir))
                })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                let result = written.map(|(copied, frames, dir)| {
                    let images = out["images"].as_array().map_or(0, Vec::len);
                    for (i, file) in copied.into_iter().enumerate() {
                        let slot = if i < images {
                            &mut out["images"][i]
                        } else {
                            &mut out["more_pictures"][i - images]
                        };
                        // Missing (not yet unpacked): said, never renumbered.
                        slot["file"] = json!(file);
                    }
                    out["frames"] = frames;
                    out["dir"] = json!(dir);
                    out
                });
                done(editor, result, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Where a beat's brief is written unless told otherwise.
    pub(super) fn brief_dir(beat: BeatId) -> PathBuf {
        dirs().cache.join("briefs").join(format!("beat-{}", beat.0))
    }

    // ctl.

    /// `splice ctl bible …`.
    pub(super) fn ctl_bible(
        &mut self,
        request: &serde_json::Value,
        reply: std::sync::mpsc::Sender<String>,
        cx: &mut Context<Self>,
    ) -> Option<serde_json::Value> {
        use crate::ctl::fail;
        use serde_json::{Value, json};
        let text = |key: &str| request.get(key).and_then(Value::as_str).map(str::to_string);
        let entry_json = |e: &Entry, this: &Self| {
            json!({
                "entry": e.id.0,
                "kind": e.kind,
                "name": e.name,
                "words": e.words,
                "pictures": e.pictures.iter().filter_map(|s| this.still_file(s)).collect::<Vec<_>>(),
                "beats": this.doc.project.cast_in(e.id).iter().map(|b| b.0).collect::<Vec<_>>(),
            })
        };
        let find = |this: &Self| -> Result<EntryId, String> {
            let key = request
                .get("entry")
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                .ok_or("name the entry with --entry ID|NAME (`bible read` lists them)")?;
            this.doc
                .project
                .bible
                .find(&key)
                .map(|e| e.id)
                .ok_or(format!("the bible has no entry {key}"))
        };
        match request.get("sub").and_then(Value::as_str).unwrap_or("read") {
            "read" => Some(json!({
                "ok": true,
                "entries": self.doc.project.bible.entries.iter().map(|e| entry_json(e, self)).collect::<Vec<_>>(),
            })),
            "add" => {
                let Some(kind) = text("kind").as_deref().and_then(Kind::parse) else {
                    return Some(fail("give --kind character|place|thing|look"));
                };
                let Some(name) = text("name").filter(|n| !n.trim().is_empty()) else {
                    return Some(fail("give --name NAME"));
                };
                if self.doc.project.bible.find(name.trim()).is_some() {
                    return Some(fail(format!(
                        "the bible has {} already; names must differ (`bible set` changes it)",
                        name.trim()
                    )));
                }
                if kind == Kind::Look && self.doc.project.bible.of(Kind::Look).next().is_some() {
                    return Some(fail("the bible has its look already: change it with `bible set`"));
                }
                let words = text("words").unwrap_or_default();
                let id = self.edit(cx, |p| {
                    let id = p.add_entry(kind, name.trim());
                    p.set_entry(id, |e| e.words = words);
                    id
                });
                if let Some(image) = text("image") {
                    self.add_entry_pictures(id, vec![PathBuf::from(image)], Some(reply), cx);
                    return None;
                }
                Some(json!({ "ok": true, "entry": id.0 }))
            }
            "set" => {
                let id = match find(self) {
                    Ok(id) => id,
                    Err(e) => return Some(fail(e)),
                };
                let (name, words) = (text("name"), text("words"));
                if name.is_none() && words.is_none() {
                    return Some(fail("give --name and/or --words"));
                }
                if let Some(n) = &name
                    && self.doc.project.bible.find(n).is_some_and(|e| e.id != id)
                {
                    return Some(fail(format!("the bible has {n} already; names must differ")));
                }
                self.edit(cx, |p| {
                    p.set_entry(id, |e| {
                        if let Some(n) = name {
                            e.name = n;
                        }
                        if let Some(w) = words {
                            e.words = w;
                        }
                    })
                });
                Some(json!({ "ok": true }))
            }
            "picture" => {
                let id = match find(self) {
                    Ok(id) => id,
                    Err(e) => return Some(fail(e)),
                };
                if let Some(i) = text("remove").and_then(|r| r.trim().parse::<u64>().ok()) {
                    self.edit(cx, |p| {
                        p.set_entry(id, |e| {
                            if (i as usize) < e.pictures.len() {
                                e.pictures.remove(i as usize);
                            }
                        })
                    });
                    return Some(json!({ "ok": true }));
                }
                let Some(image) = text("image") else {
                    return Some(fail("give --image FILE to add, or --remove I (0 is the first picture)"));
                };
                if !Path::new(&image).exists() {
                    return Some(fail(format!("{image} does not exist")));
                }
                self.add_entry_pictures(id, vec![PathBuf::from(image)], Some(reply), cx);
                None
            }
            "remove" => match find(self) {
                Ok(id) => {
                    self.typing = None;
                    self.edit(cx, |p| p.remove_entry(id));
                    Some(json!({ "ok": true }))
                }
                Err(e) => Some(fail(e)),
            },
            other => Some(fail(format!(
                "unknown `bible {other}`: read, add, set, picture or remove"
            ))),
        }
    }

    /// `splice ctl story cast|brief|take …`.
    pub(super) fn ctl_plan(
        &mut self,
        request: &serde_json::Value,
        reply: std::sync::mpsc::Sender<String>,
        cx: &mut Context<Self>,
    ) -> Option<serde_json::Value> {
        use crate::ctl::fail;
        use serde_json::{Value, json};
        let text = |key: &str| request.get(key).and_then(Value::as_str).map(str::to_string);
        let flag = |key: &str| request.get(key).and_then(Value::as_bool) == Some(true);
        let beat = |this: &Self| -> Result<BeatId, String> {
            let id = request
                .get("beat")
                .and_then(Value::as_u64)
                .map(BeatId)
                .or(this.beat)
                .ok_or("name the beat with --beat B")?;
            this.doc
                .project
                .story
                .index(id)
                .map(|_| id)
                .ok_or(format!("no beat {}", id.0))
        };
        let entry = |this: &Self, key: &str| -> Result<EntryId, String> {
            this.doc
                .project
                .bible
                .find(key)
                .map(|e| e.id)
                .ok_or(format!("the bible has no entry {key}"))
        };
        let origin = || Origin {
            tool: text("tool").unwrap_or_default(),
            model: text("model").unwrap_or_default(),
            mode: text("mode").unwrap_or_default(),
            prompt: text("prompt").unwrap_or_default(),
            references: text("refs")
                .map(|r| {
                    r.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            job: text("job").unwrap_or_default(),
            note: text("note").unwrap_or_default(),
        };
        match request.get("sub").and_then(Value::as_str).unwrap_or("") {
            "cast" => {
                let b = match beat(self) {
                    Ok(b) => b,
                    Err(e) => return Some(fail(e)),
                };
                for (key, join) in [("add", true), ("remove", false)] {
                    if let Some(name) = text(key) {
                        let id = match entry(self, &name) {
                            Ok(id) => id,
                            Err(e) => return Some(fail(e)),
                        };
                        let present = self
                            .doc
                            .project
                            .story
                            .beats
                            .iter()
                            .any(|x| x.id == b && x.cast.contains(&id));
                        if present != join {
                            self.edit(cx, |p| p.toggle_cast(b, id));
                        }
                    }
                }
                let cast: Vec<Value> = self
                    .doc
                    .project
                    .story
                    .beats
                    .iter()
                    .find(|x| x.id == b)
                    .map(|x| x.cast.clone())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|e| self.doc.project.bible.entry(*e))
                    .map(|e| json!({ "entry": e.id.0, "name": e.name }))
                    .collect();
                Some(json!({ "ok": true, "beat": b.0, "cast": cast }))
            }
            "brief" => {
                let b = match beat(self) {
                    Ok(b) => b,
                    Err(e) => return Some(fail(e)),
                };
                let dir = text("dir").map(PathBuf::from).unwrap_or_else(|| Self::brief_dir(b));
                self.write_brief(b, dir, cx, move |_, result, _| {
                    let answer = match result {
                        Ok(brief) => json!({ "ok": true, "brief": brief }),
                        Err(e) => fail(e),
                    };
                    let _ = reply.send(answer.to_string());
                });
                None
            }
            "take" => {
                let action = text("action").unwrap_or_default();
                let take = || request.get("take").and_then(Value::as_u64).map(TakeId);
                match action.as_str() {
                    // A generation under way: a take with no footage yet.
                    "pending" => {
                        let b = match beat(self) {
                            Ok(b) => b,
                            Err(e) => return Some(fail(e)),
                        };
                        let id = self.edit(cx, |p| p.add_take(b, None, origin(), now()));
                        Some(json!({ "ok": true, "take": id.map(|t| t.0), "beat": b.0 }))
                    }
                    // Footage for a take: a new take, or a pending one landing.
                    "add" | "land" => {
                        let Some(file) = text("video") else {
                            return Some(fail("give the footage with --video FILE"));
                        };
                        let path = PathBuf::from(&file);
                        if !path.exists() {
                            return Some(fail(format!("{file} does not exist")));
                        }
                        // Read it first: nothing is made for footage that cannot come in.
                        if let Err(e) = ffmpeg::probe(&path) {
                            return Some(fail(format!("{file} is not footage Splice can read: {e}")));
                        }
                        let skip = if flag("seam") {
                            SEAM_FRAMES * self.doc.project.frame()
                        } else {
                            0
                        };
                        let choose = !flag("keep");
                        let landing = if action == "land" {
                            match take().filter(|t| self.doc.project.take(*t).is_some()) {
                                Some(take) => Landing::Land {
                                    take,
                                    origin: origin(),
                                    skip,
                                    choose,
                                },
                                None => return Some(fail("name the pending take with --take T")),
                            }
                        } else {
                            match beat(self) {
                                Ok(beat) => Landing::New {
                                    beat,
                                    origin: origin(),
                                    skip,
                                    choose,
                                },
                                Err(e) => return Some(fail(e)),
                            }
                        };
                        self.take_after_import.insert(path.clone(), landing);
                        self.import_paths(vec![path], None, Some(reply), cx);
                        None
                    }
                    // A generation that failed or was abandoned.
                    "fail" => match take().filter(|t| self.doc.project.take(*t).is_some_and(|t| t.media.is_none())) {
                        Some(t) => {
                            self.edit(cx, |p| p.remove_take(t));
                            Some(json!({ "ok": true }))
                        }
                        None => Some(fail("name a pending take with --take T")),
                    },
                    "choose" => match take().filter(|t| self.doc.project.take(*t).is_some()) {
                        Some(t) => match self.edit(cx, |p| p.choose_take(t)) {
                            Some(clip) => {
                                self.selection = vec![clip];
                                Some(self.edited_with(json!({ "clip": clip.0 })))
                            }
                            None => Some(fail("that take has no footage yet")),
                        },
                        None => Some(fail("name the take with --take T (`story read` lists them)")),
                    },
                    "remove" => match take().filter(|t| self.doc.project.take(*t).is_some()) {
                        Some(t) => {
                            self.edit(cx, |p| p.remove_take(t));
                            Some(json!({ "ok": true }))
                        }
                        None => Some(fail("name the take with --take T")),
                    },
                    _ => Some(fail("story take pending | add | land | fail | choose | remove")),
                }
            }
            other => Some(fail(format!("unknown `story {other}`"))),
        }
    }
}

/// A file-name-safe form of a name.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "picture".into()
    } else {
        s.chars().take(32).collect()
    }
}
