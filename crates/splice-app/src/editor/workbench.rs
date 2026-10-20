//! The editors of made media, in place of the viewer and inspector: a piano
//! roll for a score, its code beside the picture for a scene. Both play with
//! the timeline, so the music lines up with the pictures it is for.

use super::render::{caps, icon, icon_button, panel, tip};
use super::*;
use gpui::{FontWeight, Hsla};
use splice_core::model::{Clip, Recipe, Scene};
use splice_core::score::{GM, PPQ, Phrase, Pitch, Score, midi_name, pitch_to_midi};
use splice_core::time::from_secs;
use std::cell::RefCell;

/// What the workbench has open, and what is selected in it.
pub struct Bench {
    pub media: MediaId,
    /// Selected notes: (part, phrase, note).
    pub notes: Vec<(usize, usize, usize)>,
    /// Where each part's roll was drawn, for the mouse.
    pub rolls: Rc<RefCell<HashMap<usize, Roll>>>,
}

/// A part's roll as drawn: its box and what fits in it.
#[derive(Clone, Copy)]
pub struct Roll {
    pub bounds: Bounds<Pixels>,
    pub low: u8,
    pub high: u8,
    pub beats: f64,
}

impl Roll {
    fn beat_px(&self) -> f32 {
        f32::from(self.bounds.size.width) / self.beats.max(1.0) as f32
    }

    fn key_px(&self) -> f32 {
        f32::from(self.bounds.size.height) / (self.high - self.low + 1) as f32
    }

    fn x(&self, beat: f64) -> Pixels {
        self.bounds.left() + px(beat as f32 * self.beat_px())
    }

    fn y(&self, key: u8) -> Pixels {
        self.bounds.top() + px((self.high as f32 - key as f32) * self.key_px())
    }

    fn beat_at(&self, x: Pixels) -> f64 {
        (f32::from(x - self.bounds.left()) / self.beat_px()) as f64
    }

    fn key_at(&self, y: Pixels) -> u8 {
        let k = self.high as f32 - (f32::from(y - self.bounds.top()) / self.key_px()).floor();
        k.clamp(0.0, 127.0) as u8
    }
}

/// A note dragged in the roll.
pub struct NoteGrab {
    pub part: usize,
    pub from: Point<Pixels>,
    pub roll: Roll,
    /// The held notes as they were: (phrase, note, at, duration, key).
    pub start: Vec<(usize, usize, f64, f64, u8)>,
    /// Stretching from the right edge rather than moving.
    pub resize: bool,
    pub before: Project,
}

/// A note as the roll shows it, where it sounds.
struct Shown {
    phrase: usize,
    note: usize,
    beat: f64,
    length: f64,
    key: u8,
    velocity: f64,
    /// A repeat of its phrase: drawn fainter, edits change every repeat.
    echo: bool,
}

/// No vermilion: in the roll that is the playhead's.
const PART_COLORS: [u32; 7] = [0x6c9ef8, 0xf2c14e, 0x86cfc0, 0xc792ea, 0x9ccc65, 0x7fc8e8, 0xe59ac0];
/// Grid the mouse snaps notes to, in beats.
const SNAP: f64 = 0.25;
const LANE_HEADER: f32 = 176.0;

fn shown(part: &splice_core::score::Part) -> Vec<Shown> {
    let mut out = Vec::new();
    for (p, phrase) in part.phrases.iter().enumerate() {
        let length = phrase
            .length
            .unwrap_or_else(|| phrase.notes.iter().map(|n| n.at + n.duration).fold(0.0, f64::max));
        for r in 0..phrase.repeat.unwrap_or(1).min(512) {
            for (n, note) in phrase.notes.iter().enumerate() {
                let Ok(key) = pitch_to_midi(&note.pitch) else { continue };
                let key = (key as i32 + phrase.transpose.unwrap_or(0)).clamp(0, 127) as u8;
                out.push(Shown {
                    phrase: p,
                    note: n,
                    beat: phrase.at.unwrap_or(0.0) + r as f64 * length + note.at,
                    length: note.duration,
                    key,
                    velocity: note.velocity.unwrap_or(80.0),
                    echo: r > 0,
                });
            }
        }
    }
    out
}

fn snap(beat: f64) -> f64 {
    (beat / SNAP).round() * SNAP
}

fn rgb(color: u32) -> Hsla {
    gpui::rgb(color).into()
}

impl Editor {
    pub(super) fn open_workbench(&mut self, media: MediaId, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        if self.workbench.as_ref().is_some_and(|b| b.media == media) {
            return;
        }
        self.workbench = Some(Bench {
            media,
            notes: Vec::new(),
            rolls: Rc::default(),
        });
        self.bin_selection = Some(media);
        // Bring the playhead onto it, so what plays is what is open.
        let project = &self.doc.project;
        let on = |c: &Clip| c.media == media && c.generator.is_none();
        let under = project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .any(|c| on(c) && c.contains(self.player.position));
        if !under
            && let Some(first) = project
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| on(c))
                .min_by_key(|c| c.start)
        {
            let start = first.start;
            self.seek(start, cx);
        }
        cx.notify();
    }

    pub(super) fn close_workbench(&mut self, cx: &mut Context<Self>) {
        self.workbench = None;
        cx.notify();
    }

    /// The open score, if the workbench holds one.
    fn bench_score(&self) -> Option<(MediaId, Score)> {
        let id = self.workbench.as_ref()?.media;
        match &self.doc.project.media(id)?.recipe {
            Some(Recipe::Score(score)) => Some((id, score.clone())),
            _ => None,
        }
    }

    fn change_score(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Score)) {
        let Some((id, mut score)) = self.bench_score() else {
            return;
        };
        f(&mut score);
        if score.compile().is_ok() {
            self.edit(cx, |p| p.set_recipe(id, Recipe::Score(score)));
        }
    }

    /// Where the playhead is in the open media, in its own seconds.
    fn bench_time(&self, media: MediaId) -> Option<f64> {
        let t = self.player.position;
        self.doc
            .project
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .find(|c| c.media == media && c.generator.is_none() && c.contains(t))
            .map(|c| secs(c.source_at(t)))
    }

    // The panel.

    pub(super) fn render_workbench(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = self.workbench.as_ref().map(|b| b.media) else {
            return div().into_any_element();
        };
        let Some(media) = self.doc.project.media(id).cloned() else {
            self.workbench = None;
            return div().into_any_element();
        };
        let body = match media.recipe.clone() {
            Some(Recipe::Score(score)) => self.score_body(id, &score, cx).into_any_element(),
            Some(Recipe::Scene(scene)) => self.scene_body(id, &scene, window, cx).into_any_element(),
            None => div().into_any_element(),
        };
        panel()
            .id("workbench")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.bench_header(&media, cx))
            .child(body)
            .into_any_element()
    }

    fn bench_header(&mut self, media: &Media, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let id = media.id;
        let is_scene = matches!(media.recipe, Some(Recipe::Scene(_)));
        let (glyph, kind) = if is_scene {
            ("scene", "Scene")
        } else {
            ("score", "Score")
        };
        let state: (String, bool) = if let Some(render) = self.render_of(id) {
            let p = render.progress();
            (
                if is_scene && p > 0.0 {
                    format!("rendering {:.0}%", p * 100.0)
                } else {
                    "rendering…".into()
                },
                false,
            )
        } else if let Some(e) = self.made.errors.get(&id) {
            (e.lines().next().unwrap_or("could not render").to_string(), true)
        } else {
            (format!("{:.1}s", secs(media.duration)), false)
        };
        let playing = self.player.is_playing();
        let button = |id: &'static str, label: &'static str, glyph: &'static str| {
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
        div()
            .h(px(52.0))
            .flex_none()
            .px(px(14.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .border_b_1()
            .border_color(Theme::edge())
            .child(
                div()
                    .size(px(30.0))
                    .rounded(px(8.0))
                    .bg(Theme::lane())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(glyph, Theme::subtext()).size(px(15.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(Theme::text())
                            .truncate()
                            .child(media.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.0))
                            .text_size(px(10.5))
                            .child(div().text_color(Theme::overlay()).child(kind))
                            .child(div().text_color(Theme::surface2()).child("·"))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(if state.1 { Theme::error() } else { Theme::overlay() })
                                    .child(state.0),
                            ),
                    ),
            )
            .child(
                icon_button("bench-play", if playing { "pause" } else { "play" }, true)
                    .tooltip(tip(if playing { "Pause" } else { "Play" }, Some("Space")))
                    .on_click(cx.listener(|this, _, window, cx| this.play_pause(&PlayPause, window, cx))),
            )
            .when(is_scene, |d| {
                d.child(
                    button("bench-code", "Edit", "scene")
                        .tooltip(tip("Edit the scene in a text editor; each save re-renders here", None))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_scene_code(id, cx))),
                )
            })
            .child(
                button("bench-ask", "Ask", "message")
                    .tooltip(tip(
                        if is_scene {
                            "Ask the assistant to change this scene"
                        } else {
                            "Ask the assistant to compose or change this music"
                        },
                        Some("⇧⌘A"),
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.ask_agent(&AskAgent, window, cx))),
            )
            .child(
                icon_button("bench-close", "close", true)
                    .tooltip(tip("Close the editor", Some("Esc")))
                    .on_click(cx.listener(|this, _, _, cx| this.close_workbench(cx))),
            )
    }

    // Score.

    fn score_body(&mut self, id: MediaId, score: &Score, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let compiled = score.compile().ok();
        let beats = compiled
            .as_ref()
            .map_or(16.0, |c| c.length_ticks as f64 / PPQ as f64)
            .max(4.0);
        let bar_ticks: Vec<i64> = compiled
            .as_ref()
            .map(|c| c.bars(c.length_ticks).into_iter().map(|(t, _)| t).collect())
            .unwrap_or_default();
        let playhead = self
            .bench_time(id)
            .and_then(|s| compiled.as_ref().map(|c| c.tick_at(s) as f64 / PPQ as f64));
        let bpm = match &score.tempo {
            splice_core::score::Tempo::Constant(b) => *b,
            splice_core::score::Tempo::Map(m) => m.first().map_or(120.0, |t| t.bpm),
        };
        let meter = match &score.meter {
            Some(splice_core::score::Meter::Constant([n, d])) => format!("{n}/{d}"),
            Some(splice_core::score::Meter::Map(m)) => m
                .first()
                .map_or("4/4".into(), |m| format!("{}/{}", m.meter[0], m.meter[1])),
            None => "4/4".into(),
        };
        let tempo_map = matches!(score.tempo, splice_core::score::Tempo::Map(_));

        let stepper = |id: &'static str, glyph: &'static str, delta: f64, cx: &mut Context<Self>| {
            icon_button(id, glyph, !tempo_map).on_click(cx.listener(move |this, _, _, cx| {
                this.change_score(cx, |s| {
                    if let splice_core::score::Tempo::Constant(b) = &mut s.tempo {
                        *b = (*b + delta).clamp(30.0, 300.0);
                    }
                });
            }))
        };
        let slower = stepper("tempo-down", "minus", -4.0, cx);
        let faster = stepper("tempo-up", "plus", 4.0, cx);

        let selected = self.workbench.as_ref().map(|b| b.notes.clone()).unwrap_or_default();
        let rolls = self.workbench.as_ref().map(|b| b.rolls.clone()).unwrap_or_default();
        let lanes: Vec<AnyElement> = score
            .parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let notes = shown(part);
                let color = PART_COLORS[i % PART_COLORS.len()];
                let (mut low, mut high) = notes
                    .iter()
                    .fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
                if notes.is_empty() {
                    (low, high) = (55, 72);
                }
                // A little room above and below, and at least an octave.
                low = low.saturating_sub(2);
                high = (high + 2).min(127);
                if high - low < 12 {
                    let pad = (12 - (high - low)).div_ceil(2);
                    low = low.saturating_sub(pad);
                    high = (high + pad).min(127);
                }
                let height = ((high - low + 1) as f32 * 7.0).clamp(84.0, 230.0);
                let sel: Vec<(usize, usize)> = selected.iter().filter(|s| s.0 == i).map(|s| (s.1, s.2)).collect();
                self.lane(
                    id,
                    i,
                    part,
                    notes,
                    color,
                    (low, high),
                    height,
                    beats,
                    &bar_ticks,
                    playhead,
                    sel,
                    rolls.clone(),
                    cx,
                )
            })
            .collect();

        let parts = score.parts.len();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(44.0))
                    .flex_none()
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .border_b_1()
                    .border_color(Theme::edge())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(caps("Tempo"))
                            .child(slower)
                            .child(
                                div()
                                    .font_family(MONO_FONT)
                                    .text_size(px(12.0))
                                    .text_color(Theme::text())
                                    .child(if tempo_map {
                                        format!("{bpm:.0}…")
                                    } else {
                                        format!("{bpm:.0}")
                                    }),
                            )
                            .child(div().text_size(px(10.5)).text_color(Theme::overlay()).child("bpm"))
                            .child(faster),
                    )
                    .child(
                        div().flex().items_center().gap(px(8.0)).child(caps("Meter")).child(
                            div()
                                .font_family(MONO_FONT)
                                .text_size(px(12.0))
                                .text_color(Theme::text())
                                .child(meter),
                        ),
                    )
                    .child(
                        div().flex().items_center().gap(px(8.0)).child(caps("Bars")).child(
                            div()
                                .font_family(MONO_FONT)
                                .text_size(px(12.0))
                                .text_color(Theme::text())
                                .child(bar_ticks.len().saturating_sub(1).max(1).to_string()),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("add-part")
                            .h(px(26.0))
                            .px(px(10.0))
                            .rounded(px(7.0))
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .cursor_pointer()
                            .border_1()
                            .border_color(Theme::edge())
                            .hover(|s| s.bg(Theme::lane()))
                            .child(icon("plus", Theme::subtext()).size(px(12.0)))
                            .child(div().text_size(px(11.5)).text_color(Theme::text()).child("Instrument"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.change_score(cx, |s| {
                                    let n = (1..)
                                        .find(|n| !s.parts.iter().any(|p| p.id == format!("part-{n}")))
                                        .unwrap_or(parts + 1);
                                    let mut part = s
                                        .parts
                                        .first()
                                        .cloned()
                                        .unwrap_or_else(|| Score::starter().parts[0].clone());
                                    part.id = format!("part-{n}");
                                    part.instrument = "string-ensemble-1".into();
                                    part.program = None;
                                    part.drums = false;
                                    part.mute = false;
                                    part.solo = false;
                                    part.phrases = vec![Phrase::default()];
                                    s.parts.push(part);
                                });
                            })),
                    ),
            )
            .child(
                div()
                    .id("score-lanes")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(lanes)
                    .child(
                        div()
                            .px(px(14.0))
                            .pt(px(6.0))
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(Theme::overlay())
                            .child(
                                "Click to add a note, drag to move it, drag its end to lengthen it, ⌫ to delete. \
                                 Fainter notes repeat a phrase: changing one changes every repeat.",
                            ),
                    ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn lane(
        &mut self,
        media: MediaId,
        index: usize,
        part: &splice_core::score::Part,
        notes: Vec<Shown>,
        color: u32,
        range: (u8, u8),
        height: f32,
        beats: f64,
        bars: &[i64],
        playhead: Option<f64>,
        selected: Vec<(usize, usize)>,
        rolls: Rc<RefCell<HashMap<usize, Roll>>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let program = part
            .program
            .unwrap_or_else(|| splice_core::score::program_for(&part.instrument));
        let drums = part.drums || splice_core::score::is_drums(&part.instrument);
        let sound = if drums {
            "Drum kit".to_string()
        } else {
            GM[program as usize].replace('-', " ")
        };
        let muted = part.mute;
        let solo = part.solo;
        let toggle = |id: String, label: &'static str, on: bool, tint: Hsla| {
            div()
                .id(SharedString::from(id))
                .size(px(20.0))
                .rounded(px(5.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .border_1()
                .map(|d| {
                    if on {
                        d.bg(tint).border_color(tint).text_color(Theme::ground())
                    } else {
                        d.border_color(Theme::edge())
                            .text_color(Theme::overlay())
                            .hover(|s| s.bg(Theme::lane()))
                    }
                })
                .child(label)
        };
        let bars: Vec<f64> = bars.iter().map(|t| *t as f64 / PPQ as f64).collect();
        let draw_rolls = rolls.clone();
        let roll = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let roll = Roll {
                    bounds,
                    low: range.0,
                    high: range.1,
                    beats,
                };
                draw_rolls.borrow_mut().insert(index, roll);
                // Ground, octave bands, beat and bar lines.
                window.paint_quad(gpui::fill(bounds, Theme::crust()));
                for key in range.0..=range.1 {
                    if key % 12 == 0 {
                        let y = roll.y(key) + px(roll.key_px());
                        window.paint_quad(gpui::fill(
                            Bounds::new(point(bounds.left(), y - px(0.5)), size(bounds.size.width, px(1.0))),
                            Theme::lane(),
                        ));
                    }
                }
                let mut beat = 0.0;
                while beat <= beats {
                    let x = roll.x(beat);
                    let bar = bars.iter().any(|b| (b - beat).abs() < 1e-6);
                    window.paint_quad(gpui::fill(
                        Bounds::new(point(x, bounds.top()), size(px(1.0), bounds.size.height)),
                        if bar { Theme::surface1() } else { Theme::lane() },
                    ));
                    beat += 1.0;
                }
                let base = rgb(color);
                for n in &notes {
                    let x = roll.x(n.beat);
                    let w = (roll.x(n.beat + n.length) - x).max(px(3.0)) - px(1.0);
                    let h = px(roll.key_px().max(3.0) - 1.0);
                    let rect = Bounds::new(point(x, roll.y(n.key)), size(w, h));
                    let picked = selected.contains(&(n.phrase, n.note));
                    let mut fill = base;
                    fill.a = if muted {
                        0.25
                    } else if n.echo {
                        0.45
                    } else {
                        0.55 + 0.45 * (n.velocity / 127.0) as f32
                    };
                    window.paint_quad(quad(
                        rect,
                        Corners::all(px(2.0)),
                        fill,
                        px(if picked { 1.5 } else { 0.0 }),
                        Hsla::from(Theme::text()),
                        Default::default(),
                    ));
                }
                if let Some(at) = playhead {
                    let x = roll.x(at);
                    window.paint_quad(gpui::fill(
                        Bounds::new(point(x, bounds.top()), size(px(1.5), bounds.size.height)),
                        Theme::accent(),
                    ));
                }
            },
        )
        .size_full();

        div()
            .id(("lane", index))
            .mx(px(10.0))
            .h(px(height))
            .flex_none()
            .flex()
            .rounded(px(8.0))
            .overflow_hidden()
            .border_1()
            .border_color(Theme::edge())
            .child(
                div()
                    .w(px(LANE_HEADER))
                    .flex_none()
                    .h_full()
                    .px(px(10.0))
                    .py(px(8.0))
                    .bg(Theme::lane())
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .child(div().size(px(8.0)).rounded_full().flex_none().bg(rgb(color)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Theme::text())
                                    .child(part.id.clone()),
                            )
                            .child(
                                toggle(format!("mute-{index}"), "M", muted, Theme::overlay().into())
                                    .tooltip(tip("Mute", None))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.change_score(cx, |s| s.parts[index].mute = !s.parts[index].mute);
                                    })),
                            )
                            .child(
                                toggle(format!("solo-{index}"), "S", solo, rgb(0xf2c14e))
                                    .tooltip(tip("Solo", None))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.change_score(cx, |s| s.parts[index].solo = !s.parts[index].solo);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id(("instrument", index))
                            .h(px(24.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(4.0))
                            .cursor_pointer()
                            .bg(Theme::crust())
                            .hover(|s| s.bg(Theme::surface0()))
                            .tooltip(tip("Choose the instrument", None))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .text_color(Theme::subtext())
                                    .child(sound),
                            )
                            .child(icon("chevron", Theme::overlay()).size(px(10.0)))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.open_instruments(media, index, window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id(("roll", index))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    .cursor_crosshair()
                    .child(roll)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| this.roll_down(index, e, window, cx)),
                    ),
            )
            .into_any_element()
    }

    fn roll_down(&mut self, part: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let Some((_, score)) = self.bench_score() else { return };
        let Some(bench) = &self.workbench else { return };
        let Some(roll) = bench.rolls.borrow().get(&part).copied() else {
            return;
        };
        let Some(p) = score.parts.get(part) else { return };
        let at = event.position;
        // The topmost note under the pointer, and whether at its right end.
        let hit = shown(p).into_iter().rev().find(|n| {
            let x0 = roll.x(n.beat);
            let x1 = roll.x(n.beat + n.length).max(x0 + px(4.0));
            let y0 = roll.y(n.key);
            at.x >= x0 && at.x <= x1 && at.y >= y0 && at.y <= y0 + px(roll.key_px())
        });
        let before = self.doc.project.clone();
        let hit_something = hit.is_some();
        let (phrase, note, resize) = match hit {
            Some(n) => {
                let end = roll.x(n.beat + n.length);
                (n.phrase, n.note, f32::from(end - at.x) < 6.0)
            }
            None => {
                // A new note on the grid, a beat long.
                let beat = (snap(roll.beat_at(at.x) - SNAP / 2.0)).max(0.0);
                let key = roll.key_at(at.y);
                let mut added = (0, 0);
                self.change_score_live(|s| {
                    let part = &mut s.parts[part];
                    let i = part
                        .phrases
                        .iter()
                        .position(|ph| {
                            ph.at.unwrap_or(0.0) == 0.0
                                && ph.repeat.unwrap_or(1) == 1
                                && ph.transpose.is_none()
                                && ph.length.is_none()
                        })
                        .unwrap_or_else(|| {
                            part.phrases.push(Phrase::default());
                            part.phrases.len() - 1
                        });
                    part.phrases[i].notes.push(splice_core::score::Note {
                        at: beat,
                        duration: 1.0,
                        pitch: Pitch::Name(midi_name(key)),
                        velocity: Some(84.0),
                    });
                    added = (i, part.phrases[i].notes.len() - 1);
                });
                (added.0, added.1, true)
            }
        };
        let shift = event.modifiers.shift || event.modifiers.platform;
        if let Some(bench) = &mut self.workbench {
            if shift {
                if !bench.notes.contains(&(part, phrase, note)) {
                    bench.notes.push((part, phrase, note));
                }
            } else if !bench.notes.contains(&(part, phrase, note)) || !hit_something {
                bench.notes = vec![(part, phrase, note)];
            }
            bench.notes.retain(|n| n.0 == part);
        }
        let Some((_, score)) = self.bench_score() else { return };
        let held: Vec<(usize, usize, f64, f64, u8)> = self
            .workbench
            .as_ref()
            .map(|b| b.notes.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(_, ph, n)| {
                let note = score.parts.get(part)?.phrases.get(ph)?.notes.get(n)?;
                Some((ph, n, note.at, note.duration, pitch_to_midi(&note.pitch).ok()?))
            })
            .collect();
        self.grab = Some(stage::Grab::Note(NoteGrab {
            part,
            from: at,
            roll,
            start: held,
            resize,
            before,
        }));
        cx.notify();
    }

    /// Changes the open score without an undo step (a drag in progress).
    fn change_score_live(&mut self, f: impl FnOnce(&mut Score)) {
        let Some((id, mut score)) = self.bench_score() else {
            return;
        };
        f(&mut score);
        self.doc.project.set_recipe(id, Recipe::Score(score));
    }

    pub(super) fn note_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(stage::Grab::Note(grab)) = &self.grab else {
            return;
        };
        let beats = snap(f32::from(at.x - grab.from.x) as f64 / grab.roll.beat_px() as f64);
        let keys = -(f32::from(at.y - grab.from.y) / grab.roll.key_px()).round() as i32;
        let (part, resize, start) = (grab.part, grab.resize, grab.start.clone());
        self.change_score_live(|s| {
            let Some(p) = s.parts.get_mut(part) else { return };
            for (ph, n, at0, dur0, key0) in &start {
                let Some(note) = p.phrases.get_mut(*ph).and_then(|ph| ph.notes.get_mut(*n)) else {
                    continue;
                };
                if resize {
                    note.duration = (dur0 + beats).max(SNAP);
                } else {
                    note.at = (at0 + beats).max(0.0);
                    note.pitch = Pitch::Name(midi_name((*key0 as i32 + keys).clamp(0, 127) as u8));
                }
            }
        });
        cx.notify();
    }

    /// Lets go of notes: the change is one undo step, and renders.
    pub(super) fn note_drop(&mut self, before: Project, cx: &mut Context<Self>) {
        self.doc.commit(before);
        self.after_edit();
        self.reconcile_made(cx);
    }

    /// Deletes the selected notes. True when there were some.
    pub(super) fn delete_notes(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(bench) = &mut self.workbench else { return false };
        if bench.notes.is_empty() {
            return false;
        }
        let mut notes = std::mem::take(&mut bench.notes);
        // Highest index first, so the others stay where they are.
        notes.sort_by(|a, b| b.cmp(a));
        self.change_score(cx, |s| {
            for (part, phrase, note) in notes {
                if let Some(ph) = s.parts.get_mut(part).and_then(|p| p.phrases.get_mut(phrase))
                    && note < ph.notes.len()
                {
                    ph.notes.remove(note);
                }
            }
            for part in &mut s.parts {
                part.phrases.retain(|ph| !ph.notes.is_empty());
                if part.phrases.is_empty() {
                    part.phrases.push(Phrase::default());
                }
            }
        });
        true
    }

    /// Esc in the workbench: drop the note selection, else close it.
    pub(super) fn bench_escape(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(bench) = &mut self.workbench else { return false };
        if bench.notes.is_empty() {
            self.close_workbench(cx);
        } else {
            bench.notes.clear();
            cx.notify();
        }
        true
    }

    // Instruments.

    fn open_instruments(&mut self, media: MediaId, part: usize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .bench_score()
            .and_then(|(_, s)| {
                s.parts.get(part).map(|p| {
                    p.program
                        .unwrap_or_else(|| splice_core::score::program_for(&p.instrument))
                })
            })
            .unwrap_or(0);
        self.overlay = Overlay::Instruments {
            media,
            part,
            query: String::new(),
            index: current as usize,
        };
        window.focus(&self.palette_focus);
        cx.notify();
    }

    /// Instruments matching what is typed: GM names, and the drum kit.
    pub(super) fn instrument_matches(query: &str) -> Vec<(Option<u8>, String)> {
        let q = query.trim().to_lowercase();
        let mut all: Vec<(Option<u8>, String)> = GM
            .iter()
            .enumerate()
            .map(|(i, name)| (Some(i as u8), name.replace('-', " ")))
            .collect();
        all.push((None, "drum kit".into()));
        all.into_iter()
            .filter(|(_, name)| q.split_whitespace().all(|w| name.contains(w)))
            .collect()
    }

    pub(super) fn choose_instrument(
        &mut self,
        part: usize,
        choice: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_score(cx, |s| {
            if let Some(p) = s.parts.get_mut(part) {
                match choice {
                    Some(program) => {
                        p.instrument = GM[program as usize].into();
                        p.program = Some(program);
                        p.drums = false;
                    }
                    None => {
                        p.instrument = "drums".into();
                        p.program = None;
                        p.drums = true;
                    }
                }
            }
        });
        self.close_overlay(window, cx);
    }

    // Scene.

    fn scene_body(
        &mut self,
        id: MediaId,
        scene: &Scene,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let error = self.made.errors.get(&id).cloned();
        let broken = error.as_deref().and_then(syntax::error_line);
        let lines: Vec<AnyElement> = scene
            .source
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let text = if line.is_empty() {
                    " ".to_string()
                } else {
                    line.replace('\t', "    ")
                };
                let paint = |t: syntax::Token| -> Hsla {
                    match t {
                        syntax::Token::Key => rgb(0x9ab4f5),
                        syntax::Token::Str => rgb(0x86cfc0),
                        syntax::Token::Number => rgb(0xf2c14e),
                        syntax::Token::Word => rgb(0xc792ea),
                        syntax::Token::Mark => Theme::overlay().into(),
                    }
                };
                let highlights: Vec<_> = syntax::json(&text)
                    .into_iter()
                    .map(|(range, t)| {
                        (
                            range,
                            gpui::HighlightStyle {
                                color: Some(paint(t)),
                                ..Default::default()
                            },
                        )
                    })
                    .collect();
                let swatches: Vec<AnyElement> = syntax::colors(&text)
                    .into_iter()
                    .map(|c| {
                        div()
                            .size(px(10.0))
                            .flex_none()
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(gpui::rgba(0xffffff33))
                            .bg(gpui::Rgba {
                                r: c[0] as f32 / 255.0,
                                g: c[1] as f32 / 255.0,
                                b: c[2] as f32 / 255.0,
                                a: c[3] as f32 / 255.0,
                            })
                            .into_any_element()
                    })
                    .collect();
                let wrong = broken == Some(i + 1);
                div()
                    .flex()
                    .items_center()
                    .when(wrong, |d| d.bg(gpui::rgba(0xef6a5526)))
                    .child(
                        div()
                            .w(px(34.0))
                            .flex_none()
                            .pr(px(10.0))
                            .flex()
                            .justify_end()
                            .text_color(if wrong { Theme::error() } else { Theme::surface2() })
                            .child((i + 1).to_string()),
                    )
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_color(Theme::subtext())
                            .child(gpui::StyledText::new(text).with_highlights(highlights)),
                    )
                    .when(!swatches.is_empty(), |d| {
                        d.child(div().ml(px(8.0)).flex().gap(px(4.0)).children(swatches))
                    })
                    .into_any_element()
            })
            .collect();
        let transparent = scene.transparent;
        let length = secs(scene.duration);
        let stage = self.render_stage(cx);
        // The presets, and the scene's own length when it is none of them.
        let mut choices = vec![3.0, 5.0, 8.0, 12.0];
        if !choices.iter().any(|s| (length - s).abs() < 0.01) {
            choices.push(length);
            choices.sort_by(f64::total_cmp);
        }
        let lengths: Vec<AnyElement> = choices
            .into_iter()
            .map(|s| {
                let on = (length - s).abs() < 0.01;
                let label = if (s - s.round()).abs() < 0.05 {
                    format!("{s:.0}s")
                } else {
                    format!("{s:.1}s")
                };
                self.chip(format!("scene-len-{s}"), &label, on, cx, move |this, cx| {
                    this.change_scene(id, cx, |sc| sc.duration = from_secs(s));
                })
                .into_any_element()
            })
            .collect();
        let backdrop = self.toggle(
            "scene-backdrop",
            "See-through background",
            transparent,
            cx,
            move |this, cx| {
                this.change_scene(id, cx, |sc| sc.transparent = !sc.transparent);
            },
        );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(
                div()
                    .w(relative(0.46))
                    .min_w(px(260.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(Theme::edge())
                    .when_some(error, |d, error| {
                        d.child(
                            div()
                                .id("scene-error")
                                .max_h(px(140.0))
                                .overflow_y_scroll()
                                .m(px(10.0))
                                .p(px(10.0))
                                .rounded(px(8.0))
                                .bg(gpui::rgba(0xef6a5522))
                                .border_1()
                                .border_color(Theme::error())
                                .font_family(MONO_FONT)
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(Theme::text())
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .id("scene-code")
                            .flex_1()
                            .min_h_0()
                            .overflow_scroll()
                            .py(px(10.0))
                            .pr(px(10.0))
                            .font_family(MONO_FONT)
                            .text_size(px(11.0))
                            .line_height(px(17.0))
                            .children(lines),
                    ),
            )
            .child(
                div().flex_1().min_w_0().h_full().flex().flex_col().child(stage).child(
                    div()
                        .flex_none()
                        .px(px(14.0))
                        .py(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .border_t_1()
                        .border_color(Theme::edge())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .w(px(56.0))
                                        .text_size(px(11.5))
                                        .text_color(Theme::subtext())
                                        .child("Length"),
                                )
                                .children(lengths),
                        )
                        .child(backdrop),
                ),
            )
    }

    fn change_scene(&mut self, id: MediaId, cx: &mut Context<Self>, f: impl FnOnce(&mut Scene)) {
        let Some(Some(Recipe::Scene(mut scene))) = self.doc.project.media(id).map(|m| m.recipe.clone()) else {
            return;
        };
        f(&mut scene);
        self.edit(cx, |p| p.set_recipe(id, Recipe::Scene(scene)));
    }
}
