//! The inspector: everything about the selected clip. Text and its style for
//! titles; position, size and opacity; animation in and out; green screen;
//! a mask; and volume. Sliders and chips edit live, one undo step each.

use super::render::{caps, icon, tip};
use super::stage::Grab;
use super::*;
use gpui::{Hsla, Stateful};
use splice_core::look::{
    Backdrop, Drift, Ease, KEY_NEAR, Key, Mask, MotionKind, Preset, Prop, Redact, RedactStyle, Shape,
};
use splice_core::model::{Align, Clip, Source, Title};
use splice_core::transition::{Kind as TransitionKind, Transition};

pub const INSPECTOR_WIDTH: f32 = 292.0;

/// Letterbox shapes offered: name, and width over height (None: no bars).
pub const LETTERBOXES: &[(&str, Option<f32>)] = &[
    ("None", None),
    ("2.39 Cinema", Some(2.39)),
    ("2:1", Some(2.0)),
    ("1.85", Some(1.85)),
    ("4:3", Some(4.0 / 3.0)),
];

/// A number the inspector edits with a slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    X,
    Y,
    Scale,
    Opacity,
    Volume,
    TextSize,
    KeySimilarity,
    KeyBlend,
    MaskX,
    MaskY,
    MaskWidth,
    MaskHeight,
    MaskFeather,
    CropLeft,
    CropRight,
    CropTop,
    CropBottom,
    EnterDuration,
    ExitDuration,
    Exposure,
    Contrast,
    Saturation,
    Warmth,
    Tint,
    Speed,
    TransitionLength,
    Blur,
    Vignette,
    Grain,
    Shake,
    Glitch,
    BackdropBlur,
    BackdropDim,
    RedactX,
    RedactY,
    RedactWidth,
    RedactHeight,
    TextOver,
}

impl Field {
    /// The keyframed property it shows, if it is one.
    pub fn prop(self) -> Option<Prop> {
        match self {
            Field::X => Some(Prop::X),
            Field::Y => Some(Prop::Y),
            Field::Scale => Some(Prop::Scale),
            Field::Opacity => Some(Prop::Opacity),
            _ => None,
        }
    }

    fn range(self) -> (f32, f32) {
        match self {
            Field::X | Field::Y => (-0.75, 0.75),
            Field::Scale => (0.05, 3.0),
            Field::Opacity => (0.0, 1.0),
            Field::Volume => (0.0, 2.0),
            Field::TextSize => (0.02, 0.3),
            Field::KeySimilarity => (0.01, 0.6),
            Field::KeyBlend => (0.0, 0.5),
            Field::MaskX | Field::MaskY => (0.0, 1.0),
            Field::MaskWidth | Field::MaskHeight => (0.05, 1.5),
            Field::MaskFeather => (0.001, 1.0),
            Field::CropLeft | Field::CropRight | Field::CropTop | Field::CropBottom => (0.0, 0.95),
            Field::EnterDuration | Field::ExitDuration => (0.1, 3.0),
            Field::Exposure | Field::Contrast | Field::Saturation | Field::Warmth | Field::Tint => (-1.0, 1.0),
            Field::Speed => (0.1, 4.0),
            Field::TransitionLength => (0.1, 3.0),
            Field::Blur
            | Field::Vignette
            | Field::Grain
            | Field::Shake
            | Field::Glitch
            | Field::BackdropBlur
            | Field::BackdropDim
            | Field::RedactX
            | Field::RedactY => (0.0, 1.0),
            Field::RedactWidth | Field::RedactHeight => (0.02, 1.0),
            Field::TextOver => (0.1, 5.0),
        }
    }

    /// What a double-click puts back.
    fn default(self) -> f32 {
        match self {
            Field::X | Field::Y => 0.0,
            Field::Scale | Field::Opacity | Field::Volume => 1.0,
            Field::TextSize => Title::default().size,
            Field::KeySimilarity => Key::default().similarity,
            Field::KeyBlend => Key::default().blend,
            Field::MaskX | Field::MaskY => 0.5,
            Field::MaskWidth | Field::MaskHeight => 0.8,
            Field::MaskFeather => Mask::default().feather,
            Field::CropLeft | Field::CropRight | Field::CropTop | Field::CropBottom => 0.0,
            Field::EnterDuration | Field::ExitDuration => 0.5,
            Field::Exposure | Field::Contrast | Field::Saturation | Field::Warmth | Field::Tint => 0.0,
            Field::Speed => 1.0,
            Field::TransitionLength => secs(Transition::DEFAULT_LENGTH) as f32,
            Field::Blur | Field::Vignette | Field::Grain | Field::Shake | Field::Glitch => 0.0,
            Field::BackdropBlur => Backdrop::default().blur,
            Field::BackdropDim => Backdrop::default().dim,
            Field::RedactX => Redact::default().x,
            Field::RedactY => Redact::default().y,
            Field::RedactWidth => Redact::default().width,
            Field::RedactHeight => Redact::default().height,
            Field::TextOver => 1.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Field::X => "Across",
            Field::Y => "Down",
            Field::Scale => "Size",
            Field::Opacity => "Opacity",
            Field::Volume => "Volume",
            Field::TextSize => "Text size",
            Field::KeySimilarity => "Strength",
            Field::KeyBlend => "Softness",
            Field::MaskX => "Across",
            Field::MaskY => "Down",
            Field::MaskWidth => "Width",
            Field::MaskHeight => "Height",
            Field::MaskFeather => "Feather",
            Field::CropLeft => "Left",
            Field::CropRight => "Right",
            Field::CropTop => "Top",
            Field::CropBottom => "Bottom",
            Field::EnterDuration | Field::ExitDuration | Field::TransitionLength => "Length",
            Field::Exposure => "Exposure",
            Field::Contrast => "Contrast",
            Field::Saturation => "Saturation",
            Field::Warmth => "Warmth",
            Field::Tint => "Tint",
            Field::Speed => "Speed",
            Field::Blur | Field::BackdropBlur => "Blur",
            Field::Vignette => "Vignette",
            Field::Grain => "Grain",
            Field::Shake => "Shake",
            Field::Glitch => "Glitch",
            Field::BackdropDim => "Darken",
            Field::RedactX => "Across",
            Field::RedactY => "Down",
            Field::RedactWidth => "Width",
            Field::RedactHeight => "Height",
            Field::TextOver => "Over",
        }
    }

    /// The value as shown; `canvas_height` turns text size into pixels.
    fn format(self, v: f32, canvas_height: u32) -> String {
        match self {
            Field::Scale => format!("{:.0}%", v * 100.0),
            Field::Volume => {
                if v <= 0.0 {
                    "off".into()
                } else {
                    format!("{:+.1} dB", 20.0 * v.log10())
                }
            }
            Field::TextSize => format!("{:.0} px", v * canvas_height as f32),
            Field::EnterDuration | Field::ExitDuration | Field::TransitionLength | Field::TextOver => {
                format!("{v:.1}s")
            }
            Field::Exposure => format!("{v:+.1}"),
            Field::Contrast | Field::Saturation | Field::Warmth | Field::Tint => format!("{:+.0}", v * 100.0),
            Field::Speed => format!("{}×", crate::timeline::speed_label(v as f64)),
            Field::X | Field::Y => format!("{:+.0}%", v * 100.0),
            // Mask places count from the middle of the picture, like X and Y.
            Field::MaskX | Field::MaskY | Field::RedactX | Field::RedactY => format!("{:+.0}%", (v - 0.5) * 100.0),
            _ => format!("{:.0}%", v * 100.0),
        }
    }

    /// Its value for `clip`, `at` (clip time) for what keyframes animate.
    fn get(self, clip: &Clip, at: Time) -> f32 {
        let look = &clip.look;
        let key = look.key.unwrap_or_default();
        let mask = look.mask.unwrap_or_default();
        let title = match &clip.generator {
            Some(Generator::Text(t)) => t.size,
            _ => Title::default().size,
        };
        if let Some(prop) = self.prop() {
            return look.value(prop, at);
        }
        match self {
            Field::X | Field::Y | Field::Scale | Field::Opacity => unreachable!("keyed above"),
            Field::Volume => clip.volume,
            Field::TextSize => title,
            Field::KeySimilarity => key.similarity,
            Field::KeyBlend => key.blend,
            Field::MaskX => mask.x,
            Field::MaskY => mask.y,
            Field::MaskWidth => mask.width,
            Field::MaskHeight => mask.height,
            Field::MaskFeather => mask.feather,
            Field::CropLeft => look.crop.map_or(0.0, |c| c.left),
            Field::CropRight => look.crop.map_or(0.0, |c| c.right),
            Field::CropTop => look.crop.map_or(0.0, |c| c.top),
            Field::CropBottom => look.crop.map_or(0.0, |c| c.bottom),
            Field::EnterDuration => secs(look.enter.duration) as f32,
            Field::ExitDuration => secs(look.exit.duration) as f32,
            Field::Exposure => look.grade.exposure,
            Field::Contrast => look.grade.contrast,
            Field::Saturation => look.grade.saturation,
            Field::Warmth => look.grade.temperature,
            Field::Tint => look.grade.tint,
            Field::Speed => clip.speed as f32,
            Field::TransitionLength => clip.transition.map_or(0.6, |t| secs(t.duration) as f32),
            Field::Blur => look.fx.blur,
            Field::Vignette => look.fx.vignette,
            Field::Grain => look.fx.grain,
            Field::Shake => look.fx.shake,
            Field::Glitch => look.fx.glitch,
            Field::BackdropBlur => look.backdrop.unwrap_or_default().blur,
            Field::BackdropDim => look.backdrop.unwrap_or_default().dim,
            Field::RedactX => look.redact.unwrap_or_default().x,
            Field::RedactY => look.redact.unwrap_or_default().y,
            Field::RedactWidth => look.redact.unwrap_or_default().width,
            Field::RedactHeight => look.redact.unwrap_or_default().height,
            Field::TextOver => match &clip.generator {
                Some(Generator::Text(t)) => t.animate.map_or(1.0, |m| secs(m.over) as f32),
                _ => 1.0,
            },
        }
    }

    /// Sets it on `clip`; what keyframes animate gets a keyframe `at` (clip
    /// time). Speed and transition lengths go through the project instead.
    fn set(self, clip: &mut Clip, v: f32, at: Time) {
        let look = &mut clip.look;
        if let Some(prop) = self.prop() {
            look.set_value(prop, at, v);
            return;
        }
        match self {
            Field::X | Field::Y | Field::Scale | Field::Opacity => {}
            Field::Speed | Field::TransitionLength => {}
            Field::TextOver => {
                if let Some(Generator::Text(t)) = &mut clip.generator
                    && let Some(m) = &mut t.animate
                {
                    m.over = splice_core::time::from_secs(v as f64);
                }
            }
            Field::Blur => look.fx.blur = v,
            Field::Vignette => look.fx.vignette = v,
            Field::Grain => look.fx.grain = v,
            Field::Shake => look.fx.shake = v,
            Field::Glitch => look.fx.glitch = v,
            Field::BackdropBlur | Field::BackdropDim => {
                if let Some(b) = &mut look.backdrop {
                    if self == Field::BackdropBlur {
                        b.blur = v;
                    } else {
                        b.dim = v;
                    }
                }
            }
            Field::RedactX | Field::RedactY | Field::RedactWidth | Field::RedactHeight => {
                if let Some(r) = &mut look.redact {
                    match self {
                        Field::RedactX => r.x = v,
                        Field::RedactY => r.y = v,
                        Field::RedactWidth => r.width = v,
                        _ => r.height = v,
                    }
                }
            }
            Field::Exposure => look.grade.exposure = v,
            Field::Contrast => look.grade.contrast = v,
            Field::Saturation => look.grade.saturation = v,
            Field::Warmth => look.grade.temperature = v,
            Field::Tint => look.grade.tint = v,
            Field::Volume => clip.volume = v,
            Field::TextSize => {
                if let Some(Generator::Text(t)) = &mut clip.generator {
                    t.size = v;
                }
            }
            Field::KeySimilarity => {
                if let Some(k) = &mut look.key {
                    k.similarity = v;
                }
            }
            Field::KeyBlend => {
                if let Some(k) = &mut look.key {
                    k.blend = v;
                }
            }
            Field::MaskX | Field::MaskY | Field::MaskWidth | Field::MaskHeight | Field::MaskFeather => {
                if let Some(m) = &mut look.mask {
                    match self {
                        Field::MaskX => m.x = v,
                        Field::MaskY => m.y = v,
                        Field::MaskWidth => m.width = v,
                        Field::MaskHeight => m.height = v,
                        _ => m.feather = v,
                    }
                }
            }
            Field::CropLeft | Field::CropRight | Field::CropTop | Field::CropBottom => {
                let mut c = look.crop.unwrap_or_default();
                match self {
                    Field::CropLeft => c.left = v,
                    Field::CropRight => c.right = v,
                    Field::CropTop => c.top = v,
                    _ => c.bottom = v,
                }
                let c = c.clamped();
                look.crop = (!c.is_none()).then_some(c);
            }
            Field::EnterDuration => look.enter.duration = splice_core::time::from_secs(v as f64),
            Field::ExitDuration => look.exit.duration = splice_core::time::from_secs(v as f64),
        }
    }
}

/// Words being typed in the inspector, and the project before the typing began.
pub struct TitleEdit {
    pub clip: ClipId,
    /// Which words of the clip.
    pub what: Typing,
    pub before: Project,
    /// Byte offset of the caret.
    pub cursor: usize,
    /// Everything is selected: the next keystroke replaces it.
    pub all: bool,
    /// Words typed that are not yet anywhere in the project (a hex colour).
    pub scratch: String,
}

/// What is being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typing {
    /// A title's words.
    Title,
    /// The words one use of a symbol puts in its placeholder.
    Fill(String),
    /// The name of the placeholder a clip is, in a symbol.
    Label,
    /// A colour for one use's placeholder, typed as hex.
    Hex(String),
}

/// Swatches offered for text, boxes and colours: 0xRRGGBBAA.
pub const SWATCHES: &[u32] = &[
    0xffffffff, 0xf3eee4ff, 0x12151fff, 0x000000ff, 0xef6a55ff, 0xf2c14eff, 0x86cfc0ff, 0x6c9ef8ff, 0xc792eaff,
];
const BOXES: &[u32] = &[0x000000b3, 0x12151fe6, 0xffffffe6, 0xef6a55ff, 0xf2c14eff];

/// A length in plain words: `4s`, `2.5s`, `1:05`.
fn seconds(t: Time) -> String {
    let s = splice_core::time::secs(t);
    if s >= 60.0 {
        format!("{}:{:02}", (s / 60.0) as u64, (s % 60.0).round() as u64)
    } else if (s - s.round()).abs() < 0.05 {
        format!("{}s", s.round())
    } else {
        format!("{s:.1}s")
    }
}

fn hsla(color: u32) -> Hsla {
    gpui::rgba(color).into()
}

impl Editor {
    pub(super) fn inspected(&self) -> Option<ClipId> {
        self.selection
            .first()
            .copied()
            .filter(|id| self.doc.project.slot(*id).is_some())
    }

    /// Applies a change to one clip as its own undo step.
    fn change(&mut self, id: ClipId, cx: &mut Context<Self>, f: impl FnOnce(&mut Clip)) {
        self.edit(cx, |p| {
            if let Some(clip) = p.clip_mut(id) {
                f(clip);
            }
        });
    }

    fn change_title(&mut self, id: ClipId, cx: &mut Context<Self>, f: impl FnOnce(&mut Title)) {
        self.change(id, cx, |clip| {
            if let Some(Generator::Text(title)) = &mut clip.generator {
                f(title);
            }
        });
    }

    // Sliders.

    fn slider(&self, id: ClipId, clip: &Clip, field: Field, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (lo, hi) = self.field_range(field, clip);
        let value = match field {
            // As it plays: trims since can have shortened it.
            Field::TransitionLength => self
                .doc
                .project
                .cut_into(id)
                .map_or_else(|| field.get(clip, 0), |c| secs(c.duration()) as f32),
            _ => field.get(clip, self.clip_time(clip)),
        };
        let frac = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
        let bounds = self.slider_bounds.clone();
        let held = matches!(&self.grab, Some(Grab::Slider { field: f, .. }) if *f == field);
        let canvas_height = self.doc.project.settings.height;
        let group: SharedString = format!("slider-{field:?}").into();
        div()
            .h(px(26.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(64.0))
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(Theme::subtext())
                    .child(field.label()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("track-{field:?}")))
                    .group(group.clone())
                    .flex_1()
                    .h(px(20.0))
                    .relative()
                    .cursor_pointer()
                    .child(
                        canvas(
                            move |b, _, _| {
                                bounds.borrow_mut().insert(field, b);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(9.0))
                            .h(px(2.0))
                            .rounded_full()
                            .bg(Theme::surface1()),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(9.0))
                            .h(px(2.0))
                            .w(relative(frac))
                            .rounded_full()
                            .bg(Theme::subtext()),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(4.0))
                            .left(relative(frac))
                            .ml(px(-6.0))
                            .size(px(12.0))
                            .rounded_full()
                            .bg(Theme::text())
                            .border_2()
                            .border_color(if held { Theme::accent() } else { Theme::panel() })
                            .shadow(render::shadow(0.45, 4.0, 1.0))
                            .group_hover(group, |s| s.border_color(Theme::accent())),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count >= 2 {
                                let before = this.doc.project.clone();
                                this.apply_field(id, field, field.default());
                                this.doc.commit(before);
                                this.after_edit();
                                cx.notify();
                                return;
                            }
                            this.grab = Some(Grab::Slider {
                                field,
                                before: this.doc.project.clone(),
                            });
                            this.slide(field, event.position, cx);
                        }),
                    ),
            )
            .child(
                div()
                    .w(px(48.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .font_family(MONO_FONT)
                    .text_size(px(10.5))
                    .text_color(Theme::muted())
                    .child(field.format(value, canvas_height)),
            )
    }

    /// Sets a slider's value from the pointer.
    pub(super) fn slide(&mut self, field: Field, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(bounds) = self.slider_bounds.borrow().get(&field).copied() else {
            return;
        };
        let Some(id) = self.inspected() else { return };
        let Some(clip) = self.doc.project.clip(id) else { return };
        let (lo, hi) = self.field_range(field, clip);
        let frac = (f32::from(at.x - bounds.left()) / f32::from(bounds.size.width).max(1.0)).clamp(0.0, 1.0);
        let mut v = lo + frac * (hi - lo);
        // Rest at the natural value when near it.
        if (v - field.default()).abs() < (hi - lo) * 0.015 {
            v = field.default();
        }
        self.apply_field(id, field, v);
        cx.notify();
    }

    /// A slider's range for this clip: a transition is no longer than its clips allow.
    fn field_range(&self, field: Field, clip: &Clip) -> (f32, f32) {
        let (lo, hi) = field.range();
        match field {
            Field::TransitionLength => {
                let most = secs(self.doc.project.longest_transition(clip.id)) as f32;
                (lo, hi.min(most).max(lo + 0.05))
            }
            _ => (lo, hi),
        }
    }

    /// Sets a slider's value on clip `id`, not yet an undo step.
    pub(super) fn apply_field(&mut self, id: ClipId, field: Field, v: f32) {
        match field {
            Field::Speed => self.doc.project.set_speed(&[id], v as f64),
            Field::TransitionLength => {
                if let Some(t) = self.doc.project.clip(id).and_then(|c| c.transition) {
                    let length = splice_core::time::from_secs(v as f64);
                    self.doc
                        .project
                        .set_transition(&[id], Some(Transition { duration: length, ..t }));
                }
            }
            _ => {
                let Some(at) = self.doc.project.clip(id).map(|c| self.clip_time(c)) else {
                    return;
                };
                if let Some(clip) = self.doc.project.clip_mut(id) {
                    field.set(clip, v, at);
                }
            }
        }
    }

    // Small controls.

    pub(super) fn chip(
        &self,
        id: impl Into<SharedString>,
        label: &str,
        on: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Editor, &mut Context<Editor>) + 'static,
    ) -> Stateful<gpui::Div> {
        div()
            .id(gpui::ElementId::Name(id.into()))
            .h(px(24.0))
            .px(px(9.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.5))
            .cursor_pointer()
            .border_1()
            .map(|d| {
                if on {
                    d.bg(Theme::surface1())
                        .border_color(Theme::surface2())
                        .text_color(Theme::text())
                } else {
                    d.border_color(Theme::edge())
                        .text_color(Theme::subtext())
                        .hover(|s| s.bg(Theme::lane()).text_color(Theme::text()))
                }
            })
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
    }

    pub(super) fn swatch(
        &self,
        id: impl Into<SharedString>,
        color: Option<u32>,
        on: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Editor, &mut Context<Editor>) + 'static,
    ) -> Stateful<gpui::Div> {
        div()
            .id(gpui::ElementId::Name(id.into()))
            .size(px(24.0))
            .rounded_full()
            .p(px(2.0))
            .border_1()
            .border_color(if on { Theme::accent() } else { gpui::rgba(0x00000000) })
            .cursor_pointer()
            .child(
                div()
                    .size_full()
                    .rounded_full()
                    .border_1()
                    .border_color(gpui::rgba(0xffffff2e))
                    .map(|d| match color {
                        Some(c) => d.bg(hsla(c)),
                        // "None": a crossed-out circle.
                        None => d
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().w(px(14.0)).h(px(1.5)).bg(Theme::muted()).rounded_full()),
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
    }

    pub(super) fn toggle(
        &self,
        id: &'static str,
        label: &str,
        on: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Editor, &mut Context<Editor>) + 'static,
    ) -> Stateful<gpui::Div> {
        div()
            .id(id)
            .h(px(26.0))
            .flex()
            .items_center()
            .justify_between()
            .cursor_pointer()
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(Theme::subtext())
                    .child(label.to_string()),
            )
            .child(
                div()
                    .w(px(28.0))
                    .h(px(16.0))
                    .rounded_full()
                    .p(px(2.0))
                    .flex()
                    .when(on, |d| d.justify_end())
                    .bg(if on { Theme::accent() } else { Theme::surface1() })
                    .child(div().size(px(12.0)).rounded_full().bg(Theme::text())),
            )
            .on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
    }

    // The panel.

    pub(super) fn render_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let body: AnyElement = match self.inspected().and_then(|id| self.doc.project.clip(id).cloned()) {
            Some(clip) => self.inspect_clip(clip, window, cx).into_any_element(),
            None => self.inspect_canvas(cx).into_any_element(),
        };
        render::panel()
            .id("inspector")
            .w(px(INSPECTOR_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .child(body)
    }

    fn header(&self, glyph: &str, title: String, detail: String) -> impl IntoElement + use<> {
        div()
            .h(px(52.0))
            .flex_none()
            .px(px(16.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .border_b_1()
            .border_color(Theme::edge())
            .child(
                div()
                    .size(px(28.0))
                    .rounded(px(7.0))
                    .bg(Theme::lane())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(glyph, Theme::subtext()).size(px(14.0))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(Theme::text())
                            .truncate()
                            .child(title),
                    )
                    .child(div().text_size(px(10.5)).text_color(Theme::overlay()).child(detail)),
            )
    }

    pub(super) fn section(&self, title: &str) -> gpui::Div {
        div()
            .px(px(16.0))
            .pt(px(14.0))
            .pb(px(12.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .border_b_1()
            .border_color(Theme::edge())
            .child(div().pb(px(2.0)).child(caps(title)))
    }

    /// A section that folds away while unused: open when in use, or when
    /// opened by hand. Closed, its header says what it is set to.
    fn fold(
        &self,
        id: ClipId,
        title: &'static str,
        in_use: bool,
        summary: String,
        cx: &mut Context<Self>,
    ) -> (gpui::Div, bool) {
        let open = self.folds.get(&(id, title)).copied().unwrap_or(in_use);
        let group: SharedString = format!("fold-{title}").into();
        let header = div()
            .id(SharedString::from(format!("fold-{title}")))
            .group(group.clone())
            .h(px(16.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .cursor_pointer()
            .child(caps(title).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(6.0))
                    .when(!open, |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(if in_use { Theme::subtext() } else { Theme::overlay() })
                                .child(summary),
                        )
                    })
                    .child(
                        icon(if open { "chevron" } else { "chevron-right" }, Theme::overlay())
                            .size(px(12.0))
                            .group_hover(group, |s| s.text_color(Theme::text())),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.folds.insert((id, title), !open);
                cx.notify();
            }));
        let section = div()
            .px(px(16.0))
            .pt(px(14.0))
            .pb(px(if open { 12.0 } else { 14.0 }))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .border_b_1()
            .border_color(Theme::edge())
            .child(div().when(open, |d| d.pb(px(2.0))).child(header));
        (section, open)
    }

    fn inspect_canvas(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let s = self.doc.project.settings;
        let chips: Vec<AnyElement> = ASPECTS
            .iter()
            .map(|(name, w, h)| {
                let on = (s.width, s.height) == (*w, *h);
                let (w, h) = (*w, *h);
                self.chip(format!("aspect-{name}"), name, on, cx, move |this, cx| {
                    this.edit(cx, |p| {
                        p.settings.width = w;
                        p.settings.height = h;
                    });
                })
                .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .child(self.header(
                "film",
                "Canvas".into(),
                format!("{}×{} · {} fps", s.width, s.height, s.fps),
            ))
            .child(
                self.section("Shape")
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(chips)),
            )
            .child(self.letterbox_section(cx))
            .when_some(self.hold_section(cx), |d, hold| d.child(hold))
            .child(
                div()
                    .px(px(16.0))
                    .py(px(18.0))
                    .text_size(px(11.5))
                    .line_height(px(17.0))
                    .text_color(Theme::overlay())
                    .child("Select a clip to place it, animate it, key out a green screen or mask it."),
            )
    }

    /// Bars that frame the movie to another shape.
    fn letterbox_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let current = self.doc.project.letterbox;
        let chips: Vec<AnyElement> = LETTERBOXES
            .iter()
            .map(|(label, aspect)| {
                let aspect = *aspect;
                let on = match (current, aspect) {
                    (None, None) => true,
                    (Some(a), Some(b)) => (a - b).abs() < 0.005,
                    _ => false,
                };
                self.chip(format!("letterbox-{label}"), label, on, cx, move |this, cx| {
                    this.edit(cx, |p| p.letterbox = aspect);
                })
                .into_any_element()
            })
            .collect();
        self.section("Letterbox")
            .child(div().flex().flex_wrap().gap(px(6.0)).children(chips))
            .child(
                div()
                    .text_size(px(10.5))
                    .line_height(px(15.0))
                    .text_color(Theme::overlay())
                    .child("Black bars over the whole movie, framing it to a wider shape."),
            )
    }

    fn inspect_clip(&mut self, clip: Clip, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let id = clip.id;
        let project = &self.doc.project;
        let (glyph, kind, visual, audio, media_clip) = match project.source(&clip) {
            Some(Source::Media(m)) if matches!(m.recipe, Some(splice_core::model::Recipe::Symbol(_))) => {
                ("symbol", "Symbol", true, m.has_audio, false)
            }
            Some(Source::Media(m)) if m.recipe.is_some() => match m.kind {
                MediaKind::Audio => ("score", "Score", false, true, false),
                _ => ("scene", "Scene", true, false, false),
            },
            Some(Source::Media(m)) => match m.kind {
                MediaKind::Video => ("film", "Video", true, m.has_audio, true),
                MediaKind::Image => ("image", "Picture", true, false, true),
                MediaKind::Audio => ("music", "Sound", false, true, true),
            },
            Some(Source::Generated(Generator::Text(_))) => ("text", "Text", true, false, false),
            Some(Source::Generated(Generator::Color { .. })) => ("swatch", "Colour", true, false, false),
            None => ("film", "Missing", false, false, false),
        };
        let name = match &clip.generator {
            Some(generator) => shelf::generated_name(generator),
            None => project.clip_name(&clip),
        };
        let detail = format!(
            "{kind} · {} long · starts at {}",
            seconds(clip.duration),
            seconds(clip.start)
        );

        // What its media lets it do: change colour (any picture of media),
        // change speed (video and sound, not a symbol, which stretches instead).
        let (graded, timed) = match project.source(&clip) {
            Some(Source::Media(m)) => (
                m.is_visual(),
                m.is_timed() && !matches!(m.recipe, Some(splice_core::model::Recipe::Symbol(_))),
            ),
            _ => (false, false),
        };
        let mut sections: Vec<AnyElement> = Vec::new();
        let symbol = project
            .media(clip.media)
            .filter(|_| clip.generator.is_none())
            .and_then(|m| match &m.recipe {
                Some(splice_core::model::Recipe::Symbol(s)) => Some((m.id, s.clone())),
                _ => None,
            });
        let made = project
            .media(clip.media)
            .filter(|_| clip.generator.is_none() && symbol.is_none())
            .and_then(|m| m.recipe.as_ref().map(|r| (m.id, r.kind())));
        if let Some((media, symbol)) = &symbol {
            sections.extend(self.symbol_sections(id, *media, symbol, window, cx));
        }
        if self.editing_a_symbol() {
            sections.push(self.placeholder_section(id, &clip, window, cx));
        }
        if let Some((media, kind)) = made {
            let (glyph, label, hint) = if kind == "score" {
                (
                    "score",
                    "Edit the score",
                    "Notes, instruments and tempo, or ask the assistant to compose.",
                )
            } else {
                (
                    "scene",
                    "Edit the scene",
                    "Its layers and motion beside the picture, or ask the assistant.",
                )
            };
            sections.push(
                self.section(if kind == "score" { "Score" } else { "Scene" })
                    .child(
                        div()
                            .id("open-workbench")
                            .h(px(32.0))
                            .rounded(px(8.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(7.0))
                            .cursor_pointer()
                            .bg(Theme::text())
                            .hover(|s| s.bg(Theme::subtext()))
                            .child(icon(glyph, Theme::ground()).size(px(13.0)))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(Theme::ground())
                                    .child(label),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| this.open_workbench(media, window, cx))),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(Theme::overlay())
                            .child(hint),
                    )
                    .into_any_element(),
            );
        }
        match &clip.generator {
            Some(Generator::Text(title)) => sections.push(
                self.text_section(id, &clip, title.clone(), window, cx)
                    .into_any_element(),
            ),
            Some(Generator::Color { color }) => {
                let current = *color;
                let swatches: Vec<AnyElement> = SWATCHES
                    .iter()
                    .map(|&c| {
                        self.swatch(format!("fill-{c:08x}"), Some(c), c == current, cx, move |this, cx| {
                            this.change(id, cx, |clip| clip.generator = Some(Generator::Color { color: c }));
                        })
                        .into_any_element()
                    })
                    .collect();
                sections.push(
                    self.section("Colour")
                        .child(div().flex().flex_wrap().gap(px(4.0)).children(swatches))
                        .into_any_element(),
                );
            }
            None => {}
        }
        if visual {
            sections.push(self.place_section(id, &clip, cx).into_any_element());
            sections.push(self.transition_section(id, &clip, cx).into_any_element());
            sections.push(self.motion_section(id, &clip, cx).into_any_element());
            if graded {
                sections.push(self.colour_section(id, &clip, cx).into_any_element());
                sections.push(self.effects_section(id, &clip, cx).into_any_element());
                sections.push(self.backdrop_section(id, &clip, cx).into_any_element());
                sections.push(self.redact_section(id, &clip, cx).into_any_element());
            } else {
                sections.push(self.effects_section(id, &clip, cx).into_any_element());
            }
        }
        if timed {
            sections.push(self.speed_section(id, &clip, cx).into_any_element());
        }
        // A screen recording with its clicks noted.
        let clicks = self
            .doc
            .project
            .media(clip.media)
            .filter(|_| clip.generator.is_none())
            .map_or(0, |m| m.clicks.len());
        if clicks > 0 {
            sections.push(self.zoom_section(id, clicks, cx).into_any_element());
        }
        if visual {
            if media_clip {
                sections.push(self.crop_section(id, &clip, cx).into_any_element());
                sections.push(self.key_section(id, &clip, cx).into_any_element());
            }
            sections.push(self.mask_section(id, &clip, cx).into_any_element());
        }
        if audio {
            let speech = clip.generator.is_none() && !clip.reverse && !clip.frozen;
            sections.push(
                self.section("Sound")
                    .child(self.slider(id, &clip, Field::Volume, cx))
                    .when(speech, |d| {
                        d.child(
                            div().flex().child(
                                self.chip("captions", "Add captions from speech", false, cx, move |this, cx| {
                                    if let Err(e) = this.caption_clip(id, Default::default(), None, cx) {
                                        this.say_error(e);
                                    }
                                })
                                .tooltip(tip("Hears what is said, on this Mac, and lays captions over it", None)),
                            ),
                        )
                    })
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.header(glyph, name, detail))
            .child(
                div()
                    .id("inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(sections),
            )
    }

    fn text_section(
        &mut self,
        id: ClipId,
        clip: &Clip,
        title: Title,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let editing = self
            .title_edit
            .as_ref()
            .filter(|e| e.clip == id && e.what == Typing::Title && self.text_focus.is_focused(window));
        let (cursor, all) = editing.map_or((title.text.len(), false), |e| (e.cursor.min(title.text.len()), e.all));
        let focused = editing.is_some();

        // The words, with a caret where typing goes.
        let mut lines: Vec<AnyElement> = Vec::new();
        let mut offset = 0;
        for line in title.text.split('\n') {
            let end = offset + line.len();
            let row = div().min_h(px(18.0)).flex().items_center().flex_wrap();
            let row = if focused && !all && cursor >= offset && cursor <= end {
                let (a, b) = line.split_at(cursor - offset);
                row.child(a.to_string())
                    .child(div().w(px(1.5)).h(px(16.0)).bg(Theme::accent()))
                    .child(b.to_string())
            } else {
                row.child(line.to_string())
            };
            lines.push(row.into_any_element());
            offset = end + 1;
        }

        let text_box = div()
            .id("title-text")
            .key_context("TextField")
            .track_focus(&self.text_focus)
            .on_key_down(cx.listener(Self::title_key))
            .on_action(cx.listener(|this, _: &TextSelectAll, _, cx| {
                if let Some(edit) = &mut this.title_edit {
                    edit.all = true;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TextPaste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    this.type_text(&text, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TextDone, window, cx| {
                this.end_title_edit(cx);
                window.focus(&this.focus);
            }))
            .min_h(px(64.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(Theme::crust())
            .border_1()
            .border_color(if focused { Theme::surface2() } else { Theme::edge() })
            .text_size(px(13.0))
            .line_height(px(18.0))
            .text_color(Theme::text())
            .cursor_text()
            .when(all && focused, |d| d.bg(Theme::accent_soft()))
            .children(lines)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.begin_title_edit(id, window, cx);
                    cx.notify();
                }),
            );

        let font = title.font.clone();
        let aligns: Vec<AnyElement> = [
            (Align::Left, "align-left", "Align left"),
            (Align::Center, "align-center", "Centre"),
            (Align::Right, "align-right", "Align right"),
        ]
        .into_iter()
        .map(|(a, glyph, name)| {
            let on = title.align == a;
            self.chip(format!("align-{glyph}"), "", on, cx, move |this, cx| {
                this.change_title(id, cx, |t| t.align = a);
            })
            .w(px(30.0))
            .px_0()
            .tooltip(tip(name, None))
            .child(icon(glyph, if on { Theme::text() } else { Theme::subtext() }).size(px(14.0)))
            .into_any_element()
        })
        .collect();
        let bold = self
            .chip("bold", "", title.bold, cx, move |this, cx| {
                this.change_title(id, cx, |t| t.bold = !t.bold);
            })
            .w(px(30.0))
            .px_0()
            .tooltip(tip("Bold", None))
            .child(div().font_weight(gpui::FontWeight::BOLD).text_size(px(13.0)).child("B"));
        let italic = self
            .chip("italic", "", title.italic, cx, move |this, cx| {
                this.change_title(id, cx, |t| t.italic = !t.italic);
            })
            .w(px(30.0))
            .px_0()
            .tooltip(tip("Italic", None))
            .child(
                div()
                    .italic()
                    .font_family("Iowan Old Style")
                    .text_size(px(14.0))
                    .child("I"),
            );
        let colors: Vec<AnyElement> = SWATCHES
            .iter()
            .map(|&c| {
                self.swatch(
                    format!("text-{c:08x}"),
                    Some(c),
                    title.color == c,
                    cx,
                    move |this, cx| {
                        this.change_title(id, cx, |t| t.color = c);
                    },
                )
                .into_any_element()
            })
            .collect();
        let mut boxes: Vec<AnyElement> = vec![
            self.swatch("box-none", None, title.background.is_none(), cx, move |this, cx| {
                this.change_title(id, cx, |t| t.background = None);
            })
            .into_any_element(),
        ];
        boxes.extend(BOXES.iter().map(|&c| {
            self.swatch(
                format!("box-{c:08x}"),
                Some(c),
                title.background == Some(c),
                cx,
                move |this, cx| {
                    this.change_title(id, cx, |t| t.background = Some(c));
                },
            )
            .into_any_element()
        }));

        self.section("Text")
            .child(text_box)
            .child(
                div()
                    .id("font")
                    .mt(px(4.0))
                    .h(px(32.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(Theme::edge())
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::lane()))
                    .child(
                        div()
                            .font_family(SharedString::from(font.clone()))
                            .text_size(px(14.0))
                            .child(font),
                    )
                    .child(icon("chevron", Theme::overlay()).size(px(12.0)))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_fonts(id, window, cx))),
            )
            .child(self.slider(id, clip, Field::TextSize, cx))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().flex().gap(px(4.0)).child(bold).child(italic))
                    .child(div().flex().gap(px(4.0)).children(aligns)),
            )
            .child(
                div()
                    .pt(px(4.0))
                    .text_size(px(11.0))
                    .text_color(Theme::overlay())
                    .child("Colour"),
            )
            .child(div().flex().flex_wrap().gap(px(2.0)).children(colors))
            .child(
                div()
                    .pt(px(4.0))
                    .text_size(px(11.0))
                    .text_color(Theme::overlay())
                    .child("Box"),
            )
            .child(div().flex().flex_wrap().gap(px(2.0)).children(boxes))
            .child(self.toggle("shadow", "Shadow", title.shadow, cx, move |this, cx| {
                this.change_title(id, cx, |t| t.shadow = !t.shadow);
            }))
            .child(self.words_in(id, clip, &title, cx))
    }

    /// How a title's words come in: one at a time, and how.
    fn words_in(&mut self, id: ClipId, clip: &Clip, title: &Title, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use splice_core::textmotion::{By, Style, TextMotion};
        let motion = title.animate;
        let timed = !title.timing.is_empty();
        let mut styles: Vec<AnyElement> = vec![
            self.chip("words-none", "All at once", motion.is_none(), cx, move |this, cx| {
                this.change_title(id, cx, |t| t.animate = None);
            })
            .into_any_element(),
        ];
        styles.extend(Style::ALL.into_iter().map(|style| {
            let on = motion.is_some_and(|m| m.style == style);
            self.chip(
                format!("words-{}", style.name()),
                style.label(),
                on,
                cx,
                move |this, cx| {
                    this.change_title(id, cx, |t| {
                        t.animate = Some(TextMotion {
                            style,
                            ..t.animate.unwrap_or_default()
                        });
                    });
                },
            )
            .into_any_element()
        }));
        let units: Vec<AnyElement> = By::ALL
            .into_iter()
            .map(|by| {
                let on = motion.is_some_and(|m| m.by == by);
                let label = match by {
                    By::Word => "Words",
                    By::Letter => "Letters",
                    By::Line => "Lines",
                };
                self.chip(format!("words-by-{}", by.name()), label, on, cx, move |this, cx| {
                    this.change_title(id, cx, |t| {
                        if let Some(m) = &mut t.animate {
                            m.by = by;
                        }
                    });
                })
                .flex_1()
                .into_any_element()
            })
            .collect();
        div()
            .pt(px(6.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(Theme::overlay())
                    .child("Words come in"),
            )
            .child(div().flex().flex_wrap().gap(px(5.0)).children(styles))
            .when(motion.is_some(), |d| {
                d.child(div().flex().gap(px(5.0)).children(units))
                    .when(!timed, |d| d.child(self.slider(id, clip, Field::TextOver, cx)))
                    .when(timed, |d| {
                        d.child(
                            div()
                                .text_size(px(10.5))
                                .line_height(px(15.0))
                                .text_color(Theme::overlay())
                                .child("Timed to the words as they are said."),
                        )
                    })
            })
    }

    fn place_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // Nine places to put a picture, like a phone's camera grid.
        let spot = |x: f32, y: f32, cx: &mut Context<Self>| -> AnyElement {
            {
                let on = (clip.look.x - x).abs() < 0.01 && (clip.look.y - y).abs() < 0.01;
                div()
                    .id(SharedString::from(format!("spot-{x}-{y}")))
                    .size(px(18.0))
                    .rounded(px(4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::surface0()))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(if on { Theme::accent() } else { Theme::surface2() }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change(id, cx, |c| {
                            c.look.x = x;
                            c.look.y = y;
                        });
                    }))
                    .into_any_element()
            }
        };
        let rows: Vec<AnyElement> = [-0.32f32, 0.0, 0.32]
            .into_iter()
            .map(|y| {
                let cells: Vec<AnyElement> = [-0.32f32, 0.0, 0.32].into_iter().map(|x| spot(x, y, cx)).collect();
                div().flex().gap(px(2.0)).children(cells).into_any_element()
            })
            .collect();
        self.section("Place")
            .child(
                div()
                    .flex()
                    .gap(px(14.0))
                    .items_center()
                    .child(
                        div()
                            .p(px(4.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(Theme::edge())
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(rows),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(Theme::overlay())
                            .child("Drag it on the stage, or its corners to resize. ◆ keys a value at the playhead."),
                    ),
            )
            .child(self.keyed_slider(id, clip, Field::Scale, cx))
            .child(self.keyed_slider(id, clip, Field::Opacity, cx))
            .child(self.keyed_slider(id, clip, Field::X, cx))
            .child(self.keyed_slider(id, clip, Field::Y, cx))
            .when(!clip.look.keys.is_empty(), |d| d.child(self.keyframe_bar(id, clip, cx)))
            .when(
                !clip.look.keys.is_empty() && (self.player.position < clip.start || self.player.position > clip.end()),
                |d| {
                    d.child(
                        div()
                            .text_size(px(10.5))
                            .line_height(px(15.0))
                            .text_color(Theme::overlay())
                            .child(
                                "The playhead is outside this clip: changes to animated values key its nearest end.",
                            ),
                    )
                },
            )
    }

    // Keyframes.

    /// A slider with a keyframe button: filled when this value has a
    /// keyframe at the playhead, hollow and bright when it is animated
    /// elsewhere, faint when it is not animated.
    fn keyed_slider(
        &mut self,
        id: ClipId,
        clip: &Clip,
        field: Field,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let prop = field.prop().expect("a keyed field");
        let at = self.clip_time(clip);
        let keys = clip.look.keys.of(prop);
        let here = keys.iter().any(|k| (k.at - at).abs() <= KEY_NEAR);
        let animated = !keys.is_empty();
        // Keyframes go where the playhead is, so it has to be in the clip.
        let inside = self.player.position >= clip.start && self.player.position <= clip.end();
        let color = if !inside {
            Theme::surface2()
        } else if here || animated {
            Theme::accent()
        } else {
            Theme::overlay()
        };
        let button = div()
            .id(SharedString::from(format!("key-{}", prop.name())))
            .size(px(18.0))
            .flex_none()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(Theme::lane()))
            .tooltip(tip(
                if !inside {
                    "Move the playhead into this clip to keyframe it"
                } else if here {
                    "Take away this keyframe"
                } else if animated {
                    "Add a keyframe here"
                } else {
                    "Animate: keyframe this value here"
                },
                None,
            ))
            .child(icon(if here { "key-on" } else { "key" }, color).size(px(11.0)))
            .when(inside, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| this.toggle_key(id, prop, cx)))
            });
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(button)
            .child(div().flex_1().min_w_0().child(self.slider(id, clip, field, cx)))
    }

    /// Keyframes the value of `prop` at the playhead, or takes away the one there.
    pub(super) fn toggle_key(&mut self, id: ClipId, prop: Prop, cx: &mut Context<Self>) {
        let Some(clip) = self.doc.project.clip(id) else { return };
        let at = self.clip_time(clip);
        self.change(id, cx, |c| {
            let value = c.look.value(prop, at);
            if !c.look.keys.remove(prop, at) {
                c.look.keys.set(prop, at, value, None);
            } else if c.look.keys.of(prop).is_empty() {
                // The last one gone, the value stays where it was.
                c.look.set_value(prop, at, value);
            }
        });
    }

    /// Under the keyed sliders: step between keyframes, ease the one at the
    /// playhead, or clear them all.
    fn keyframe_bar(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let at = self.clip_time(clip);
        let moments = clip.look.keys.moments();
        let start = clip.start;
        let previous = moments.iter().rev().find(|m| **m < at - KEY_NEAR).copied();
        let next = moments.iter().find(|m| **m > at + KEY_NEAR).copied();
        let here = clip.look.keys.has_at(at);
        let step = |name: &'static str, glyph: &str, to: Option<Time>, cx: &mut Context<Self>| {
            let on = to.is_some();
            div()
                .id(name)
                .size(px(22.0))
                .rounded(px(5.0))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |d| d.cursor_pointer().hover(|s| s.bg(Theme::lane())))
                .child(icon(glyph, if on { Theme::text() } else { Theme::surface2() }).size(px(13.0)))
                .when_some(to, |d, to| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.seek(start + to, cx)))
                })
                .tooltip(tip(
                    if name == "key-prev" {
                        "Previous keyframe"
                    } else {
                        "Next keyframe"
                    },
                    None,
                ))
                .into_any_element()
        };
        let ease_now = Prop::ALL
            .iter()
            .flat_map(|p| clip.look.keys.of(*p).iter())
            .find(|k| (k.at - at).abs() <= KEY_NEAR)
            .map(|k| k.ease);
        let eases: Vec<AnyElement> = [
            (Ease::Linear, "Linear"),
            (Ease::InOut, "Smooth"),
            (Ease::In, "In"),
            (Ease::Out, "Out"),
            (Ease::Hold, "Hold"),
        ]
        .into_iter()
        .map(|(ease, label)| {
            self.chip(
                format!("ease-{}", ease.name()),
                label,
                ease_now == Some(ease),
                cx,
                move |this, cx| {
                    let Some(clip) = this.doc.project.clip(id) else { return };
                    let at = this.clip_time(clip);
                    this.change(id, cx, |c| {
                        for p in Prop::ALL {
                            for k in c.look.keys.of_mut(p) {
                                if (k.at - at).abs() <= KEY_NEAR {
                                    k.ease = ease;
                                }
                            }
                        }
                    });
                },
            )
            .flex_1()
            .px(px(4.0))
            .into_any_element()
        })
        .collect();
        let count = moments.len();
        div()
            .pt(px(4.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(step("key-prev", "chevron-left", previous, cx))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .text_color(Theme::subtext())
                            .child(if here {
                                format!("Keyframe at {}", seconds(at))
                            } else {
                                format!("{count} keyframe{}", if count == 1 { "" } else { "s" })
                            }),
                    )
                    .child(step("key-next", "chevron-right", next, cx))
                    .child(self.chip("keys-clear", "Clear", false, cx, move |this, cx| {
                        let Some(clip) = this.doc.project.clip(id) else { return };
                        let at = this.clip_time(clip);
                        this.change(id, cx, |c| {
                            // Each value stays as it is at the playhead.
                            for p in Prop::ALL {
                                let v = c.look.value(p, at);
                                c.look.keys.of_mut(p).clear();
                                c.look.set_value(p, at, v);
                            }
                        });
                    })),
            )
            .when(here, |d| {
                d.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(Theme::overlay())
                        .child("From this keyframe to the next:"),
                )
                .child(div().flex().gap(px(4.0)).children(eases))
            })
    }

    // Transitions.

    fn transition_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let current = clip.transition;
        let from_before = self.doc.project.clip_before(id).is_some();
        let summary = match current {
            Some(t) => format!("{} · {}", t.kind.label(), seconds(t.duration)),
            None => "None".into(),
        };
        let (section, open) = self.fold(id, "Transition", current.is_some(), summary, cx);
        let mut chips: Vec<AnyElement> = vec![
            self.chip("transition-none", "None", current.is_none(), cx, move |this, cx| {
                this.edit(cx, |p| p.set_transition(&[id], None));
            })
            .into_any_element(),
        ];
        chips.extend(TransitionKind::ALL.into_iter().map(|kind| {
            let on = current.is_some_and(|t| t.kind == kind);
            self.chip(
                format!("transition-{}", kind.name()),
                kind.label(),
                on,
                cx,
                move |this, cx| {
                    let length = this
                        .doc
                        .project
                        .clip(id)
                        .and_then(|c| c.transition)
                        .map_or(Transition::DEFAULT_LENGTH, |t| t.duration);
                    this.edit(cx, |p| {
                        p.set_transition(&[id], Some(Transition { kind, duration: length }));
                    });
                },
            )
            .into_any_element()
        }));
        section.when(open, |d| {
            d.child(div().flex().flex_wrap().gap(px(5.0)).children(chips))
                .when(current.is_some(), |d| {
                    d.child(self.slider(id, clip, Field::TransitionLength, cx))
                })
                .child(
                    div()
                        .text_size(px(10.5))
                        .line_height(px(15.0))
                        .text_color(Theme::overlay())
                        .child(if from_before {
                            "From the clip before, centred on the cut. Frames past either clip's ends hold still."
                        } else {
                            "No clip ends right here, so it comes in over what is beneath."
                        }),
                )
        })
    }

    // Colour.

    fn colour_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let grade = clip.look.grade;
        let (section, open) = self.fold(id, "Colour", !grade.is_none(), grade.summary(), cx);
        let looks: Vec<AnyElement> = Preset::ALL
            .into_iter()
            .map(|preset| {
                self.chip(
                    format!("look-{}", preset.name()),
                    preset.label(),
                    grade.preset == preset,
                    cx,
                    move |this, cx| this.change(id, cx, |c| c.look.grade.preset = preset),
                )
                .into_any_element()
            })
            .collect();
        section.when(open, |d| {
            d.child(div().flex().flex_wrap().gap(px(5.0)).children(looks))
                .child(self.slider(id, clip, Field::Exposure, cx))
                .child(self.slider(id, clip, Field::Contrast, cx))
                .child(self.slider(id, clip, Field::Saturation, cx))
                .child(self.slider(id, clip, Field::Warmth, cx))
                .child(self.slider(id, clip, Field::Tint, cx))
                .child(
                    div()
                        .flex()
                        .gap(px(5.0))
                        .when(!grade.is_none(), |d| {
                            d.child(self.chip("grade-reset", "Reset", false, cx, move |this, cx| {
                                this.change(id, cx, |c| c.look.grade = Default::default());
                            }))
                        })
                        .child(
                            self.chip("grade-all", "Use on every clip", false, cx, move |this, cx| {
                                this.grade_everything_like(id, cx);
                            })
                            .tooltip(tip("Give every video and picture this colour", None)),
                        ),
                )
        })
    }

    /// Gives every video and picture clip the colour of clip `id`.
    fn grade_everything_like(&mut self, id: ClipId, cx: &mut Context<Self>) {
        let Some(grade) = self.doc.project.clip(id).map(|c| c.look.grade) else {
            return;
        };
        let count = self.edit(cx, |p| {
            let ids: Vec<ClipId> = p
                .clips()
                .filter(|(_, c)| c.generator.is_none())
                .filter(|(_, c)| p.media(c.media).is_some_and(|m| m.is_visual()))
                .map(|(_, c)| c.id)
                .collect();
            for clip in &ids {
                p.set_look(*clip, |l| l.grade = grade);
            }
            ids.len()
        });
        self.say(format!("Every picture has this colour now ({count} clips)"));
    }

    // Effects.

    fn effects_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let fx = clip.look.fx;
        // Titles and colours shake and glitch; what blurs and grains is media.
        let media = clip.generator.is_none();
        let (section, open) = self.fold(id, "Effects", !fx.is_none(), fx.summary(), cx);
        section.when(open, |d| {
            d.when(media, |d| {
                d.child(self.slider(id, clip, Field::Blur, cx))
                    .child(self.slider(id, clip, Field::Vignette, cx))
                    .child(self.slider(id, clip, Field::Grain, cx))
            })
            .child(self.slider(id, clip, Field::Shake, cx))
            .child(self.slider(id, clip, Field::Glitch, cx))
            .when(!fx.is_none(), |d| {
                d.child(
                    div()
                        .flex()
                        .child(self.chip("fx-reset", "Reset effects", false, cx, move |this, cx| {
                            this.change(id, cx, |c| c.look.fx = Default::default());
                        })),
                )
            })
        })
    }

    /// A blurred copy of the picture filling the canvas behind it.
    fn backdrop_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let on = clip.look.backdrop.is_some();
        let (section, open) = self.fold(id, "Background", on, if on { "Blurred fill" } else { "Off" }.into(), cx);
        section.when(open, |d| {
            d.child(self.toggle(
                "backdrop-on",
                "Fill the canvas behind it, blurred",
                on,
                cx,
                move |this, cx| {
                    this.change(id, cx, |c| {
                        c.look.backdrop = if c.look.backdrop.is_some() {
                            None
                        } else {
                            Some(Backdrop::default())
                        };
                    });
                },
            ))
            .when(on, |d| {
                d.child(self.slider(id, clip, Field::BackdropBlur, cx))
                    .child(self.slider(id, clip, Field::BackdropDim, cx))
            })
            .child(
                div()
                    .text_size(px(10.5))
                    .line_height(px(15.0))
                    .text_color(Theme::overlay())
                    .child("For a picture that doesn't fill the frame: a phone video on a wide canvas."),
            )
        })
    }

    /// Hiding part of the picture: a face, a screen, a number plate.
    fn redact_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let redact = clip.look.redact;
        let styles: Vec<AnyElement> = std::iter::once(None)
            .chain(RedactStyle::ALL.into_iter().map(Some))
            .map(|style| {
                let label = match style {
                    None => "Off",
                    Some(RedactStyle::Blur) => "Blur",
                    Some(RedactStyle::Pixelate) => "Pixelate",
                    Some(RedactStyle::Black) => "Black box",
                };
                let on = redact.map(|r| r.style) == style;
                self.chip(format!("redact-{label}"), label, on, cx, move |this, cx| {
                    this.change(id, cx, |c| {
                        c.look.redact = style.map(|style| Redact {
                            style,
                            ..c.look.redact.unwrap_or_default()
                        });
                    });
                })
                .flex_1()
                .px(px(4.0))
                .into_any_element()
            })
            .collect();
        let summary = match redact.map(|r| r.style) {
            None => "Off",
            Some(RedactStyle::Blur) => "Blurred box",
            Some(RedactStyle::Pixelate) => "Pixelated box",
            Some(RedactStyle::Black) => "Black box",
        };
        let (section, open) = self.fold(id, "Hide part", redact.is_some(), summary.into(), cx);
        section.when(open, |d| {
            d.child(div().flex().gap(px(4.0)).children(styles))
                .when(redact.is_some(), |d| {
                    d.child(self.slider(id, clip, Field::RedactWidth, cx))
                        .child(self.slider(id, clip, Field::RedactHeight, cx))
                        .child(self.slider(id, clip, Field::RedactX, cx))
                        .child(self.slider(id, clip, Field::RedactY, cx))
                })
        })
    }

    /// Zooming in where a screen recording was clicked.
    fn zoom_section(&mut self, id: ClipId, clicks: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let zooms: Vec<AnyElement> = [1.5f32, 2.0, 2.5]
            .into_iter()
            .map(|zoom| {
                self.chip(
                    format!("zoom-{zoom}"),
                    &format!("{zoom}×"),
                    false,
                    cx,
                    move |this, cx| {
                        let made = this.edit(cx, |p| p.zoom_to_clicks(id, zoom));
                        if made == 0 {
                            this.say_error("No clicks in the part of the recording this clip plays");
                        } else {
                            this.say(format!(
                                "Zooms in {made} time{}; they are keyframes, so drag or ease them as you like",
                                if made == 1 { "" } else { "s" }
                            ));
                        }
                    },
                )
                .flex_1()
                .into_any_element()
            })
            .collect();
        self.section("Zoom to clicks")
            .child(
                div()
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(Theme::overlay())
                    .child(format!(
                        "{clicks} click{} noted while recording. Zoom in towards each burst of them:",
                        if clicks == 1 { "" } else { "s" }
                    )),
            )
            .child(div().flex().gap(px(5.0)).children(zooms))
            .child(
                div()
                    .flex()
                    .child(self.chip("zoom-off", "Remove zoom", false, cx, move |this, cx| {
                        this.edit(cx, |p| p.clear_zoom(id));
                    })),
            )
    }

    // Speed.

    fn speed_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if clip.frozen {
            let (section, open) = self.fold(id, "Speed", true, "Held frame".into(), cx);
            return section
                .when(open, |d| {
                    d.child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(Theme::overlay())
                            .child("One frame, held. Drag its end on the timeline to hold it longer."),
                    )
                })
                .into_any_element();
        }
        let video = self
            .doc
            .project
            .media(clip.media)
            .is_some_and(|m| m.kind == MediaKind::Video);
        let speed = clip.speed;
        let mut said = Vec::new();
        if speed != 1.0 {
            said.push(format!("{}×", crate::timeline::speed_label(speed)));
        }
        if clip.reverse {
            said.push("Backwards".to_string());
        }
        let summary = if said.is_empty() {
            "Normal".to_string()
        } else {
            said.join(" · ")
        };
        let (section, open) = self.fold(id, "Speed", !said.is_empty(), summary, cx);
        let presets: Vec<AnyElement> = [0.25, 0.5, 1.0, 1.5, 2.0, 4.0]
            .into_iter()
            .map(|v: f64| {
                self.chip(
                    format!("speed-{v}"),
                    &format!("{}×", crate::timeline::speed_label(v)),
                    (speed - v).abs() < 1e-6,
                    cx,
                    move |this, cx| this.edit(cx, |p| p.set_speed(&[id], v)),
                )
                .flex_1()
                .px(px(2.0))
                .into_any_element()
            })
            .collect();
        let inside = clip.contains(self.player.position);
        section
            .when(open, |d| {
                d.child(div().flex().gap(px(4.0)).children(presets))
                    .child(self.slider(id, clip, Field::Speed, cx))
                    .child(
                        self.toggle("reverse", "Play backwards", clip.reverse, cx, move |this, cx| {
                            let back = this.doc.project.clip(id).is_some_and(|c| !c.reverse);
                            this.edit(cx, |p| p.set_reverse(&[id], back));
                        }),
                    )
                    .when(video, |d| {
                        d.child(
                            div().flex().child(
                                self.chip("freeze", "Freeze frame at the playhead", false, cx, move |this, cx| {
                                    this.freeze_frame_at_playhead(cx);
                                })
                                .when(!inside, |c| c.opacity(0.5))
                                .tooltip(tip(
                                    "Holds this frame for two seconds; what follows moves along",
                                    Some("⌥F"),
                                )),
                            ),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(10.5))
                            .line_height(px(15.0))
                            .text_color(Theme::overlay())
                            .child("Faster clips get shorter and what follows moves up; sound keeps its pitch."),
                    )
            })
            .into_any_element()
    }

    fn motion_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let look = clip.look.clone();
        let names = |k: MotionKind| match k {
            MotionKind::None => "None",
            MotionKind::Fade => "Fade",
            MotionKind::SlideUp => "Up",
            MotionKind::SlideDown => "Down",
            MotionKind::SlideLeft => "Left",
            MotionKind::SlideRight => "Right",
            MotionKind::Zoom => "Zoom",
            MotionKind::Pop => "Pop",
        };
        let row = |this: &mut Self, enter: bool, cx: &mut Context<Self>| -> Vec<AnyElement> {
            MotionKind::ALL
                .into_iter()
                .map(|k| {
                    let current = if enter { look.enter.kind } else { look.exit.kind };
                    let side = if enter { "in" } else { "out" };
                    this.chip(
                        format!("motion-{side}-{}", k.name()),
                        names(k),
                        current == k,
                        cx,
                        move |this, cx| {
                            this.change(id, cx, |c| {
                                if enter {
                                    c.look.enter.kind = k;
                                } else {
                                    c.look.exit.kind = k;
                                }
                            });
                        },
                    )
                    .into_any_element()
                })
                .collect()
        };
        // Rows of four, the same width, so the choices line up.
        let grid = |chips: Vec<AnyElement>| {
            let mut chips = chips.into_iter();
            let mut rows = Vec::new();
            loop {
                let row: Vec<AnyElement> = chips
                    .by_ref()
                    .take(4)
                    .map(|c| div().flex_1().flex().flex_col().child(c).into_any_element())
                    .collect();
                if row.is_empty() {
                    break;
                }
                rows.push(div().flex().gap(px(5.0)).children(row).into_any_element());
            }
            div().flex().flex_col().gap(px(5.0)).children(rows)
        };
        let enter = grid(row(self, true, cx));
        let exit = grid(row(self, false, cx));
        let drifts: Vec<AnyElement> = [
            (Drift::None, "None"),
            (Drift::ZoomIn, "Zoom in"),
            (Drift::ZoomOut, "Zoom out"),
        ]
        .into_iter()
        .map(|(d, label)| {
            self.chip(format!("drift-{label}"), label, look.drift == d, cx, move |this, cx| {
                this.change(id, cx, |c| c.look.drift = d);
            })
            .into_any_element()
        })
        .collect();
        let label = |text: &str| {
            div()
                .pt(px(2.0))
                .text_size(px(11.0))
                .text_color(Theme::overlay())
                .child(text.to_string())
        };
        // Closed, the header says what moves: "Fade in · Slide down out".
        let said = |k: MotionKind| match k {
            MotionKind::SlideUp => "Slide up",
            MotionKind::SlideDown => "Slide down",
            MotionKind::SlideLeft => "Slide left",
            MotionKind::SlideRight => "Slide right",
            k => names(k),
        };
        let mut used = Vec::new();
        if look.enter.kind != MotionKind::None {
            used.push(format!("{} in", said(look.enter.kind)));
        }
        if look.exit.kind != MotionKind::None {
            used.push(format!("{} out", said(look.exit.kind)));
        }
        match look.drift {
            Drift::ZoomIn => used.push("Slow zoom".into()),
            Drift::ZoomOut => used.push("Slow zoom out".into()),
            Drift::None => {}
        }
        let summary = if used.is_empty() {
            "None".into()
        } else {
            used.join(" · ")
        };
        let (section, open) = self.fold(id, "Animate", !used.is_empty(), summary, cx);
        section.when(open, |d| {
            d.child(label("In"))
                .child(enter)
                .when(look.enter.kind != MotionKind::None, |d| {
                    d.child(self.slider(id, clip, Field::EnterDuration, cx))
                })
                .child(label("Out"))
                .child(exit)
                .when(look.exit.kind != MotionKind::None, |d| {
                    d.child(self.slider(id, clip, Field::ExitDuration, cx))
                })
                .child(label("Across the clip"))
                .child(grid(drifts))
        })
    }

    fn key_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let key = clip.look.key;
        let on = key.is_some();
        let colors: Vec<AnyElement> = [(0x00ff00u32, "green"), (0x0047bbu32, "blue")]
            .into_iter()
            .map(|(c, name)| {
                let current = key.map(|k| k.color) == Some(c);
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(self.swatch(
                        format!("key-{name}"),
                        Some(c << 8 | 0xff),
                        current,
                        cx,
                        move |this, cx| {
                            this.change(id, cx, |clip| {
                                let mut k = clip.look.key.unwrap_or_default();
                                k.color = c;
                                clip.look.key = Some(k);
                            });
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(Theme::subtext())
                            .child(match name {
                                "green" => "Green screen",
                                _ => "Blue screen",
                            }),
                    )
                    .into_any_element()
            })
            .collect();
        let summary = match key.map(|k| k.color) {
            None => "Off",
            Some(0x00ff00) => "Green",
            Some(0x0047bb) => "Blue",
            Some(_) => "On",
        };
        let (section, open) = self.fold(id, "Green screen", on, summary.into(), cx);
        section
            .when(open, |d| {
                d.child(
                    self.toggle("key-on", "Remove the background colour", on, cx, move |this, cx| {
                        this.change(id, cx, |c| {
                            c.look.key = if c.look.key.is_some() {
                                None
                            } else {
                                Some(Key::default())
                            };
                        });
                    }),
                )
            })
            .when(open && on, |d| {
                d.child(div().flex().gap(px(14.0)).children(colors))
                    .child(self.slider(id, clip, Field::KeySimilarity, cx))
                    .child(self.slider(id, clip, Field::KeyBlend, cx))
                    .child(self.toggle(
                        "despill",
                        "Clean up colour fringes",
                        key.is_some_and(|k| k.despill),
                        cx,
                        move |this, cx| {
                            this.change(id, cx, |c| {
                                if let Some(k) = &mut c.look.key {
                                    k.despill = !k.despill;
                                }
                            });
                        },
                    ))
            })
    }

    /// Cutting edges off a picture: four sliders, and a way back.
    fn crop_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let crop = clip.look.crop.filter(|c| !c.is_none());
        let summary = match crop {
            None => "Off".to_string(),
            Some(c) => {
                let (w, h) = (1.0 - c.left - c.right, 1.0 - c.top - c.bottom);
                format!("{:.0}% × {:.0}% left", w * 100.0, h * 100.0)
            }
        };
        let (section, open) = self.fold(id, "Crop", crop.is_some(), summary, cx);
        section.when(open, |d| {
            d.child(self.slider(id, clip, Field::CropLeft, cx))
                .child(self.slider(id, clip, Field::CropRight, cx))
                .child(self.slider(id, clip, Field::CropTop, cx))
                .child(self.slider(id, clip, Field::CropBottom, cx))
                .child(
                    div()
                        .text_size(px(10.5))
                        .line_height(px(15.0))
                        .text_color(Theme::overlay())
                        .child("The picture keeps its size; what remains stays where it was."),
                )
                .when(crop.is_some(), |d| {
                    d.child(
                        div()
                            .flex()
                            .child(self.chip("crop-reset", "Reset crop", false, cx, move |this, cx| {
                                this.change(id, cx, |c| c.look.crop = None);
                            })),
                    )
                })
        })
    }

    fn mask_section(&mut self, id: ClipId, clip: &Clip, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mask = clip.look.mask;
        let shapes: Vec<AnyElement> = [
            (None, "None"),
            (Some(Shape::Ellipse), "Circle"),
            (Some(Shape::Rect), "Box"),
        ]
        .into_iter()
        .map(|(shape, label)| {
            let on = mask.map(|m| m.shape) == shape;
            self.chip(format!("mask-{label}"), label, on, cx, move |this, cx| {
                this.change(id, cx, |c| {
                    c.look.mask = shape.map(|shape| Mask {
                        shape,
                        ..c.look.mask.unwrap_or_default()
                    });
                });
            })
            .into_any_element()
        })
        .collect();
        let summary = match mask.map(|m| m.shape) {
            None => "Off",
            Some(Shape::Ellipse) => "Circle",
            Some(Shape::Rect) => "Box",
        };
        let (section, open) = self.fold(id, "Mask", mask.is_some(), summary.into(), cx);
        section
            .when(open, |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(5.0))
                        .children(shapes.into_iter().map(|c| div().flex_1().flex().flex_col().child(c))),
                )
            })
            .when(open && mask.is_some(), |d| {
                d.child(self.slider(id, clip, Field::MaskWidth, cx))
                    .child(self.slider(id, clip, Field::MaskHeight, cx))
                    .child(self.slider(id, clip, Field::MaskX, cx))
                    .child(self.slider(id, clip, Field::MaskY, cx))
                    .child(self.slider(id, clip, Field::MaskFeather, cx))
                    .child(self.toggle(
                        "mask-invert",
                        "Show the outside instead",
                        mask.is_some_and(|m| m.invert),
                        cx,
                        move |this, cx| {
                            this.change(id, cx, |c| {
                                if let Some(m) = &mut c.look.mask {
                                    m.invert = !m.invert;
                                }
                            });
                        },
                    ))
            })
    }

    // Typing a title.

    pub(super) fn begin_title_edit(&mut self, id: ClipId, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_inspector_typing(id, Typing::Title, window, cx);
    }

    /// Starts typing `what` of clip `id`, everything selected.
    pub(super) fn begin_inspector_typing(
        &mut self,
        id: ClipId,
        what: Typing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.title_edit.as_ref().is_some_and(|e| e.clip == id && e.what == what) {
            window.focus(&self.text_focus);
            return;
        }
        self.end_title_edit(cx);
        if self.doc.project.clip(id).is_none() || (what == Typing::Title && self.typed_text(id, &what).is_none()) {
            return;
        }
        // A colour typed as hex starts from the one shown.
        let scratch = match &what {
            Typing::Hex(key) => self
                .doc
                .project
                .clip(id)
                .and_then(|c| self.doc.project.media(c.media))
                .and_then(|m| match &m.recipe {
                    Some(splice_core::model::Recipe::Symbol(s)) => s.placeholder(key).map(|p| s.shown(&p)),
                    _ => None,
                })
                .map(|fill| match fill {
                    splice_core::symbol::Fill::Color { color } => format!("#{:06x}", color >> 8),
                    _ => String::new(),
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        let len = match &what {
            Typing::Hex(_) => scratch.len(),
            _ => self.typed_text(id, &what).map_or(0, |t| t.len()),
        };
        self.selection = vec![id];
        self.layout.inspector_open = true;
        self.title_edit = Some(TitleEdit {
            clip: id,
            what,
            before: self.doc.project.clone(),
            cursor: len,
            all: true,
            scratch,
        });
        window.focus(&self.text_focus);
    }

    /// The words `what` of clip `id` show now.
    pub(super) fn typed_text(&self, id: ClipId, what: &Typing) -> Option<String> {
        use splice_core::symbol::Fill;
        let clip = self.doc.project.clip(id)?;
        match what {
            Typing::Title => match &clip.generator {
                Some(Generator::Text(t)) => Some(t.text.clone()),
                _ => None,
            },
            Typing::Label => Some(clip.placeholder.as_ref().map(|t| t.label.clone()).unwrap_or_default()),
            Typing::Hex(_) => self.title_edit.as_ref().map(|e| e.scratch.clone()),
            Typing::Fill(key) => match &self.doc.project.media(clip.media)?.recipe {
                Some(splice_core::model::Recipe::Symbol(symbol)) => match symbol.shown(&symbol.placeholder(key)?) {
                    Fill::Text { text } => Some(text),
                    _ => None,
                },
                _ => None,
            },
        }
    }

    /// Finishes typing: the words become one undo step.
    pub(super) fn end_title_edit(&mut self, cx: &mut Context<Self>) {
        use splice_core::model::Recipe;
        if let Some(edit) = self.title_edit.take() {
            match &edit.what {
                Typing::Label => {
                    // Typed into the tag as it was; now applied properly (a
                    // name another placeholder has joins it, an empty one
                    // keeps the old name).
                    let typed = self.typed_text(edit.clip, &Typing::Label).unwrap_or_default();
                    let was = edit.before.clip(edit.clip).and_then(|c| c.placeholder.clone());
                    if let Some(clip) = self.doc.project.clip_mut(edit.clip) {
                        clip.placeholder = was;
                    }
                    self.doc.project.set_placeholder(edit.clip, Some(&typed));
                }
                Typing::Fill(key) => {
                    // A fill typed back to what the symbol has is no fill.
                    let media = self.doc.project.clip(edit.clip).map(|c| c.media);
                    if let Some(media) = media
                        && let Some(Recipe::Symbol(mut symbol)) =
                            self.doc.project.media(media).and_then(|m| m.recipe.clone())
                        && let Some(p) = symbol.placeholder(key)
                    {
                        let fill = symbol.shown(&p);
                        let _ = symbol.fill(&p.id, fill);
                        self.doc.project.set_recipe(media, Recipe::Symbol(symbol));
                    }
                }
                Typing::Hex(key) => {
                    let media = self.doc.project.clip(edit.clip).map(|c| c.media);
                    match super::agent::parse_color(edit.scratch.trim()) {
                        Some(color) => {
                            if let Some(media) = media
                                && let Some(Recipe::Symbol(mut symbol)) =
                                    self.doc.project.media(media).and_then(|m| m.recipe.clone())
                                && symbol.fill(key, splice_core::symbol::Fill::Color { color }).is_ok()
                            {
                                self.doc.project.set_recipe(media, Recipe::Symbol(symbol));
                            }
                        }
                        None if !edit.scratch.trim().is_empty() => {
                            self.say_error(format!("{} is not a colour: try #ff8800", edit.scratch.trim()));
                        }
                        None => {}
                    }
                }
                Typing::Title => {}
            }
            if self.doc.project != edit.before {
                self.doc.commit(edit.before);
            }
            self.after_edit();
            self.reconcile_made(cx);
            cx.notify();
        }
    }

    fn with_title_text(&mut self, f: impl FnOnce(&mut String, &mut TitleEdit)) {
        let Some(edit) = &mut self.title_edit else { return };
        let project = &mut self.doc.project;
        match edit.what.clone() {
            Typing::Title => {
                if let Some(Generator::Text(title)) = project.clip_mut(edit.clip).and_then(|c| c.generator.as_mut()) {
                    f(&mut title.text, edit);
                }
            }
            Typing::Label => {
                if let Some(tag) = project.clip_mut(edit.clip).and_then(|c| c.placeholder.as_mut()) {
                    f(&mut tag.label, edit);
                }
            }
            Typing::Hex(_) => {
                let mut scratch = std::mem::take(&mut edit.scratch);
                f(&mut scratch, edit);
                edit.scratch = scratch;
            }
            Typing::Fill(key) => {
                let Some(media) = project.clip(edit.clip).map(|c| c.media) else {
                    return;
                };
                let Some(m) = project.media.iter_mut().find(|m| m.id == media) else {
                    return;
                };
                let Some(splice_core::model::Recipe::Symbol(symbol)) = &mut m.recipe else {
                    return;
                };
                let Some(placeholder) = symbol.placeholder(&key) else {
                    return;
                };
                if !symbol.fills.contains_key(&placeholder.id) {
                    symbol.fills.insert(placeholder.id.clone(), placeholder.default.clone());
                }
                if let Some(splice_core::symbol::Fill::Text { text }) = symbol.fills.get_mut(&placeholder.id) {
                    f(text, edit);
                }
            }
        }
    }

    pub(super) fn type_text(&mut self, typed: &str, cx: &mut Context<Self>) {
        self.with_title_text(|text, edit| {
            if edit.all {
                text.clear();
                edit.cursor = 0;
                edit.all = false;
            }
            let at = edit.cursor.min(text.len());
            text.insert_str(at, typed);
            edit.cursor = at + typed.len();
        });
        self.preview_typed_fill(cx);
        cx.notify();
    }

    pub(super) fn title_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let (cmd, alt) = (k.modifiers.platform, k.modifiers.alt);
        let prev = |text: &str, at: usize| text[..at].char_indices().next_back().map_or(0, |(i, _)| i);
        let next = |text: &str, at: usize| text[at..].chars().next().map_or(at, |c| at + c.len_utf8());
        // Where a word starts, going left.
        let word_left = |text: &str, at: usize| {
            let trimmed = text[..at].trim_end();
            trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1)
        };
        match k.key.as_str() {
            "escape" => {
                self.end_title_edit(cx);
                window.focus(&self.focus);
            }
            // One-line fields finish on Return; words take a new line.
            "enter"
                if self
                    .title_edit
                    .as_ref()
                    .is_some_and(|e| matches!(e.what, Typing::Label | Typing::Hex(_))) =>
            {
                self.end_title_edit(cx);
                window.focus(&self.focus);
            }
            "enter" => self.type_text("\n", cx),
            "backspace" => self.with_title_text(|text, edit| {
                if edit.all {
                    text.clear();
                    edit.cursor = 0;
                    edit.all = false;
                    return;
                }
                let at = edit.cursor.min(text.len());
                let from = if cmd {
                    text[..at].rfind('\n').map_or(0, |i| i + 1)
                } else if alt {
                    word_left(text, at)
                } else {
                    prev(text, at)
                };
                text.replace_range(from..at, "");
                edit.cursor = from;
            }),
            "delete" => self.with_title_text(|text, edit| {
                let at = edit.cursor.min(text.len());
                let to = next(text, at);
                text.replace_range(at..to, "");
                edit.all = false;
            }),
            "left" | "right" | "home" | "end" | "up" | "down" => self.with_title_text(|text, edit| {
                let at = edit.cursor.min(text.len());
                edit.cursor = match k.key.as_str() {
                    "left" if edit.all => 0,
                    "right" if edit.all => text.len(),
                    "left" if cmd => text[..at].rfind('\n').map_or(0, |i| i + 1),
                    "right" if cmd => text[at..].find('\n').map_or(text.len(), |i| at + i),
                    "left" if alt => word_left(text, at),
                    "left" => prev(text, at),
                    "right" => next(text, at),
                    "home" | "up" => 0,
                    _ => text.len(),
                };
                edit.all = false;
            }),
            _ if cmd || k.modifiers.control => return,
            _ => match k.key_char.clone().filter(|c| !c.chars().any(char::is_control)) {
                Some(c) => self.type_text(&c, cx),
                None => return,
            },
        }
        self.preview_typed_fill(cx);
        cx.stop_propagation();
        cx.notify();
    }

    // Fonts.

    pub(super) fn open_fonts(&mut self, clip: ClipId, window: &mut Window, cx: &mut Context<Self>) {
        let current = match self.doc.project.clip(clip).and_then(|c| c.generator.as_ref()) {
            Some(Generator::Text(t)) => t.font.clone(),
            _ => return,
        };
        let list = titles::families();
        let index = list.iter().position(|f| *f == current).unwrap_or(0);
        self.overlay = Overlay::Fonts {
            clip,
            query: String::new(),
            index,
            list,
        };
        window.focus(&self.palette_focus);
        cx.notify();
    }

    pub(super) fn font_matches(list: &[String], query: &str) -> Vec<String> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        list.iter()
            .filter(|name| words.iter().all(|w| name.to_lowercase().contains(w)))
            .cloned()
            .collect()
    }

    pub(super) fn choose_font(&mut self, clip: ClipId, font: String, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(window, cx);
        self.change_title(clip, cx, |t| t.font = font);
    }
}
