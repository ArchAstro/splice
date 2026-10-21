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
        }
    }
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
pub struct PoseExpr {
    pub x: Expr,
    pub y: Expr,
    pub scale: Expr,
}

impl Look {
    /// How the clip moves over a clip of `duration`.
    pub fn pose_expr(&self, duration: Time) -> PoseExpr {
        let d = secs(duration).max(1e-3);
        let mut x = C(self.x as f64);
        let mut y = C(self.y as f64);
        let mut scale = C(self.scale as f64);
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
        PoseExpr { x, y, scale }
    }

    /// Where the clip is `local` into a clip of `duration`.
    pub fn pose(&self, duration: Time, local: Time) -> Pose {
        let e = self.pose_expr(duration);
        let t = secs(local);
        let d = secs(duration);
        let fade = |m: Motion, p: f64| {
            if m.kind == MotionKind::None {
                1.0
            } else {
                (p / secs(m.duration).max(1e-3)).clamp(0.0, 1.0)
            }
        };
        Pose {
            x: e.x.eval(t),
            y: e.y.eval(t),
            scale: e.scale.eval(t).max(0.0),
            opacity: self.opacity as f64 * fade(self.enter, t) * fade(self.exit, d - t),
        }
    }

    /// Whether anything about the clip changes over time.
    pub fn is_animated(&self) -> bool {
        self.enter.kind != MotionKind::None || self.exit.kind != MotionKind::None || self.drift != Drift::None
    }

    /// Filters that change the clip's own pixels (key and mask), shared by
    /// the viewer's decoder and export. They leave an RGBA frame.
    pub fn pixel_filters(&self) -> Option<String> {
        let mut filters = Vec::new();
        // Cut first, so a key and a mask work on what remains.
        if let Some(crop) = self.crop.filter(|c| !c.is_none()) {
            filters.push(crop.ffmpeg());
        }
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
