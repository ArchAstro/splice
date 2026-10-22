//! How a clip looks on the canvas: where it sits, how big, how opaque, a
//! green-screen key, a mask, and simple animations in and out.
//!
//! Animation is written once as a small expression over the clip's local time
//! and used twice: evaluated here for the viewer, and printed as an ffmpeg
//! expression for export, so the two cannot drift apart. Keys and masks are
//! ffmpeg filters that the viewer's decoder runs too.

use crate::time::{SECOND, Time, secs};
use serde::{Deserialize, Serialize};

/// Position, size, opacity, key, mask and motion of a visual clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// Offset of the clip's centre from the canvas centre, as fractions of
    /// the canvas width and height (right and down are positive).
    pub x: f32,
    pub y: f32,
    /// 1 is the clip's natural size: media fitted to the canvas, a title as set.
    pub scale: f32,
    pub opacity: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<Key>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
    /// Edges cut away. The picture keeps its scale, and what remains stays
    /// where it was on the canvas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
    pub enter: Motion,
    pub exit: Motion,
    pub drift: Drift,
    /// Colour: a look, and exposure, contrast, saturation, warmth and tint.
    #[serde(skip_serializing_if = "Grade::is_none")]
    pub grade: Grade,
    /// Keyframes: place, size and opacity set at moments in the clip, eased
    /// between. A property with keyframes follows them instead of its value above.
    #[serde(skip_serializing_if = "Keys::is_empty")]
    pub keys: Keys,
}

impl Default for Look {
    fn default() -> Look {
        Look {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            opacity: 1.0,
            key: None,
            mask: None,
            crop: None,
            enter: Motion::default(),
            exit: Motion::default(),
            drift: Drift::None,
            grade: Grade::default(),
            keys: Keys::default(),
        }
    }
}

/// A ready-made look, under the grade's own adjustments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    #[default]
    None,
    Warm,
    Cool,
    Vivid,
    Muted,
    Faded,
    Film,
    TealOrange,
    Mono,
    Noir,
}

impl Preset {
    pub const ALL: [Preset; 10] = [
        Preset::None,
        Preset::Warm,
        Preset::Cool,
        Preset::Vivid,
        Preset::Muted,
        Preset::Faded,
        Preset::Film,
        Preset::TealOrange,
        Preset::Mono,
        Preset::Noir,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Preset::None => "none",
            Preset::Warm => "warm",
            Preset::Cool => "cool",
            Preset::Vivid => "vivid",
            Preset::Muted => "muted",
            Preset::Faded => "faded",
            Preset::Film => "film",
            Preset::TealOrange => "teal-orange",
            Preset::Mono => "mono",
            Preset::Noir => "noir",
        }
    }

    /// As the inspector says it.
    pub fn label(self) -> &'static str {
        match self {
            Preset::None => "None",
            Preset::Warm => "Warm",
            Preset::Cool => "Cool",
            Preset::Vivid => "Vivid",
            Preset::Muted => "Muted",
            Preset::Faded => "Faded",
            Preset::Film => "Film",
            Preset::TealOrange => "Teal & orange",
            Preset::Mono => "Black & white",
            Preset::Noir => "Noir",
        }
    }

    pub fn parse(name: &str) -> Option<Preset> {
        let name = name.trim().to_lowercase().replace([' ', '_'], "-").replace('&', "");
        let name = match name.as_str() {
            "teal--orange" | "teal-and-orange" | "orange-teal" => "teal-orange",
            "black-white" | "black--white" | "black-and-white" | "bw" | "b-w" | "grey" | "gray" => "mono",
            other => other,
        }
        .to_string();
        Preset::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Its ffmpeg filters.
    fn ffmpeg(self) -> Option<&'static str> {
        Some(match self {
            Preset::None => return None,
            Preset::Warm => "colorchannelmixer=rr=1.08:gg=1.01:bb=0.88,eq=saturation=1.06",
            Preset::Cool => "colorchannelmixer=rr=0.9:gg=0.99:bb=1.1,eq=saturation=0.96",
            Preset::Vivid => "eq=contrast=1.08:saturation=1.38",
            Preset::Muted => "eq=contrast=0.96:saturation=0.66",
            Preset::Faded => "curves=all='0/0.09 0.5/0.52 1/0.9',eq=saturation=0.78",
            Preset::Film => {
                "curves=preset=medium_contrast,colorbalance=rs=-0.04:bs=0.05:rh=0.05:bh=-0.05,eq=saturation=0.9"
            }
            Preset::TealOrange => "colorbalance=rs=-0.12:gs=0.02:bs=0.12:rh=0.1:gh=0.02:bh=-0.1,eq=saturation=1.1",
            Preset::Mono => "hue=s=0",
            Preset::Noir => "hue=s=0,curves=preset=strong_contrast",
        })
    }
}

/// How a clip's colour is changed. Every adjustment is 0 when untouched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grade {
    pub preset: Preset,
    /// Stops brighter (up to 1) or darker (down to −1).
    pub exposure: f32,
    /// −1 to 1: flatter to punchier.
    pub contrast: f32,
    /// −1 (grey) to 1.
    pub saturation: f32,
    /// −1 (cooler, bluer) to 1 (warmer, more orange).
    pub temperature: f32,
    /// −1 (greener) to 1 (more magenta).
    pub tint: f32,
}

impl Grade {
    pub fn is_none(&self) -> bool {
        *self == Grade::default()
    }

    /// Every adjustment in range.
    pub fn clamped(self) -> Grade {
        let unit = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        Grade {
            preset: self.preset,
            exposure: unit(self.exposure),
            contrast: unit(self.contrast),
            saturation: unit(self.saturation),
            temperature: unit(self.temperature),
            tint: unit(self.tint),
        }
    }

    /// Its ffmpeg filters: the look first, then the adjustments over it.
    pub fn ffmpeg(&self) -> Option<String> {
        let g = self.clamped();
        let mut filters: Vec<String> = g.preset.ffmpeg().map(String::from).into_iter().collect();
        let near = |v: f32| v.abs() < 0.005;
        // Exposure as a gain, warmth and tint as a lean of the channels.
        if !(near(g.exposure) && near(g.temperature) && near(g.tint)) {
            let k = 2f32.powf(g.exposure);
            let (r, gr, b) = (
                k * (1.0 + 0.14 * g.temperature),
                k * (1.0 - 0.1 * g.tint),
                k * (1.0 - 0.14 * g.temperature),
            );
            filters.push(format!(
                "colorchannelmixer=rr={:.4}:gg={:.4}:bb={:.4}",
                r.clamp(0.0, 2.0),
                gr.clamp(0.0, 2.0),
                b.clamp(0.0, 2.0)
            ));
        }
        if !(near(g.contrast) && near(g.saturation)) {
            // Contrast eases off below the middle, so −1 is flat, not grey.
            let contrast = if g.contrast >= 0.0 {
                1.0 + 0.8 * g.contrast
            } else {
                1.0 + 0.6 * g.contrast
            };
            filters.push(format!(
                "eq=contrast={contrast:.4}:saturation={:.4}",
                (1.0 + g.saturation).clamp(0.0, 3.0)
            ));
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    /// What it does, in a few words: "Film · warmer · +0.5 stop".
    pub fn summary(&self) -> String {
        let g = self.clamped();
        let mut said = Vec::new();
        if g.preset != Preset::None {
            said.push(g.preset.label().to_string());
        }
        if g.exposure.abs() >= 0.05 {
            said.push(format!("{:+.1} stop", g.exposure));
        }
        let lean = |v: f32, up: &str, down: &str| {
            if v >= 0.05 {
                Some(up.to_string())
            } else if v <= -0.05 {
                Some(down.to_string())
            } else {
                None
            }
        };
        said.extend(lean(g.contrast, "more contrast", "less contrast"));
        said.extend(lean(g.saturation, "richer", "paler"));
        said.extend(lean(g.temperature, "warmer", "cooler"));
        said.extend(lean(g.tint, "magenta", "green"));
        if said.is_empty() {
            "Off".into()
        } else {
            said.join(" · ")
        }
    }
}

/// How a keyframed value leaves a keyframe for the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ease {
    Linear,
    /// Starts slowly.
    In,
    /// Arrives slowly.
    Out,
    /// Starts and arrives slowly.
    #[default]
    InOut,
    /// Stays put, then jumps at the next keyframe.
    Hold,
}

impl Ease {
    pub const ALL: [Ease; 5] = [Ease::Linear, Ease::In, Ease::Out, Ease::InOut, Ease::Hold];

    pub fn name(self) -> &'static str {
        match self {
            Ease::Linear => "linear",
            Ease::In => "in",
            Ease::Out => "out",
            Ease::InOut => "in-out",
            Ease::Hold => "hold",
        }
    }

    pub fn parse(name: &str) -> Option<Ease> {
        let name = name.trim().to_lowercase();
        let name = match name.as_str() {
            "ease-in" | "ease in" => "in",
            "ease-out" | "ease out" => "out",
            "ease" | "smooth" | "ease-in-out" | "inout" => "in-out",
            "step" | "none" => "hold",
            other => other,
        };
        Ease::ALL.into_iter().find(|e| e.name() == name)
    }

    /// Progress `p` (0 to 1, already clipped) eased.
    fn expr(self, p: Expr) -> Expr {
        match self {
            Ease::Linear => p,
            Ease::In => pow(p, 3.0),
            Ease::Out => sub(C(1.0), pow(sub(C(1.0), p), 3.0)),
            // Smoothstep: 3p² − 2p³.
            Ease::InOut => mul(mul(p.clone(), p.clone()), sub(C(3.0), mul(C(2.0), p))),
            Ease::Hold => C(0.0),
        }
    }
}

/// What keyframes can animate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Prop {
    X,
    Y,
    Scale,
    Opacity,
}

impl Prop {
    pub const ALL: [Prop; 4] = [Prop::X, Prop::Y, Prop::Scale, Prop::Opacity];

    pub fn name(self) -> &'static str {
        match self {
            Prop::X => "x",
            Prop::Y => "y",
            Prop::Scale => "scale",
            Prop::Opacity => "opacity",
        }
    }

    pub fn parse(name: &str) -> Option<Prop> {
        match name.trim().to_lowercase().as_str() {
            "x" | "across" => Some(Prop::X),
            "y" | "down" => Some(Prop::Y),
            "scale" | "size" => Some(Prop::Scale),
            "opacity" => Some(Prop::Opacity),
            _ => None,
        }
    }

    /// The look's own value for it.
    pub fn of(self, look: &Look) -> f32 {
        match self {
            Prop::X => look.x,
            Prop::Y => look.y,
            Prop::Scale => look.scale,
            Prop::Opacity => look.opacity,
        }
    }

    /// Its value kept in range.
    pub fn clamp(self, v: f32) -> f32 {
        let v = if v.is_finite() { v } else { 0.0 };
        match self {
            Prop::X | Prop::Y => v.clamp(-4.0, 4.0),
            Prop::Scale => v.clamp(0.01, 20.0),
            Prop::Opacity => v.clamp(0.0, 1.0),
        }
    }
}

/// A value at a moment of the clip (clip time, from its start).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub at: Time,
    pub value: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// Keyframes per property, each list in time order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keys {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub x: Vec<Keyframe>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub y: Vec<Keyframe>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scale: Vec<Keyframe>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub opacity: Vec<Keyframe>,
}

/// Keyframes closer than this are the same one.
pub const KEY_NEAR: Time = SECOND / 100;

impl Keys {
    pub fn is_empty(&self) -> bool {
        Prop::ALL.iter().all(|p| self.of(*p).is_empty())
    }

    pub fn of(&self, prop: Prop) -> &[Keyframe] {
        match prop {
            Prop::X => &self.x,
            Prop::Y => &self.y,
            Prop::Scale => &self.scale,
            Prop::Opacity => &self.opacity,
        }
    }

    pub fn of_mut(&mut self, prop: Prop) -> &mut Vec<Keyframe> {
        match prop {
            Prop::X => &mut self.x,
            Prop::Y => &mut self.y,
            Prop::Scale => &mut self.scale,
            Prop::Opacity => &mut self.opacity,
        }
    }

    /// Sets `prop` to `value` at `at`, replacing a keyframe there. A new
    /// keyframe takes the ease of the one before it.
    pub fn set(&mut self, prop: Prop, at: Time, value: f32, ease: Option<Ease>) {
        let value = prop.clamp(value);
        let keys = self.of_mut(prop);
        if let Some(k) = keys.iter_mut().find(|k| (k.at - at).abs() <= KEY_NEAR) {
            k.value = value;
            if let Some(e) = ease {
                k.ease = e;
            }
            return;
        }
        let i = keys.partition_point(|k| k.at < at);
        let ease = ease.unwrap_or_else(|| i.checked_sub(1).map_or(Ease::default(), |b| keys[b].ease));
        keys.insert(i, Keyframe { at, value, ease });
    }

    /// Takes away the keyframe at `at`; whether there was one.
    pub fn remove(&mut self, prop: Prop, at: Time) -> bool {
        let keys = self.of_mut(prop);
        let before = keys.len();
        keys.retain(|k| (k.at - at).abs() > KEY_NEAR);
        keys.len() != before
    }

    /// Every moment with a keyframe, in order.
    pub fn moments(&self) -> Vec<Time> {
        let mut at: Vec<Time> = Prop::ALL
            .iter()
            .flat_map(|p| self.of(*p).iter().map(|k| k.at))
            .collect();
        at.sort_unstable();
        at.dedup_by(|a, b| (*a - *b).abs() <= KEY_NEAR);
        at
    }

    /// Whether any property has a keyframe at `at`.
    pub fn has_at(&self, at: Time) -> bool {
        Prop::ALL
            .iter()
            .any(|p| self.of(*p).iter().any(|k| (k.at - at).abs() <= KEY_NEAR))
    }

    /// The value `prop` follows at clip time `t`, or None without keyframes.
    pub fn value(&self, prop: Prop, t: Time) -> Option<f32> {
        let keys = self.of(prop);
        (!keys.is_empty()).then(|| keyed(keys, C(0.0)).eval(secs(t)) as f32)
    }

    /// Moves every keyframe by `dt`. Those that leave the clip are kept, so
    /// trimming back out finds them again.
    pub fn shift(&mut self, dt: Time) {
        for prop in Prop::ALL {
            for k in self.of_mut(prop) {
                k.at += dt;
            }
        }
    }

    /// Stretches keyframe times by `k` (a clip played at another speed).
    pub fn scale_time(&mut self, k: f64) {
        for prop in Prop::ALL {
            for key in self.of_mut(prop) {
                key.at = (key.at as f64 * k).round() as Time;
            }
        }
    }

    /// The keyframes of the part of a clip from `from` to `to` (clip time),
    /// rebased to start there. Each animated property keeps its value at both
    /// ends, so the part moves exactly as it did.
    pub fn part(&self, from: Time, to: Time) -> Keys {
        let mut out = Keys::default();
        for prop in Prop::ALL {
            let keys = self.of(prop);
            if keys.is_empty() {
                continue;
            }
            let at = |t: Time| keyed(keys, C(0.0)).eval(secs(t)) as f32;
            let mut part: Vec<Keyframe> = keys
                .iter()
                .filter(|k| k.at > from + KEY_NEAR && k.at < to - KEY_NEAR)
                .map(|k| Keyframe { at: k.at - from, ..*k })
                .collect();
            // Each end takes the ease of the stretch it is in.
            let ease_at = |t: Time| {
                keys.iter()
                    .rev()
                    .find(|k| k.at <= t + KEY_NEAR)
                    .map_or(keys[0].ease, |k| k.ease)
            };
            part.insert(
                0,
                Keyframe {
                    at: 0,
                    value: at(from),
                    ease: ease_at(from),
                },
            );
            part.push(Keyframe {
                at: to - from,
                value: at(to),
                ease: ease_at(to),
            });
            *out.of_mut(prop) = part;
        }
        out
    }
}

/// The value keyframes give over clip time `t` (seconds), as an expression:
/// the first value, plus each step to the next eased across its stretch.
/// `t` is the expression for clip time; keyframes are in clip time.
fn keyed(keys: &[Keyframe], t: Expr) -> Expr {
    let t = if t == C(0.0) { Expr::T } else { t };
    let Some(first) = keys.first() else {
        return C(0.0);
    };
    let mut value = C(first.value as f64);
    for pair in keys.windows(2) {
        let (a, z) = (pair[0], pair[1]);
        let step = (z.value - a.value) as f64;
        if step == 0.0 {
            continue;
        }
        let progress = if a.ease == Ease::Hold || z.at <= a.at {
            // Jumps once the next keyframe is reached.
            clip01(Expr::Div(b(add(sub(t.clone(), C(secs(z.at))), C(1e-4))), b(C(1e-4))))
        } else {
            a.ease.expr(clip01(Expr::Div(
                b(sub(t.clone(), C(secs(a.at)))),
                b(C(secs(z.at - a.at))),
            )))
        };
        value = add(value, mul(C(step), progress));
    }
    value
}

/// Green (or blue, or any colour) screen: pixels near `color` turn clear.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Key {
    /// 0xRRGGBB.
    pub color: u32,
    /// How far from the colour still counts as screen (0.01 to 0.6).
    pub similarity: f32,
    /// How soft the edge is (0 to 0.5).
    pub blend: f32,
    /// Take the screen's colour cast off what remains.
    pub despill: bool,
}

impl Default for Key {
    fn default() -> Key {
        Key {
            color: 0x00ff00,
            similarity: 0.15,
            blend: 0.08,
            despill: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Rect,
    Ellipse,
}

/// Edges cut from a picture, each a fraction of its frame (0 keeps the edge).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Crop {
    /// At least a twentieth of the picture always remains.
    const KEEP: f32 = 0.05;

    /// The same crop, each edge in range and never crossing the other.
    pub fn clamped(self) -> Crop {
        let edge = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0 - Self::KEEP)
            } else {
                0.0
            }
        };
        let (mut l, mut r, mut t, mut b) = (edge(self.left), edge(self.right), edge(self.top), edge(self.bottom));
        let over = (l + r) - (1.0 - Self::KEEP);
        if over > 0.0 {
            let k = (1.0 - Self::KEEP) / (l + r);
            l *= k;
            r *= k;
        }
        let over = (t + b) - (1.0 - Self::KEEP);
        if over > 0.0 {
            let k = (1.0 - Self::KEEP) / (t + b);
            t *= k;
            b *= k;
        }
        Crop {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    pub fn is_none(&self) -> bool {
        self.left <= 0.0 && self.top <= 0.0 && self.right <= 0.0 && self.bottom <= 0.0
    }

    /// The size of what remains of a picture of `natural` size.
    pub fn size(&self, natural: (f64, f64)) -> (f64, f64) {
        (
            natural.0 * (1.0 - self.left - self.right) as f64,
            natural.1 * (1.0 - self.top - self.bottom) as f64,
        )
    }

    /// How far the centre of what remains sits from the picture's centre,
    /// in the picture's pixels at scale 1: where to draw it so it stays put.
    pub fn offset(&self, natural: (f64, f64)) -> (f64, f64) {
        (
            natural.0 * (self.left - self.right) as f64 / 2.0,
            natural.1 * (self.top - self.bottom) as f64 / 2.0,
        )
    }

    /// The ffmpeg filter that cuts it.
    pub fn ffmpeg(&self) -> String {
        format!(
            "crop=w='trunc(iw*{:.5}/2)*2':h='trunc(ih*{:.5}/2)*2':x='iw*{:.5}':y='ih*{:.5}'",
            1.0 - self.left - self.right,
            1.0 - self.top - self.bottom,
            self.left,
            self.top
        )
    }
}

/// Shows only part of the clip. Measured in fractions of the clip's own frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mask {
    pub shape: Shape,
    /// Centre.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Softness of the edge, as a fraction of the shape's size.
    pub feather: f32,
    /// Show what is outside the shape instead.
    pub invert: bool,
}

impl Default for Mask {
    fn default() -> Mask {
        Mask {
            shape: Shape::Ellipse,
            x: 0.5,
            y: 0.5,
            width: 0.8,
            height: 0.8,
            feather: 0.1,
            invert: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MotionKind {
    #[default]
    None,
    Fade,
    SlideUp,
    SlideDown,
    SlideLeft,
    SlideRight,
    Zoom,
    Pop,
}

impl MotionKind {
    pub const ALL: [MotionKind; 8] = [
        MotionKind::None,
        MotionKind::Fade,
        MotionKind::SlideUp,
        MotionKind::SlideDown,
        MotionKind::SlideLeft,
        MotionKind::SlideRight,
        MotionKind::Zoom,
        MotionKind::Pop,
    ];

    pub fn name(self) -> &'static str {
        match self {
            MotionKind::None => "none",
            MotionKind::Fade => "fade",
            MotionKind::SlideUp => "slide-up",
            MotionKind::SlideDown => "slide-down",
            MotionKind::SlideLeft => "slide-left",
            MotionKind::SlideRight => "slide-right",
            MotionKind::Zoom => "zoom",
            MotionKind::Pop => "pop",
        }
    }

    pub fn parse(name: &str) -> Option<MotionKind> {
        MotionKind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// An animation at one end of a clip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Motion {
    pub kind: MotionKind,
    pub duration: Time,
}

impl Default for Motion {
    fn default() -> Motion {
        Motion {
            kind: MotionKind::None,
            duration: SECOND / 2,
        }
    }
}

/// A slow zoom across the whole clip (the Ken Burns effect).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Drift {
    #[default]
    None,
    ZoomIn,
    ZoomOut,
}

/// How far a slide travels, as a fraction of the canvas.
const SLIDE: f64 = 0.12;
/// How much a drift zooms over the clip.
const DRIFT: f64 = 0.15;

/// Where a clip is at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub opacity: f64,
}

/// A number over local time `t` in seconds, printable as an ffmpeg expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    C(f64),
    T,
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Clip(Box<Expr>, f64, f64),
    Pow(Box<Expr>, f64),
}

use Expr::C;

fn b(e: Expr) -> Box<Expr> {
    Box::new(e)
}
fn add(a: Expr, z: Expr) -> Expr {
    match (&a, &z) {
        (C(x), C(y)) => C(x + y),
        (C(x), _) if *x == 0.0 => z,
        (_, C(y)) if *y == 0.0 => a,
        _ => Expr::Add(b(a), b(z)),
    }
}
fn sub(a: Expr, z: Expr) -> Expr {
    match (&a, &z) {
        (C(x), C(y)) => C(x - y),
        (_, C(y)) if *y == 0.0 => a,
        _ => Expr::Sub(b(a), b(z)),
    }
}
fn mul(a: Expr, z: Expr) -> Expr {
    match (&a, &z) {
        (C(x), C(y)) => C(x * y),
        (C(x), _) if *x == 1.0 => z,
        (_, C(y)) if *y == 1.0 => a,
        (C(x), _) | (_, C(x)) if *x == 0.0 => C(0.0),
        _ => Expr::Mul(b(a), b(z)),
    }
}
fn clip01(a: Expr) -> Expr {
    match a {
        C(x) => C(x.clamp(0.0, 1.0)),
        a => Expr::Clip(b(a), 0.0, 1.0),
    }
}
fn pow(a: Expr, n: f64) -> Expr {
    match a {
        C(x) => C(x.powf(n)),
        a => Expr::Pow(b(a), n),
    }
}

impl Expr {
    pub fn eval(&self, t: f64) -> f64 {
        match self {
            C(c) => *c,
            Expr::T => t,
            Expr::Add(a, z) => a.eval(t) + z.eval(t),
            Expr::Sub(a, z) => a.eval(t) - z.eval(t),
            Expr::Mul(a, z) => a.eval(t) * z.eval(t),
            Expr::Div(a, z) => a.eval(t) / z.eval(t),
            Expr::Clip(a, lo, hi) => a.eval(t).clamp(*lo, *hi),
            Expr::Pow(a, n) => a.eval(t).powf(*n),
        }
    }

    pub fn constant(&self) -> Option<f64> {
        match self {
            C(c) => Some(*c),
            _ => None,
        }
    }

    /// As an ffmpeg expression, with local time written as `t`.
    pub fn ffmpeg(&self, t: &str) -> String {
        match self {
            C(c) if *c < 0.0 => format!("({c:.6})"),
            C(c) => format!("{c:.6}"),
            Expr::T => t.to_string(),
            Expr::Add(a, z) => format!("({}+{})", a.ffmpeg(t), z.ffmpeg(t)),
            Expr::Sub(a, z) => format!("({}-{})", a.ffmpeg(t), z.ffmpeg(t)),
            Expr::Mul(a, z) => format!("({}*{})", a.ffmpeg(t), z.ffmpeg(t)),
            Expr::Div(a, z) => format!("({}/{})", a.ffmpeg(t), z.ffmpeg(t)),
            Expr::Clip(a, lo, hi) => format!("clip({},{lo:.6},{hi:.6})", a.ffmpeg(t)),
            Expr::Pow(a, n) => format!("pow({},{n:.6})", a.ffmpeg(t)),
        }
    }
}

/// The clip's pose over its local time, as expressions.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseExpr {
    pub x: Expr,
    pub y: Expr,
    pub scale: Expr,
    pub opacity: Expr,
}

impl PoseExpr {
    /// The pose `t` seconds into the clip.
    pub fn at(&self, t: f64) -> Pose {
        Pose {
            x: self.x.eval(t),
            y: self.y.eval(t),
            scale: self.scale.eval(t).max(0.0),
            opacity: self.opacity.eval(t).clamp(0.0, 1.0),
        }
    }

    /// Whether where it is or how big changes over time.
    pub fn moves(&self) -> bool {
        self.x.constant().is_none() || self.y.constant().is_none() || self.scale.constant().is_none()
    }
}

/// Builders for expressions, for other modules that animate clips.
pub mod build {
    use super::Expr;
    pub use super::Expr::{C, T};

    pub fn add(a: Expr, z: Expr) -> Expr {
        super::add(a, z)
    }
    pub fn sub(a: Expr, z: Expr) -> Expr {
        super::sub(a, z)
    }
    pub fn mul(a: Expr, z: Expr) -> Expr {
        super::mul(a, z)
    }
    pub fn div(a: Expr, z: Expr) -> Expr {
        match (&a, &z) {
            (Expr::C(x), Expr::C(y)) if *y != 0.0 => Expr::C(x / y),
            _ => Expr::Div(Box::new(a), Box::new(z)),
        }
    }
    pub fn clip01(a: Expr) -> Expr {
        super::clip01(a)
    }
    /// Starts and arrives slowly: 3p² − 2p³.
    pub fn smooth(p: Expr) -> Expr {
        super::Ease::InOut.expr(p)
    }
}

impl Look {
    /// The value `prop` has at clip time `t`: its keyframes', or its own.
    pub fn value(&self, prop: Prop, t: Time) -> f32 {
        self.keys.value(prop, t).unwrap_or_else(|| prop.of(self))
    }

    /// Sets `prop` at clip time `at`: a keyframe there when it is animated,
    /// otherwise its value throughout.
    pub fn set_value(&mut self, prop: Prop, at: Time, v: f32) {
        if !self.keys.of(prop).is_empty() {
            self.keys.set(prop, at, v, None);
            return;
        }
        let v = prop.clamp(v);
        match prop {
            Prop::X => self.x = v,
            Prop::Y => self.y = v,
            Prop::Scale => self.scale = v,
            Prop::Opacity => self.opacity = v,
        }
    }

    /// `prop` over clip time, as an expression.
    fn prop_expr(&self, prop: Prop) -> Expr {
        match self.keys.of(prop) {
            [] => C(prop.of(self) as f64),
            keys => keyed(keys, Expr::T),
        }
    }

    /// How the clip moves over a clip of `duration`.
    pub fn pose_expr(&self, duration: Time) -> PoseExpr {
        let d = secs(duration).max(1e-3);
        let mut x = self.prop_expr(Prop::X);
        let mut y = self.prop_expr(Prop::Y);
        let mut scale = self.prop_expr(Prop::Scale);
        let mut opacity = self.prop_expr(Prop::Opacity);
        // Progress into the clip (enter) and out of it (exit), 0 to 1.
        let ends = [
            (
                self.enter,
                clip01(Expr::Div(b(Expr::T), b(C(secs(self.enter.duration).max(1e-3))))),
            ),
            (
                self.exit,
                clip01(Expr::Div(
                    b(sub(C(d), Expr::T)),
                    b(C(secs(self.exit.duration).max(1e-3))),
                )),
            ),
        ];
        for (motion, p) in ends {
            // Every motion fades as it moves.
            if motion.kind != MotionKind::None {
                opacity = mul(opacity, p.clone());
            }
            let eased = sub(C(1.0), pow(sub(C(1.0), p.clone()), 3.0));
            let rest = sub(C(1.0), eased.clone());
            match motion.kind {
                MotionKind::None | MotionKind::Fade => {}
                MotionKind::SlideUp => y = add(y, mul(C(SLIDE), rest)),
                MotionKind::SlideDown => y = sub(y, mul(C(SLIDE), rest)),
                MotionKind::SlideLeft => x = add(x, mul(C(SLIDE), rest)),
                MotionKind::SlideRight => x = sub(x, mul(C(SLIDE), rest)),
                MotionKind::Zoom => scale = mul(scale, add(C(0.7), mul(C(0.3), eased))),
                MotionKind::Pop => {
                    // Overshoots a little before settling.
                    let q = sub(p, C(1.0));
                    let back = add(
                        C(1.0),
                        add(mul(C(2.70158), pow(q.clone(), 3.0)), mul(C(1.70158), pow(q, 2.0))),
                    );
                    scale = mul(scale, back);
                }
            }
        }
        let progress = clip01(Expr::Div(b(Expr::T), b(C(d))));
        match self.drift {
            Drift::None => {}
            Drift::ZoomIn => scale = mul(scale, add(C(1.0), mul(C(DRIFT), progress))),
            Drift::ZoomOut => scale = mul(scale, sub(C(1.0 + DRIFT), mul(C(DRIFT), progress))),
        }
        PoseExpr { x, y, scale, opacity }
    }

    /// Where the clip is `local` into a clip of `duration`.
    pub fn pose(&self, duration: Time, local: Time) -> Pose {
        self.pose_expr(duration).at(secs(local))
    }

    /// Whether anything about the clip changes over time.
    pub fn is_animated(&self) -> bool {
        self.enter.kind != MotionKind::None
            || self.exit.kind != MotionKind::None
            || self.drift != Drift::None
            || !self.keys.is_empty()
    }

    /// Filters that change the clip's own pixels (crop, colour, key and
    /// mask), shared by the viewer's decoder and export. They leave an RGBA frame.
    pub fn pixel_filters(&self) -> Option<String> {
        let mut filters = Vec::new();
        // Cut first, so a key and a mask work on what remains.
        if let Some(crop) = self.crop.filter(|c| !c.is_none()) {
            filters.push(crop.ffmpeg());
        }
        // Colour before the key would change what counts as green: key first.
        let graded = self.grade.ffmpeg();
        if let Some(key) = self.key {
            filters.push(format!(
                "chromakey=0x{:06x}:{:.4}:{:.4}",
                key.color & 0xffffff,
                key.similarity.clamp(0.01, 0.6),
                key.blend.clamp(0.0, 0.5)
            ));
            if key.despill {
                let (r, g, bl) = ((key.color >> 16) & 0xff, (key.color >> 8) & 0xff, key.color & 0xff);
                if g >= bl && g > r {
                    filters.push("despill=type=green".into());
                } else if bl > g && bl > r {
                    filters.push("despill=type=blue".into());
                }
            }
        }
        if let Some(grade) = graded {
            filters.push(grade);
        }
        if let Some(mask) = self.mask {
            filters.push("format=rgba".into());
            filters.push(format!(
                "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*{}'",
                mask.ffmpeg()
            ));
        }
        (!filters.is_empty()).then(|| {
            filters.push("format=rgba".into());
            filters.join(",")
        })
    }
}

impl Look {
    /// A picture's size at scale 1 once cropped, and where to shift it so
    /// what remains stays in place (canvas pixels).
    pub fn cropped(&self, natural: (f64, f64)) -> ((f64, f64), (f64, f64)) {
        match self.crop.filter(|c| !c.is_none()) {
            Some(c) => (c.size(natural), c.offset(natural)),
            None => (natural, (0.0, 0.0)),
        }
    }
}

impl Mask {
    /// How much of the clip shows at (u, v), fractions of its frame: 0 to 1.
    pub fn alpha(&self, u: f32, v: f32) -> f32 {
        let dx = (u - self.x) / (self.width / 2.0).max(1e-4);
        let dy = (v - self.y) / (self.height / 2.0).max(1e-4);
        let d = match self.shape {
            Shape::Ellipse => (dx * dx + dy * dy).sqrt(),
            Shape::Rect => dx.abs().max(dy.abs()),
        };
        let a = ((1.0 - d) / self.feather.max(1e-3)).clamp(0.0, 1.0);
        if self.invert { 1.0 - a } else { a }
    }

    /// The same as `alpha`, for ffmpeg's `geq` (`X`, `Y`, `W`, `H` are pixels).
    pub fn ffmpeg(&self) -> String {
        let dx = format!("(X/W-{:.6})/{:.6}", self.x, (self.width / 2.0).max(1e-4));
        let dy = format!("(Y/H-{:.6})/{:.6}", self.y, (self.height / 2.0).max(1e-4));
        let d = match self.shape {
            Shape::Ellipse => format!("hypot({dx},{dy})"),
            Shape::Rect => format!("max(abs({dx}),abs({dy}))"),
        };
        let a = format!("clip((1-{d})/{:.6},0,1)", self.feather.max(1e-3));
        if self.invert { format!("(1-{a})") } else { a }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_keep_what_remains_in_place() {
        let crop = Crop {
            left: 0.25,
            top: 0.0,
            right: 0.0,
            bottom: 0.5,
        };
        assert_eq!(crop.size((400.0, 200.0)), (300.0, 100.0));
        // A quarter off the left: what remains sits 50 px right of centre.
        assert_eq!(crop.offset((400.0, 200.0)), (50.0, -50.0));
        let wild = Crop {
            left: 0.8,
            right: 0.8,
            top: f32::NAN,
            bottom: -1.0,
        }
        .clamped();
        assert!(wild.left + wild.right <= 0.95 + 1e-6 && wild.top == 0.0 && wild.bottom == 0.0);
        let look = Look {
            crop: Some(crop),
            ..Look::default()
        };
        assert!(look.pixel_filters().unwrap().starts_with("crop="));
    }

    #[test]
    fn still_clips_do_not_move() {
        let look = Look::default();
        assert!(!look.is_animated());
        let p = look.pose(4 * SECOND, SECOND);
        assert_eq!((p.x, p.y, p.scale, p.opacity), (0.0, 0.0, 1.0, 1.0));
        let e = look.pose_expr(4 * SECOND);
        assert_eq!(e.scale.constant(), Some(1.0), "no animation folds to a constant");
    }

    #[test]
    fn motions_settle_in_the_middle() {
        let mut look = Look {
            x: 0.1,
            ..Look::default()
        };
        look.enter.kind = MotionKind::SlideUp;
        look.exit.kind = MotionKind::Pop;
        let d = 4 * SECOND;
        let start = look.pose(d, 0);
        assert!((start.y - SLIDE).abs() < 1e-9 && start.opacity == 0.0);
        let middle = look.pose(d, 2 * SECOND);
        assert!((middle.x - 0.1).abs() < 1e-6 && middle.y.abs() < 1e-9 && (middle.scale - 1.0).abs() < 1e-9);
        assert_eq!(middle.opacity, 1.0);
        let end = look.pose(d, d);
        assert!(end.scale.abs() < 1e-9, "pop shrinks to nothing on the way out");
    }

    #[test]
    fn drift_zooms_across_the_clip() {
        let look = Look {
            drift: Drift::ZoomIn,
            ..Look::default()
        };
        assert!((look.pose(10 * SECOND, 0).scale - 1.0).abs() < 1e-9);
        assert!((look.pose(10 * SECOND, 10 * SECOND).scale - 1.15).abs() < 1e-9);
    }

    #[test]
    fn expressions_print_for_ffmpeg() {
        let mut look = Look::default();
        look.enter.kind = MotionKind::Zoom;
        let e = look.pose_expr(2 * SECOND);
        let printed = e.scale.ffmpeg("t");
        assert!(printed.contains("clip((t/0.500000),0.000000,1.000000)"), "{printed}");
    }

    #[test]
    fn masks() {
        let m = Mask::default();
        assert_eq!(m.alpha(0.5, 0.5), 1.0);
        assert_eq!(m.alpha(0.0, 0.0), 0.0);
        let inv = Mask { invert: true, ..m };
        assert_eq!(inv.alpha(0.5, 0.5), 0.0);
        let look = Look {
            key: Some(Key::default()),
            mask: Some(m),
            ..Look::default()
        };
        let f = look.pixel_filters().unwrap();
        assert!(f.starts_with("chromakey=0x00ff00:") && f.contains("despill=type=green") && f.contains("geq="));
        assert!(Look::default().pixel_filters().is_none());
    }
}
