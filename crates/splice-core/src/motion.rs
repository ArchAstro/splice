//! Scenes: animation and diagrams described as data. A scene is a list of
//! layers (shapes, lines, paths, text, groups) whose values can change over
//! time through keyframes, eased or sprung entrances and exits, lines that
//! draw themselves, typed and counted text, and a camera. Everything is a
//! function of the time, so any frame renders on its own and the same scene
//! always renders the same pictures. The app crate paints them.

use serde::{Deserialize, Serialize};

/// A scene as written: JSON, by a person or an assistant.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    /// Behind everything; leave it out to see through to the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Camera>,
    pub layers: Vec<Layer>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Group,
    Rect,
    Ellipse,
    Line,
    Path,
    Text,
}

/// A number, a percentage of the canvas (`"50%"`), or keyframes
/// `[[seconds, value], [seconds, value, "ease"]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Prop {
    Number(f64),
    Percent(String),
    Keys(Vec<Key>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Number(f64),
    Percent(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Key {
    Eased(f64, Scalar, String),
    Plain(f64, Scalar),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    #[serde(rename = "type", default)]
    pub kind: Kind,
    /// For finding it again; not drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where its centre (text: its anchor) sits, in canvas pixels or `"%"`;
    /// inside a group, from the group's origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Prop>,
    /// Corner radius of a rect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<Prop>,
    /// A line's ends, from the layer's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<[Prop; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[Prop; 2]>,
    /// SVG path data, around the layer's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Letter size of text, in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    /// left, center or right; for text, also which end `x` names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Prop>,
    /// Degrees, clockwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enter: Option<Motion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<Motion>,
    /// Draws its outline from start to end over this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draw: Option<Timing>,
    /// Types its text out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typewriter: Option<Typewriter>,
    /// Counts a number up (or down) in place of its text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter: Option<Counter>,
    /// A group: each child enters this many seconds after the one before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Layer>,
}

/// A colour, or a gradient across the shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Paint {
    Color(String),
    Gradient(Gradient),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    /// `[[0, "#ef6a55"], [1, "#f2c14e"]]`
    pub stops: Vec<(f64, String)>,
    /// Degrees: 0 runs left to right, 90 top to bottom.
    #[serde(default)]
    pub angle: f64,
    /// From the shape's centre out to its edge, instead of across it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub radial: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    #[serde(default = "shadow_blur")]
    pub blur: f64,
    #[serde(default)]
    pub x: f64,
    #[serde(default = "shadow_y")]
    pub y: f64,
    #[serde(default = "shadow_color")]
    pub color: String,
}

fn shadow_blur() -> f64 {
    24.0
}
fn shadow_y() -> f64 {
    10.0
}
fn shadow_color() -> String {
    "#00000066".into()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    #[serde(default)]
    pub at: f64,
    #[serde(default = "half")]
    pub duration: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<String>,
}

fn half() -> f64 {
    0.5
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Typewriter {
    #[serde(default)]
    pub at: f64,
    /// Characters a second.
    #[serde(default = "cps")]
    pub cps: f64,
}

fn cps() -> f64 {
    24.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Counter {
    #[serde(default)]
    pub from: f64,
    pub to: f64,
    #[serde(default)]
    pub at: f64,
    #[serde(default = "one")]
    pub duration: f64,
    #[serde(default)]
    pub decimals: u32,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    /// Thousands separators.
    #[serde(default = "yes")]
    pub group: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<String>,
}

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}

/// How a layer comes in or goes out: a word (`"fade"`, `"pop"`, `"rise"`,
/// `"drop"`, `"slide-left"`, `"slide-right"`, `"zoom"`), or the details.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Motion {
    Preset(String),
    Custom(Change),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Change {
    #[serde(default)]
    pub at: f64,
    #[serde(default = "half")]
    pub duration: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<String>,
    /// Physics instead of a duration: overshoots and settles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spring: Option<Spring>,
    /// Entering: where it starts from. Exiting: `to`, where it ends.
    #[serde(default, alias = "to")]
    pub from: Offsets,
    /// A preset to start from, changed by the fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Differences from the layer's own values: multiplied (opacity, scale) or
/// added (x, y, rotate).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offsets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    #[serde(default = "stiffness")]
    pub stiffness: f64,
    #[serde(default = "damping")]
    pub damping: f64,
    #[serde(default = "one")]
    pub mass: f64,
}

fn stiffness() -> f64 {
    120.0
}
fn damping() -> f64 {
    12.0
}

impl Default for Spring {
    fn default() -> Self {
        Spring {
            stiffness: stiffness(),
            damping: damping(),
            mass: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<Prop>,
    /// The point at the centre of the frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Prop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<Prop>,
}

// Time.

/// `t` (0–1) eased by name: linear, in/out/inOut × quad, cubic, quart, expo,
/// sine, back; and `outBack`, `outElastic`, `outBounce`.
pub fn ease(name: Option<&str>, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let name = name.unwrap_or("outCubic");
    let (dir, curve) = if let Some(c) = name.strip_prefix("inOut") {
        ("inOut", c)
    } else if let Some(c) = name.strip_prefix("in") {
        ("in", c)
    } else if let Some(c) = name.strip_prefix("out") {
        ("out", c)
    } else {
        ("", name)
    };
    let f = |x: f64| -> f64 {
        match curve.to_ascii_lowercase().as_str() {
            "quad" => x * x,
            "cubic" => x * x * x,
            "quart" => x * x * x * x,
            "expo" => {
                if x <= 0.0 {
                    0.0
                } else {
                    (2f64).powf(10.0 * x - 10.0)
                }
            }
            "sine" => 1.0 - (x * std::f64::consts::FRAC_PI_2).cos(),
            "back" => {
                let c = 1.70158;
                (c + 1.0) * x * x * x - c * x * x
            }
            "elastic" => {
                if x <= 0.0 || x >= 1.0 {
                    x
                } else {
                    -(2f64.powf(10.0 * x - 10.0)) * ((x * 10.0 - 10.75) * (2.0 * std::f64::consts::PI / 3.0)).sin()
                }
            }
            "bounce" => 1.0 - bounce_out(1.0 - x),
            _ => x,
        }
    };
    match dir {
        "in" => f(t),
        "out" => 1.0 - f(1.0 - t),
        "inOut" => {
            if t < 0.5 {
                f(2.0 * t) / 2.0
            } else {
                1.0 - f(2.0 - 2.0 * t) / 2.0
            }
        }
        _ => t,
    }
}

fn bounce_out(x: f64) -> f64 {
    let (n, d) = (7.5625, 2.75);
    if x < 1.0 / d {
        n * x * x
    } else if x < 2.0 / d {
        let x = x - 1.5 / d;
        n * x * x + 0.75
    } else if x < 2.5 / d {
        let x = x - 2.25 / d;
        n * x * x + 0.9375
    } else {
        let x = x - 2.625 / d;
        n * x * x + 0.984375
    }
}

/// A spring from 0 toward 1, `t` seconds after letting go. Simulated in fixed
/// steps, so it is the same on every machine.
pub fn spring(s: Spring, t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    const STEP: f64 = 1.0 / 600.0;
    let (k, c, m) = (s.stiffness.max(1.0), s.damping.max(0.0), s.mass.max(0.01));
    let (mut x, mut v) = (0.0f64, 0.0f64);
    let steps = (t / STEP).round() as u64;
    for _ in 0..steps.min(600 * 60) {
        let a = (k * (1.0 - x) - c * v) / m;
        v += a * STEP;
        x += v * STEP;
    }
    x
}

/// Seconds a spring takes to settle within 0.1%.
pub fn spring_settles(s: Spring) -> f64 {
    let mut t = 0.0;
    let mut quiet = 0.0;
    while t < 10.0 {
        t += 1.0 / 60.0;
        if (spring(s, t) - 1.0).abs() < 0.001 {
            quiet += 1.0 / 60.0;
            if quiet > 0.2 {
                return t - 0.2;
            }
        } else {
            quiet = 0.0;
        }
    }
    t
}

/// Something that knows the canvas, to turn `"50%"` into pixels.
#[derive(Clone, Copy, Debug)]
pub struct Canvas {
    pub width: f64,
    pub height: f64,
}

fn scalar(value: &Scalar, along: f64) -> f64 {
    match value {
        Scalar::Number(n) => *n,
        Scalar::Percent(text) => percent(text, along),
    }
}

fn percent(text: &str, along: f64) -> f64 {
    let t = text.trim();
    match t.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().map_or(0.0, |p| p / 100.0 * along),
        None => t.parse().unwrap_or(0.0),
    }
}

impl Prop {
    /// Its value at `t` seconds; `along` is the canvas side `%` is of.
    pub fn at(&self, t: f64, along: f64) -> f64 {
        match self {
            Prop::Number(n) => *n,
            Prop::Percent(text) => percent(text, along),
            Prop::Keys(keys) => {
                fn point(k: &Key, along: f64) -> (f64, f64, Option<&str>) {
                    match k {
                        Key::Plain(at, v) => (*at, scalar(v, along), None),
                        Key::Eased(at, v, e) => (*at, scalar(v, along), Some(e.as_str())),
                    }
                }
                let Some(first) = keys.first().map(|k| point(k, along)) else {
                    return 0.0;
                };
                if t <= first.0 {
                    return first.1;
                }
                let mut previous = first;
                for key in &keys[1..] {
                    let next = point(key, along);
                    if t < next.0 {
                        let span = (next.0 - previous.0).max(1e-9);
                        // A key's ease shapes the move into it.
                        let p = ease(next.2.or(Some("inOutCubic")), (t - previous.0) / span);
                        return previous.1 + (next.1 - previous.1) * p;
                    }
                    previous = next;
                }
                previous.1
            }
        }
    }
}

fn prop(p: &Option<Prop>, t: f64, along: f64, default: f64) -> f64 {
    p.as_ref().map_or(default, |p| p.at(t, along))
}

impl Motion {
    /// The change spelled out, a preset filled in. Exits (`exit`) default to
    /// fading.
    pub fn change(&self, exit: bool) -> Change {
        let preset = |word: &str| -> Change {
            let mut c = Change {
                duration: 0.5,
                from: Offsets {
                    opacity: Some(0.0),
                    ..Offsets::default()
                },
                ..Change::default()
            };
            match word {
                "pop" => {
                    c.spring = Some(Spring::default());
                    c.from.scale = Some(0.0);
                }
                "zoom" => c.from.scale = Some(if exit { 1.3 } else { 0.6 }),
                "rise" => c.from.y = Some(if exit { -40.0 } else { 40.0 }),
                "drop" => c.from.y = Some(if exit { 40.0 } else { -40.0 }),
                "slide-left" => c.from.x = Some(if exit { -120.0 } else { 120.0 }),
                "slide-right" => c.from.x = Some(if exit { 120.0 } else { -120.0 }),
                "spin" => {
                    c.from.rotate = Some(-90.0);
                    c.from.scale = Some(0.4);
                    c.ease = Some("outBack".into());
                }
                _ => {}
            }
            c
        };
        match self {
            Motion::Preset(word) => preset(word),
            Motion::Custom(change) => match &change.kind {
                Some(word) => {
                    let mut c = preset(word);
                    c.at = change.at;
                    c.duration = change.duration;
                    if change.ease.is_some() {
                        c.ease = change.ease.clone();
                    }
                    if change.spring.is_some() {
                        c.spring = change.spring;
                    }
                    let f = &change.from;
                    c.from.opacity = f.opacity.or(c.from.opacity);
                    c.from.scale = f.scale.or(c.from.scale);
                    c.from.x = f.x.or(c.from.x);
                    c.from.y = f.y.or(c.from.y);
                    c.from.rotate = f.rotate.or(c.from.rotate);
                    c
                }
                None => change.clone(),
            },
        }
    }
}

impl Change {
    /// How far through it is at `t`: 0 at its start, 1 when done (a spring
    /// may pass 1 on the way).
    pub fn progress(&self, t: f64) -> f64 {
        let local = t - self.at;
        match self.spring {
            Some(s) => spring(s, local),
            None => {
                if local <= 0.0 {
                    0.0
                } else {
                    ease(self.ease.as_deref(), local / self.duration.max(1e-6))
                }
            }
        }
    }

    pub fn end(&self) -> f64 {
        self.at
            + match self.spring {
                Some(s) => spring_settles(s),
                None => self.duration,
            }
    }
}

/// A layer's values at one moment, ready to paint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub radius: f64,
    pub opacity: f64,
    pub scale: f64,
    pub rotate: f64,
    pub stroke_width: f64,
    pub size: f64,
    /// How much of its outline is drawn, 0–1.
    pub drawn: f64,
    /// The text as it reads now (typed, counted).
    pub text: String,
    pub from: (f64, f64),
    pub to: (f64, f64),
}

impl Layer {
    /// Its values at `t` seconds. `delay` shifts its entrance (a group's
    /// stagger).
    pub fn state(&self, t: f64, canvas: Canvas, delay: f64) -> State {
        let (w, h) = (canvas.width, canvas.height);
        let mut s = State {
            x: prop(&self.x, t, w, 0.0),
            y: prop(&self.y, t, h, 0.0),
            width: prop(&self.width, t, w, 100.0),
            height: prop(&self.height, t, h, 100.0),
            radius: prop(&self.radius, t, w, 0.0),
            opacity: prop(&self.opacity, t, 1.0, 1.0),
            scale: prop(&self.scale, t, 1.0, 1.0),
            rotate: prop(&self.rotate, t, 1.0, 0.0),
            stroke_width: prop(&self.stroke_width, t, w, 4.0),
            size: prop(&self.size, t, h, 48.0),
            drawn: 1.0,
            text: self.text.clone().unwrap_or_default(),
            from: (0.0, 0.0),
            to: (0.0, 0.0),
        };
        if let Some([x, y]) = &self.from {
            s.from = (x.at(t, w), y.at(t, h));
        }
        if let Some([x, y]) = &self.to {
            s.to = (x.at(t, w), y.at(t, h));
        }
        if let Some(enter) = &self.enter {
            let mut change = enter.change(false);
            change.at += delay;
            let p = change.progress(t);
            let f = &change.from;
            let start = |v: Option<f64>, rest: f64| v.unwrap_or(rest);
            s.opacity *= start(f.opacity, 1.0) + (1.0 - start(f.opacity, 1.0)) * p.min(1.0);
            s.scale *= start(f.scale, 1.0) + (1.0 - start(f.scale, 1.0)) * p;
            s.x += start(f.x, 0.0) * (1.0 - p);
            s.y += start(f.y, 0.0) * (1.0 - p);
            s.rotate += start(f.rotate, 0.0) * (1.0 - p);
        }
        if let Some(exit) = &self.exit {
            let change = exit.change(true);
            let p = change.progress(t);
            let f = &change.from;
            let end = |v: Option<f64>, rest: f64| v.unwrap_or(rest);
            s.opacity *= 1.0 + (end(f.opacity, 1.0) - 1.0) * p.min(1.0);
            s.scale *= 1.0 + (end(f.scale, 1.0) - 1.0) * p;
            s.x += end(f.x, 0.0) * p;
            s.y += end(f.y, 0.0) * p;
            s.rotate += end(f.rotate, 0.0) * p;
        }
        if let Some(draw) = &self.draw {
            let local = t - draw.at - delay;
            s.drawn = if local <= 0.0 {
                0.0
            } else {
                ease(
                    draw.ease.as_deref().or(Some("inOutCubic")),
                    local / draw.duration.max(1e-6),
                )
            };
        }
        if let Some(counter) = &self.counter {
            let local = t - counter.at - delay;
            let p = if local <= 0.0 {
                0.0
            } else {
                ease(
                    counter.ease.as_deref().or(Some("outCubic")),
                    local / counter.duration.max(1e-6),
                )
            };
            let n = counter.from + (counter.to - counter.from) * p;
            s.text = format!(
                "{}{}{}",
                counter.prefix,
                number(n, counter.decimals, counter.group),
                counter.suffix
            );
        }
        if let Some(typing) = &self.typewriter {
            let shown = ((t - typing.at - delay) * typing.cps).floor().max(0.0) as usize;
            s.text = s.text.chars().take(shown).collect();
        }
        s.opacity = s.opacity.clamp(0.0, 1.0);
        s
    }
}

/// `1234.5` → `1,234.5`.
fn number(n: f64, decimals: u32, group: bool) -> String {
    let text = format!("{:.*}", decimals as usize, n);
    if !group {
        return text;
    }
    let (sign, rest) = text.strip_prefix('-').map_or(("", text.as_str()), |r| ("-", r));
    let (whole, fraction) = rest.split_once('.').map_or((rest, None), |(w, f)| (w, Some(f)));
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match fraction {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// `#rrggbb`, `#rrggbbaa`, `#rgb`, or a few names → RGBA.
pub fn color(text: &str) -> Option<[u8; 4]> {
    let t = text.trim().to_ascii_lowercase();
    let named = match t.as_str() {
        "white" => Some("#ffffff"),
        "black" => Some("#000000"),
        "clear" | "transparent" | "none" => Some("#00000000"),
        "paper" => Some("#f3eee4"),
        "ink" => Some("#12151f"),
        "vermilion" | "red" => Some("#ef6a55"),
        "gold" | "yellow" => Some("#f2c14e"),
        "mint" | "green" => Some("#86cfc0"),
        "sky" | "blue" => Some("#6c9ef8"),
        "plum" | "purple" => Some("#c792ea"),
        "grey" | "gray" => Some("#8a8f9c"),
        _ => None,
    };
    let hex = named.unwrap_or(&t).strip_prefix('#')?.to_string();
    let hex = if hex.len() == 3 {
        hex.chars().flat_map(|c| [c, c]).collect()
    } else {
        hex
    };
    let v = u32::from_str_radix(&hex, 16).ok()?;
    match hex.len() {
        6 => Some([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]),
        8 => Some(v.to_be_bytes()),
        _ => None,
    }
}

impl Spec {
    pub fn parse(text: &str) -> anyhow::Result<Spec> {
        let spec: Spec = serde_json::from_str(text).map_err(|e| anyhow::anyhow!("the scene is not valid: {e}"))?;
        spec.check()?;
        Ok(spec)
    }

    /// What JSON alone does not catch: colours, kinds of motion, path data.
    pub fn check(&self) -> anyhow::Result<()> {
        if let Some(bg) = &self.background {
            color(bg).ok_or_else(|| anyhow::anyhow!("background: {bg} is not a colour"))?;
        }
        fn walk(layers: &[Layer], path: &str) -> anyhow::Result<()> {
            for (i, layer) in layers.iter().enumerate() {
                let here = match &layer.name {
                    Some(n) => format!("{path}{n}"),
                    None => format!("{path}layers[{i}]"),
                };
                if let Some(Paint::Color(c)) = &layer.fill {
                    color(c).ok_or_else(|| anyhow::anyhow!("{here}: fill {c} is not a colour"))?;
                }
                if let Some(Paint::Gradient(g)) = &layer.fill {
                    for (_, c) in &g.stops {
                        color(c).ok_or_else(|| anyhow::anyhow!("{here}: gradient colour {c} is not a colour"))?;
                    }
                }
                if let Some(c) = &layer.stroke {
                    color(c).ok_or_else(|| anyhow::anyhow!("{here}: stroke {c} is not a colour"))?;
                }
                for (motion, which) in [(&layer.enter, "enter"), (&layer.exit, "exit")] {
                    let word = match motion {
                        Some(Motion::Preset(w)) => Some(w.as_str()),
                        Some(Motion::Custom(c)) => c.kind.as_deref(),
                        None => None,
                    };
                    if let Some(w) = word
                        && !PRESETS.contains(&w)
                    {
                        anyhow::bail!("{here}: {which} \"{w}\" is not one of {}", PRESETS.join(", "));
                    }
                }
                match layer.kind {
                    Kind::Path if layer.d.is_none() => anyhow::bail!("{here}: a path needs `d`"),
                    Kind::Text if layer.text.is_none() && layer.counter.is_none() => {
                        anyhow::bail!("{here}: text needs `text` or a `counter`")
                    }
                    Kind::Line if layer.to.is_none() => anyhow::bail!("{here}: a line needs `to`"),
                    _ => {}
                }
                walk(&layer.children, &format!("{here}."))?;
            }
            Ok(())
        }
        walk(&self.layers, "")
    }

    /// The camera at `t`: (zoom, centre x, centre y, degrees).
    pub fn camera(&self, t: f64, canvas: Canvas) -> (f64, f64, f64, f64) {
        match &self.camera {
            None => (1.0, canvas.width / 2.0, canvas.height / 2.0, 0.0),
            Some(c) => (
                prop(&c.zoom, t, 1.0, 1.0),
                prop(&c.x, t, canvas.width, canvas.width / 2.0),
                prop(&c.y, t, canvas.height, canvas.height / 2.0),
                prop(&c.rotate, t, 1.0, 0.0),
            ),
        }
    }

    /// A scene to start from: three steps that build into a little diagram.
    pub fn starter() -> &'static str {
        STARTER
    }
}

/// The format in brief, for people and assistants writing scenes.
pub const GUIDE: &str = "A scene is JSON: { background?, camera?, layers: [layer…] }. Layers draw in order, later on top; \
anything not drawn stays see-through.
Layer: { type: rect|ellipse|line|path|text|group, name?, x, y (its centre, canvas pixels or \"50%\"; inside a group, from \
the group), width, height, radius (rect corners), from/to [x, y] (a line's ends, from its x, y), d (SVG path data), \
text, font (any installed family), size (pixels), bold, italic, align (left|center|right: which end x names), \
fill (\"#rrggbb[aa]\" or { stops: [[0, c], [1, c]], angle } or { stops, radial: true } from the centre out), stroke (colour), strokeWidth, opacity, scale, \
rotate (degrees), shadow { blur, x, y, color }, children (a group's or a shape's, placed from its centre) }.
Any number can change over time: keyframes [[seconds, value], [seconds, value, \"ease\"]].
Eases: linear, in/out/inOut + Quad|Cubic|Quart|Expo|Sine|Back|Elastic|Bounce (e.g. outBack).
enter / exit: \"fade\"|\"pop\"|\"zoom\"|\"rise\"|\"drop\"|\"slide-left\"|\"slide-right\"|\"spin\", or \
{ kind?, at, duration, ease?, spring?: { stiffness, damping, mass }, from|to: { opacity, scale, x, y, rotate } }.
draw: { at, duration, ease } draws a line, path or outline from its start. typewriter: { at, cps } types text out. \
counter: { from, to, at, duration, decimals, prefix, suffix } counts in place of text. A group's stagger: seconds \
between its children's entrances (and draws). camera: { zoom, x, y, rotate } (each may be keyframes; x, y is the \
point at the centre). Times are seconds from the scene's start.";

pub const PRESETS: &[&str] = &[
    "fade",
    "pop",
    "zoom",
    "rise",
    "drop",
    "slide-left",
    "slide-right",
    "spin",
];

const STARTER: &str = r##"{
  "layers": [
    { "type": "line", "name": "link-1", "x": "25%", "y": "50%", "from": [110, 0], "to": [  "25%", 0 ],
      "stroke": "#f3eee4", "strokeWidth": 6, "draw": { "at": 0.6, "duration": 0.5 } },
    { "type": "line", "name": "link-2", "x": "50%", "y": "50%", "from": [110, 0], "to": [ "25%", 0 ],
      "stroke": "#f3eee4", "strokeWidth": 6, "draw": { "at": 0.9, "duration": 0.5 } },
    { "type": "group", "name": "steps", "stagger": 0.25,
      "children": [
        { "type": "rect", "x": "25%", "y": "50%", "width": 200, "height": 120, "radius": 20,
          "fill": "#ef6a55", "shadow": { "blur": 28, "y": 12 }, "enter": "pop",
          "children": [ { "type": "text", "text": "Idea", "size": 38, "bold": true, "fill": "#ffffff" } ] },
        { "type": "rect", "x": "50%", "y": "50%", "width": 200, "height": 120, "radius": 20,
          "fill": "#ef6a55", "shadow": { "blur": 28, "y": 12 }, "enter": "pop",
          "children": [ { "type": "text", "text": "Draft", "size": 38, "bold": true, "fill": "#ffffff" } ] },
        { "type": "rect", "x": "75%", "y": "50%", "width": 200, "height": 120, "radius": 20,
          "fill": "#ef6a55", "shadow": { "blur": 28, "y": 12 }, "enter": "pop",
          "children": [ { "type": "text", "text": "Ship", "size": 38, "bold": true, "fill": "#ffffff" } ] }
      ] },
    { "type": "text", "x": "50%", "y": "72%", "text": "From idea to shipped, in three steps.",
      "size": 30, "fill": "#f3eee4", "typewriter": { "at": 1.2, "cps": 30 } }
  ]
}
"##;

// Editing a scene's JSON by layer name, for assistants and scripts.

use serde_json::Value;

fn layers_mut(value: &mut Value) -> Option<&mut Vec<Value>> {
    let key = if value.get("layers").is_some() {
        "layers"
    } else {
        "children"
    };
    value.get_mut(key)?.as_array_mut()
}

/// The layer named `name`, anywhere in the scene.
fn find<'a>(value: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    let list = layers_mut(value)?;
    let i = list
        .iter()
        .position(|l| l.get("name").and_then(Value::as_str) == Some(name));
    match i {
        Some(i) => list.get_mut(i),
        None => list.iter_mut().find_map(|l| find(l, name)),
    }
}

/// RFC 7386: objects merge, `null` removes, anything else replaces.
fn merge(target: &mut Value, patch: &Value) {
    match (target.as_object_mut(), patch.as_object()) {
        (Some(t), Some(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        _ => *target = patch.clone(),
    }
}

fn names(value: &Value, out: &mut Vec<String>) {
    for key in ["layers", "children"] {
        for layer in value.get(key).and_then(Value::as_array).into_iter().flatten() {
            if let Some(n) = layer.get("name").and_then(Value::as_str) {
                out.push(n.to_string());
            }
            names(layer, out);
        }
    }
}

fn missing(value: &Value, name: &str) -> anyhow::Error {
    let mut known = Vec::new();
    names(value, &mut known);
    if known.is_empty() {
        anyhow::anyhow!("no layer is named {name}; the scene names none (give layers a \"name\")")
    } else {
        anyhow::anyhow!("no layer is named {name}; named layers: {}", known.join(", "))
    }
}

fn finish(value: &Value) -> anyhow::Result<String> {
    let text = serde_json::to_string_pretty(value)? + "\n";
    Spec::parse(&text)?;
    Ok(text)
}

fn open(source: &str) -> anyhow::Result<Value> {
    serde_json::from_str(source).map_err(|e| anyhow::anyhow!("the scene is not valid JSON: {e}"))
}

/// Merges `patch` into the layer named `layer`, or into the scene itself.
pub fn patch(source: &str, layer: Option<&str>, patch: &Value) -> anyhow::Result<String> {
    let mut value = open(source)?;
    match layer {
        Some(name) => {
            let snapshot = value.clone();
            let target = find(&mut value, name).ok_or_else(|| missing(&snapshot, name))?;
            merge(target, patch);
        }
        None => merge(&mut value, patch),
    }
    finish(&value)
}

/// Adds `layer` on top of the scene, or of the group or shape named `into`.
pub fn add(source: &str, layer: Value, into: Option<&str>) -> anyhow::Result<String> {
    let mut value = open(source)?;
    let snapshot = value.clone();
    let parent = match into {
        Some(name) => find(&mut value, name).ok_or_else(|| missing(&snapshot, name))?,
        None => &mut value,
    };
    let key = if into.is_some() { "children" } else { "layers" };
    let object = parent.as_object_mut().ok_or_else(|| anyhow::anyhow!("not an object"))?;
    let list = object.entry(key).or_insert_with(|| Value::Array(Vec::new()));
    list.as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("{key} is not a list"))?
        .push(layer);
    finish(&value)
}

/// Removes the layer named `name`.
pub fn remove(source: &str, name: &str) -> anyhow::Result<String> {
    fn take(value: &mut Value, name: &str) -> bool {
        for key in ["layers", "children"] {
            if let Some(list) = value.get_mut(key).and_then(Value::as_array_mut) {
                let before = list.len();
                list.retain(|l| l.get("name").and_then(Value::as_str) != Some(name));
                if list.len() != before || list.iter_mut().any(|l| take(l, name)) {
                    return true;
                }
            }
        }
        false
    }
    let mut value = open(source)?;
    if !take(&mut value, name) {
        return Err(missing(&value, name));
    }
    finish(&value)
}

impl Change {
    fn starts(&self) -> f64 {
        self.at
    }
}

/// The scene at a glance: each layer's kind, name, words, and when it moves.
pub fn outline(spec: &Spec) -> Vec<Value> {
    fn walk(layers: &[Layer], delay: f64, out: &mut Vec<Value>, depth: usize) {
        for layer in layers {
            let mut line = serde_json::Map::new();
            line.insert("type".into(), serde_json::to_value(layer.kind).unwrap_or(Value::Null));
            if let Some(n) = &layer.name {
                line.insert("name".into(), n.clone().into());
            }
            if let Some(t) = &layer.text {
                line.insert("text".into(), t.clone().into());
            }
            if depth > 0 {
                line.insert("depth".into(), depth.into());
            }
            if let Some(m) = &layer.enter {
                let c = m.change(false);
                line.insert("enters".into(), (c.starts() + delay).into());
                line.insert("settled".into(), (c.end() + delay).into());
            }
            if let Some(d) = &layer.draw {
                line.insert(
                    "draws".into(),
                    serde_json::json!([d.at + delay, d.at + delay + d.duration]),
                );
            }
            if let Some(w) = &layer.typewriter {
                let len = layer.text.as_deref().map_or(0, |t| t.chars().count()) as f64;
                line.insert("types".into(), serde_json::json!([w.at, w.at + len / w.cps.max(0.1)]));
            }
            if let Some(c) = &layer.counter {
                line.insert(
                    "counts".into(),
                    serde_json::json!({ "from": c.from, "to": c.to, "during": [c.at, c.at + c.duration] }),
                );
            }
            if let Some(m) = &layer.exit {
                let c = m.change(true);
                line.insert("exits".into(), c.starts().into());
                line.insert("gone".into(), c.end().into());
            }
            out.push(Value::Object(line));
            walk(&layer.children, delay, out, depth + 1);
            // Staggered children: shift each one.
            if let Some(stagger) = layer.stagger {
                let first = out.len() - count(&layer.children);
                let mut i = 0;
                for (n, child) in layer.children.iter().enumerate() {
                    let shift = n as f64 * stagger;
                    for entry in out[first + i..first + i + 1 + count(&child.children)].iter_mut() {
                        for key in ["enters", "settled"] {
                            if let Some(v) = entry.get(key).and_then(Value::as_f64) {
                                entry[key] = (v + shift).into();
                            }
                        }
                    }
                    i += 1 + count(&child.children);
                }
            }
        }
    }
    fn count(layers: &[Layer]) -> usize {
        layers.iter().map(|l| 1 + count(&l.children)).sum()
    }
    let mut out = Vec::new();
    walk(&spec.layers, 0.0, &mut out, 0);
    out
}

/// Likely mistakes that still render: fonts that are not installed, motion
/// that starts after the scene ends or is still going when it does.
pub fn lint(spec: &Spec, seconds: f64, has_font: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(layers: &[Layer], path: &str, seconds: f64, has_font: &dyn Fn(&str) -> bool, out: &mut Vec<String>) {
        for (i, layer) in layers.iter().enumerate() {
            let here = layer.name.clone().unwrap_or_else(|| format!("{path}[{i}]"));
            if let Some(font) = &layer.font
                && !has_font(font)
            {
                out.push(format!(
                    "{here}: the font {font} is not installed; text falls back to another"
                ));
            }
            let mut at = Vec::new();
            if let Some(m) = &layer.enter {
                at.push(("enters", m.change(false).at));
            }
            if let Some(d) = &layer.draw {
                at.push(("draws", d.at));
            }
            if let Some(m) = &layer.exit {
                let c = m.change(true);
                at.push(("exits", c.at));
                if c.end() > seconds + 0.05 {
                    out.push(format!(
                        "{here}: its exit is still going when the scene ends at {seconds:.2}s"
                    ));
                }
            }
            if let Some(t) = &layer.typewriter {
                at.push(("types", t.at));
                let len = layer.text.as_deref().map_or(0, |t| t.chars().count()) as f64;
                if t.at + len / t.cps.max(0.1) > seconds {
                    out.push(format!("{here}: it is still typing when the scene ends"));
                }
            }
            for (what, t) in at {
                if t >= seconds {
                    out.push(format!("{here}: {what} at {t}s, after the scene ends at {seconds:.2}s"));
                }
            }
            walk(&layer.children, &format!("{here}.children"), seconds, has_font, out);
        }
    }
    walk(&spec.layers, "layers", seconds, &has_font, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: Canvas = Canvas {
        width: 1280.0,
        height: 720.0,
    };

    #[test]
    fn the_starter_reads() {
        let spec = Spec::parse(STARTER).unwrap();
        assert_eq!(spec.layers.len(), 4);
        assert_eq!(spec.layers[2].children.len(), 3);
    }

    #[test]
    fn eases_start_and_end_where_they_should() {
        for name in [
            "linear",
            "inQuad",
            "outCubic",
            "inOutExpo",
            "outBack",
            "outBounce",
            "inOutSine",
            "outElastic",
        ] {
            assert!(ease(Some(name), 0.0).abs() < 1e-9, "{name}");
            assert!((ease(Some(name), 1.0) - 1.0).abs() < 1e-9, "{name}");
        }
        assert!(ease(Some("outBack"), 0.8) > 1.0);
        assert!(ease(Some("inQuad"), 0.5) < 0.5);
    }

    #[test]
    fn springs_overshoot_and_settle() {
        let s = Spring::default();
        assert_eq!(spring(s, 0.0), 0.0);
        let peak = (1..200).map(|i| spring(s, i as f64 / 100.0)).fold(0.0, f64::max);
        assert!(peak > 1.0 && peak < 1.5, "{peak}");
        assert!((spring(s, 3.0) - 1.0).abs() < 0.01);
        assert!(spring_settles(s) < 2.0);
    }

    #[test]
    fn keyframes_and_percentages() {
        let p: Prop = serde_json::from_str(r#"[[0, "0%"], [1, "50%", "linear"], [2, 100]]"#).unwrap();
        assert_eq!(p.at(-1.0, 1000.0), 0.0);
        assert!((p.at(0.5, 1000.0) - 250.0).abs() < 1e-9);
        assert_eq!(p.at(1.0, 1000.0), 500.0);
        assert_eq!(p.at(9.0, 1000.0), 100.0);
        let p: Prop = serde_json::from_str(r#""25%""#).unwrap();
        assert_eq!(p.at(0.0, 1280.0), 320.0);
    }

    #[test]
    fn entrances_exits_and_stagger() {
        let layer: Layer = serde_json::from_str(
            r#"{ "type": "rect", "x": 100, "enter": "rise", "exit": { "at": 3, "duration": 1, "to": { "opacity": 0 } } }"#,
        )
        .unwrap();
        let before = layer.state(0.0, CANVAS, 0.0);
        assert_eq!(before.opacity, 0.0);
        assert_eq!(before.y, 40.0);
        let after = layer.state(1.0, CANVAS, 0.0);
        assert_eq!((after.opacity, after.y), (1.0, 0.0));
        let late = layer.state(1.0, CANVAS, 0.8);
        assert!(late.opacity < 1.0, "a stagger delays the entrance");
        let leaving = layer.state(3.5, CANVAS, 0.0);
        assert!(leaving.opacity < 1.0 && leaving.opacity > 0.0);
        assert_eq!(layer.state(4.0, CANVAS, 0.0).opacity, 0.0);
    }

    #[test]
    fn text_types_and_counts() {
        let layer: Layer =
            serde_json::from_str(r#"{ "type": "text", "text": "Hello", "typewriter": { "at": 0, "cps": 10 } }"#)
                .unwrap();
        assert_eq!(layer.state(0.25, CANVAS, 0.0).text, "He");
        let counter: Layer =
            serde_json::from_str(r#"{ "type": "text", "counter": { "to": 12500, "duration": 1, "prefix": "$" } }"#)
                .unwrap();
        assert_eq!(counter.state(5.0, CANVAS, 0.0).text, "$12,500");
        assert_eq!(counter.state(0.0, CANVAS, 0.0).text, "$0");
        assert_eq!(number(-1234567.891, 2, true), "-1,234,567.89");
    }

    #[test]
    fn says_what_is_wrong() {
        let e = Spec::parse(r##"{ "layers": [ { "type": "rect", "name": "box", "fill": "#zz" } ] }"##).unwrap_err();
        assert!(e.to_string().contains("box: fill"), "{e}");
        let e = Spec::parse(r#"{ "layers": [ { "type": "rect", "enter": "wobble" } ] }"#).unwrap_err();
        assert!(e.to_string().contains("not one of"), "{e}");
        assert!(Spec::parse("{ nope").is_err());
        assert_eq!(color("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(color("#00000080"), Some([0, 0, 0, 128]));
    }

    #[test]
    fn edits_layers_by_name() {
        let source = Spec::starter();
        let changed = patch(
            source,
            Some("link-1"),
            &serde_json::json!({ "stroke": "#ef6a55", "draw": null }),
        )
        .unwrap();
        let spec = Spec::parse(&changed).unwrap();
        assert_eq!(spec.layers[0].stroke.as_deref(), Some("#ef6a55"));
        assert!(spec.layers[0].draw.is_none());
        let added = add(
            &changed,
            serde_json::json!({ "type": "text", "name": "tag", "text": "New" }),
            Some("steps"),
        )
        .unwrap();
        assert_eq!(Spec::parse(&added).unwrap().layers[2].children.len(), 4);
        let removed = remove(&added, "tag").unwrap();
        assert_eq!(Spec::parse(&removed).unwrap().layers[2].children.len(), 3);
        let e = patch(source, Some("nope"), &serde_json::json!({})).unwrap_err();
        assert!(e.to_string().contains("link-1, link-2, steps"), "{e}");
        let e = patch(source, Some("link-1"), &serde_json::json!({ "stroke": "#zz" })).unwrap_err();
        assert!(e.to_string().contains("not a colour"), "{e}");
    }

    #[test]
    fn outlines_and_lints() {
        let spec = Spec::parse(Spec::starter()).unwrap();
        let outline = outline(&spec);
        let steps: Vec<f64> = outline
            .iter()
            .filter(|l| l["type"] == "rect")
            .map(|l| l["enters"].as_f64().unwrap())
            .collect();
        assert_eq!(steps, vec![0.0, 0.25, 0.5], "stagger shows");
        assert!(lint(&spec, 5.0, |_| true).is_empty());
        let late = lint(&spec, 0.7, |_| true);
        assert!(late.iter().any(|w| w.contains("draws at 0.9s")), "{late:?}");
        let fonts = lint(
            &Spec::parse(r#"{ "layers": [ { "type": "text", "text": "a", "font": "Nope Sans" } ] }"#).unwrap(),
            3.0,
            |_| false,
        );
        assert!(fonts[0].contains("not installed"));
    }
}
