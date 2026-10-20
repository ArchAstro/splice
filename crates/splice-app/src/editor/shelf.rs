//! The left panel's shelves: media you imported, text styles, and colours.
//! Anything on a shelf drags onto the timeline, or lands at the playhead with
//! a click.

use super::render::{icon, tip};
use super::*;
use gpui::{FontWeight, Rgba};
use splice_core::look::Look;
use splice_core::model::{Align, Title};

/// A ready-made text style: how it reads, and where it sits.
struct Preset {
    name: &'static str,
    title: Title,
    x: f32,
    y: f32,
}

fn presets() -> Vec<Preset> {
    let base = Title::default();
    vec![
        Preset {
            name: "Title",
            title: Title {
                text: "Title".into(),
                size: 0.09,
                ..base.clone()
            },
            x: 0.0,
            y: 0.0,
        },
        Preset {
            name: "Subtitle",
            title: Title {
                text: "Subtitle".into(),
                size: 0.05,
                bold: false,
                ..base.clone()
            },
            x: 0.0,
            y: 0.33,
        },
        Preset {
            name: "Caption",
            title: Title {
                text: "Caption".into(),
                font: "Avenir Next".into(),
                size: 0.045,
                background: Some(0x000000b3),
                shadow: false,
                ..base.clone()
            },
            x: 0.0,
            y: 0.36,
        },
        Preset {
            name: "Lower third",
            title: Title {
                text: "Name Surname\nWhat they do".into(),
                font: "Avenir Next".into(),
                size: 0.045,
                align: Align::Left,
                color: 0xf3eee4ff,
                background: Some(0xef6a55ff),
                shadow: false,
                ..base.clone()
            },
            x: -0.26,
            y: 0.3,
        },
        Preset {
            name: "Serif",
            title: Title {
                text: "Chapter One".into(),
                font: "Iowan Old Style".into(),
                size: 0.08,
                bold: false,
                italic: true,
                color: 0xf3eee4ff,
                ..base.clone()
            },
            x: 0.0,
            y: 0.0,
        },
        Preset {
            name: "Poster",
            title: Title {
                text: "BIG NEWS".into(),
                font: "Futura".into(),
                size: 0.15,
                color: 0xf2c14eff,
                shadow: false,
                ..base.clone()
            },
            x: 0.0,
            y: 0.0,
        },
        Preset {
            name: "Typewriter",
            title: Title {
                text: "Dear diary,".into(),
                font: "American Typewriter".into(),
                size: 0.06,
                bold: false,
                color: 0xf3eee4ff,
                background: Some(0x12151fe6),
                shadow: false,
                ..base.clone()
            },
            x: 0.0,
            y: 0.0,
        },
        Preset {
            name: "Code",
            title: Title {
                text: "cargo run".into(),
                font: "Menlo".into(),
                size: 0.045,
                bold: false,
                color: 0x86cfc0ff,
                ..base
            },
            x: 0.0,
            y: 0.0,
        },
    ]
}

const COLORS: &[(&str, u32)] = &[
    ("Black", 0x000000ff),
    ("White", 0xffffffff),
    ("Ink", 0x12151fff),
    ("Paper", 0xf3eee4ff),
    ("Vermilion", 0xef6a55ff),
    ("Gold", 0xf2c14eff),
    ("Mint", 0x86cfc0ff),
    ("Sky", 0x6c9ef8ff),
    ("Plum", 0xc792eaff),
    ("Tint", 0x00000080),
];

/// What a title or colour is called on the timeline: a colour by its shelf
/// name when it has one ("Ink"), else as the model names it.
pub(crate) fn generated_name(generator: &Generator) -> String {
    match generator {
        Generator::Color { color } => COLORS
            .iter()
            .find(|(_, c)| c == color)
            .map_or_else(|| generator.name(), |(name, _)| name.to_string()),
        _ => generator.name(),
    }
}

/// Light and dark squares, for showing what is see-through.
fn checkerboard() -> impl IntoElement {
    const SQUARE: f32 = 8.0;
    let rows = (0..12).map(|row| {
        div().flex().children((0..24).map(move |col| {
            let light = (row + col) % 2 == 0;
            div().flex_none().size(px(SQUARE)).bg(if light {
                gpui::rgb(0x9aa0ad)
            } else {
                gpui::rgb(0x6b7180)
            })
        }))
    });
    div()
        .absolute()
        .inset_0()
        .rounded(px(6.0))
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(rows)
}

/// How long a new title or colour runs.
const GENERATED_DURATION: Time = 3 * SECOND;

impl Editor {
    pub(super) fn shelf_tabs(&mut self, count: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab = |this: &Self, shelf: Shelf, label: &'static str, cx: &mut Context<Self>| {
            let on = this.shelf == shelf;
            div()
                .id(label)
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .when(on, |d| d.bg(Theme::lane()))
                .hover(|s| s.bg(Theme::lane()))
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if on { Theme::text() } else { Theme::overlay() })
                        .child(label.to_uppercase()),
                )
                .when(shelf == Shelf::Media && count > 0, |d| {
                    d.child(
                        div()
                            .text_size(px(10.0))
                            .text_color(Theme::surface2())
                            .child(count.to_string()),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.shelf = shelf;
                    cx.notify();
                }))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(tab(self, Shelf::Media, "Media", cx))
            .child(tab(self, Shelf::Text, "Text", cx))
            .child(tab(self, Shelf::Color, "Colour", cx))
    }

    pub(super) fn render_shelf(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.shelf {
            Shelf::Media => div().into_any_element(),
            Shelf::Text => {
                let cards: Vec<AnyElement> = presets()
                    .into_iter()
                    .map(|preset| {
                        let t = &preset.title;
                        let mut sample = t.text.lines().next().unwrap_or("").to_string();
                        // As big as the style is, but never wider than the card: letters
                        // run about 0.6 of their size wide (more when bold or capitals),
                        // and a box adds padding.
                        let room = self.card_width - if t.background.is_some() { 44.0 } else { 20.0 };
                        let lower = sample.chars().any(|c| c.is_lowercase());
                        let em = 0.62 + if t.bold { 0.1 } else { 0.0 } + if lower { 0.0 } else { 0.12 };
                        let fits = |s: &str| room / (s.chars().count().max(1) as f32 * em);
                        // Far too long for a narrow card (the estimate runs wide, so
                        // well under the 9 px floor): its first word stands for it.
                        if fits(&sample) < 6.0
                            && let Some((first, _)) = sample.split_once(' ')
                        {
                            sample = first.to_string();
                        }
                        let fits = fits(&sample);
                        let size = (t.size * 2.65 * self.card_width)
                            .min(fits)
                            .clamp(9.0, self.card_width * 0.22);
                        let words = div()
                            .max_w(px(self.card_width - 14.0))
                            .truncate()
                            .font_family(SharedString::from(t.font.clone()))
                            .text_size(px(size))
                            .font_weight(if t.bold { FontWeight::BOLD } else { FontWeight::NORMAL })
                            .when(t.italic, |d| d.italic())
                            .text_color(gpui::rgba(t.color))
                            .when_some(t.background, |d, bg| {
                                d.px(px(6.0)).py(px(2.0)).rounded(px(4.0)).bg(gpui::rgba(bg))
                            })
                            .child(sample);
                        let generator = Generator::Text(t.clone());
                        let look = Look {
                            x: preset.x,
                            y: preset.y,
                            ..Look::default()
                        };
                        let drag = MediaDrag {
                            media: MediaId(0),
                            name: preset.name.into(),
                            duration: GENERATED_DURATION,
                            generator: Some(generator.clone()),
                        };
                        self.shelf_card(
                            preset.name,
                            words.into_any_element(),
                            Theme::crust(),
                            drag,
                            generator,
                            look,
                            cx,
                        )
                    })
                    .collect();
                self.shelf_grid(
                    "Drag a style onto the timeline, or click to add it at the playhead.",
                    cards,
                )
            }
            Shelf::Color => {
                let cards: Vec<AnyElement> = COLORS
                    .iter()
                    .map(|&(name, color)| {
                        let generator = Generator::Color { color };
                        let drag = MediaDrag {
                            media: MediaId(0),
                            name: name.into(),
                            duration: GENERATED_DURATION,
                            generator: Some(generator.clone()),
                        };
                        // See-through colours sit on a checkerboard, so they read as see-through.
                        let see_through = color & 0xff < 0xff;
                        let fill = div()
                            .absolute()
                            .inset_0()
                            .when(see_through, |d| d.child(checkerboard()))
                            .child(div().absolute().inset_0().rounded(px(6.0)).bg(gpui::rgba(color)));
                        self.shelf_card(
                            name,
                            fill.into_any_element(),
                            Theme::crust(),
                            drag,
                            generator,
                            Look::default(),
                            cx,
                        )
                    })
                    .collect();
                self.shelf_grid("Backgrounds and title cards. Tint darkens what is under it.", cards)
            }
        }
    }

    fn shelf_grid(&self, hint: &'static str, cards: Vec<AnyElement>) -> AnyElement {
        div()
            .id("shelf")
            .flex_1()
            .overflow_y_scroll()
            .px(px(render::SHELF_PAD))
            .pb_3()
            .child(
                div()
                    .pb(px(10.0))
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(Theme::overlay())
                    .child(hint),
            )
            .child(div().flex().flex_wrap().gap(px(render::CARD_GAP)).children(cards))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn shelf_card(
        &self,
        name: &'static str,
        face: AnyElement,
        ground: Rgba,
        drag: MediaDrag,
        generator: Generator,
        look: Look,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group: SharedString = format!("shelf-{name}").into();
        let card = self.card_width;
        div()
            .id(SharedString::from(format!("card-{name}")))
            .group(group.clone())
            .w(px(card))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .cursor_grab()
            .child(
                div()
                    .w(px(card))
                    .h(px(card * render::CARD_ASPECT))
                    .rounded(px(7.0))
                    .overflow_hidden()
                    .relative()
                    .bg(ground)
                    .border_1()
                    .border_color(Theme::edge())
                    .group_hover(group.clone(), |s| s.border_color(Theme::surface2()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(6.0))
                    .child(face),
            )
            .child(
                div()
                    .w(px(card))
                    .px(px(1.0))
                    .text_size(px(10.5))
                    .text_color(Theme::muted())
                    .truncate()
                    .child(name),
            )
            .tooltip(tip("Click to add at the playhead", None))
            .on_drag(drag, |drag: &MediaDrag, _, _, cx| {
                cx.new(|_| DragChip(drag.name.clone()))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.add_generated(generator.clone(), look.clone(), window, cx);
            }))
            .into_any_element()
    }

    /// The layer just above everything under the playhead for `duration`.
    fn layer_above(&self, start: Time, duration: Time) -> usize {
        self.doc
            .project
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.clips.iter().any(|c| c.start < start + duration && c.end() > start))
            .map(|(i, _)| i + 1)
            .max()
            .unwrap_or(0)
    }

    /// Puts a title or colour at the playhead, over what is there.
    pub(super) fn add_generated(
        &mut self,
        generator: Generator,
        look: Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = self.player.position;
        let is_text = matches!(generator, Generator::Text(_));
        // Colours go under what is there when the playhead is on a gap.
        let track = self.layer_above(at, GENERATED_DURATION);
        let id = self.edit(cx, |p| {
            let id = p.insert_generated(generator, track, at, GENERATED_DURATION);
            p.set_look(id, |l| *l = look);
            id
        });
        self.selection = vec![id];
        if is_text {
            self.begin_title_edit(id, window, cx);
        }
        cx.notify();
    }

    pub(super) fn add_text(&mut self, _: &AddText, window: &mut Window, cx: &mut Context<Self>) {
        let preset = presets().into_iter().next().expect("a title preset");
        self.add_generated(Generator::Text(preset.title), Look::default(), window, cx);
    }

    pub(super) fn shelf_header_action(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.shelf == Shelf::Media).then(|| {
            render::icon_button("import", "plus", true)
                .tooltip(tip("Import media", Some("⌘I")))
                .on_click(cx.listener(|this, _, window, cx| this.import(&Import, window, cx)))
                .into_any_element()
        })
    }

    /// Tiles after the media that make new media: a score, a scene.
    pub(super) fn make_cards(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let card = self.card_width;
        let tile = |id: &'static str, glyph: &'static str, label: &'static str, hint: &'static str| {
            div()
                .id(id)
                .group(id)
                .w(px(card))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .cursor_pointer()
                .tooltip(tip(hint, None))
                .child(
                    div()
                        .w(px(card))
                        .h(px(card * render::CARD_ASPECT))
                        .rounded(px(7.0))
                        .border_1()
                        .border_dashed()
                        .border_color(Theme::surface1())
                        .group_hover(id, |s| s.border_color(Theme::accent()).bg(Theme::lane()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph, Theme::subtext()).size(px(18.0))),
                )
                .child(
                    div()
                        .w(px(card))
                        .px(px(1.0))
                        .text_size(px(10.5))
                        .text_color(Theme::muted())
                        .truncate()
                        .child(label),
                )
        };
        vec![
            tile(
                "make-score",
                "score",
                "New score",
                "Music to compose, or ask the assistant to",
            )
            .on_click(cx.listener(|this, _, window, cx| this.new_score(window, cx)))
            .into_any_element(),
            tile(
                "make-scene",
                "scene",
                "New scene",
                "An animation or diagram: shapes, lines and words that move",
            )
            .on_click(cx.listener(|this, _, window, cx| this.new_scene(window, cx)))
            .into_any_element(),
        ]
    }

    /// A starter score at the playhead, open in its editor.
    pub(super) fn new_score(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.player.position;
        let track = self.doc.project.tracks.len();
        let (id, _) = self.add_score(None, Some((at, track)), cx);
        self.open_workbench(id, window, cx);
        self.say("A starter score: edit the notes, or Ask the assistant to compose (⇧⌘A)");
    }

    /// A starter scene over what is at the playhead, open in its editor.
    pub(super) fn new_scene(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.player.position;
        let track = self.layer_above(at, 5 * SECOND);
        let n = self
            .doc
            .project
            .media
            .iter()
            .filter(|m| matches!(m.recipe, Some(splice_core::model::Recipe::Scene(_))))
            .count();
        let name = if n == 0 {
            "Scene".to_string()
        } else {
            format!("Scene {}", n + 1)
        };
        let (id, _) = self.add_scene(name, None, 5 * SECOND, true, Some((at, track)), cx);
        self.open_workbench(id, window, cx);
    }
}
