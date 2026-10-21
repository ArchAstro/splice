//! Scenes, painted: each frame of a `motion::Spec` drawn with tiny-skia
//! (shapes, lines, paths, gradients) and cosmic-text (words), then encoded by
//! ffmpeg to ProRes 4444, so what a scene leaves empty stays see-through on
//! the timeline. While a scene is edited its JSON also lives in a file, so a
//! text editor or an assistant can change it and Splice picks that up.

use crate::session::dirs;
use crate::titles::{self, Picture};
use anyhow::{Context, Result};
use splice_core::model::{Align, Scene, Settings};
use splice_core::motion::{self, Canvas, Kind, Layer, Paint, Spec};
use splice_core::time::SECOND;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint as Brush, PathBuilder, Pixmap, PixmapPaint,
    Point, Rect, Shader, SpreadMode, Stroke, StrokeDash, Transform,
};

/// Renders by this painter name the same bytes.
pub const ENGINE: &str = "splice-scene-2|native";

/// The file a scene's JSON lives in while it is edited.
pub const SOURCE: &str = "scene.json";

/// The folder a scene's file lives in.
pub fn workspace(scene: &Scene) -> PathBuf {
    dirs().cache.join("scenes").join(&scene.workspace)
}

/// A fresh name for a new scene's folder.
pub fn new_workspace() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    blake3::hash(format!("{nanos}-{}", std::process::id()).as_bytes()).to_hex()[..16].to_string()
}

/// Writes the scene's JSON to its file (if it changed) and returns the path.
pub fn write_file(scene: &Scene) -> Result<PathBuf> {
    let dir = workspace(scene);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(SOURCE);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(scene.source.as_str()) {
        std::fs::write(&path, &scene.source)?;
    }
    Ok(path)
}

/// The scene's JSON as it is on disk now, and when it last changed.
pub fn read_source(scene: &Scene) -> Option<(String, std::time::SystemTime)> {
    let path = workspace(scene).join(SOURCE);
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    Some((std::fs::read_to_string(&path).ok()?, modified))
}

/// Paints frames of one scene, keeping drawn words between frames.
pub struct Painter {
    spec: Spec,
    canvas: Canvas,
    width: u32,
    height: u32,
    words: HashMap<String, Option<Pixmap>>,
}

fn rgba(text: &str) -> [u8; 4] {
    motion::color(text).unwrap_or([255, 255, 255, 255])
}

fn brush(color: [u8; 4], opacity: f64) -> Brush<'static> {
    let mut brush = Brush {
        anti_alias: true,
        ..Brush::default()
    };
    brush.set_color_rgba8(
        color[0],
        color[1],
        color[2],
        (color[3] as f64 * opacity).round().clamp(0.0, 255.0) as u8,
    );
    brush
}

/// A rectangle `w` by `h` around the origin, corners rounded by `r`.
fn rounded(w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let (x0, y0, x1, y1) = (-w / 2.0, -h / 2.0, w / 2.0, h / 2.0);
    let r = r.clamp(0.0, w.min(h) / 2.0);
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_ltrb(x0, y0, x1, y1)?));
    }
    // Circle quarters as cubics.
    let k = r * 0.552_284_8;
    let mut p = PathBuilder::new();
    p.move_to(x0 + r, y0);
    p.line_to(x1 - r, y0);
    p.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    p.line_to(x1, y1 - r);
    p.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    p.line_to(x0 + r, y1);
    p.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    p.line_to(x0, y0 + r);
    p.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    p.close();
    p.finish()
}

fn svg_path(d: &str) -> Option<tiny_skia::Path> {
    use svgtypes::{SimplePathSegment as S, SimplifyingPathParser};
    let mut p = PathBuilder::new();
    for segment in SimplifyingPathParser::from(d) {
        match segment.ok()? {
            S::MoveTo { x, y } => p.move_to(x as f32, y as f32),
            S::LineTo { x, y } => p.line_to(x as f32, y as f32),
            S::CurveTo { x1, y1, x2, y2, x, y } => {
                p.cubic_to(x1 as f32, y1 as f32, x2 as f32, y2 as f32, x as f32, y as f32)
            }
            S::Quadratic { x1, y1, x, y } => p.quad_to(x1 as f32, y1 as f32, x as f32, y as f32),
            S::ClosePath => p.close(),
        }
    }
    p.finish()
}

/// How long a path's outline is, curves walked in short steps.
fn length(path: &tiny_skia::Path) -> f32 {
    use tiny_skia::PathSegment as S;
    let mut total = 0.0;
    let (mut at, mut start) = (Point::zero(), Point::zero());
    let dist = |a: Point, b: Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    for segment in path.segments() {
        match segment {
            S::MoveTo(p) => {
                at = p;
                start = p;
            }
            S::LineTo(p) => {
                total += dist(at, p);
                at = p;
            }
            S::QuadTo(c, p) => {
                let mut prev = at;
                for i in 1..=16 {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    let q = Point::from_xy(
                        u * u * at.x + 2.0 * u * t * c.x + t * t * p.x,
                        u * u * at.y + 2.0 * u * t * c.y + t * t * p.y,
                    );
                    total += dist(prev, q);
                    prev = q;
                }
                at = p;
            }
            S::CubicTo(c1, c2, p) => {
                let mut prev = at;
                for i in 1..=24 {
                    let t = i as f32 / 24.0;
                    let u = 1.0 - t;
                    let q = Point::from_xy(
                        u * u * u * at.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * p.x,
                        u * u * u * at.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * p.y,
                    );
                    total += dist(prev, q);
                    prev = q;
                }
                at = p;
            }
            S::Close => {
                total += dist(at, start);
                at = start;
            }
        }
    }
    total
}

fn align(layer: &Layer) -> Align {
    match layer.align.as_deref() {
        Some("left") => Align::Left,
        Some("right") => Align::Right,
        _ => Align::Center,
    }
}

fn to_pixmap(picture: &Picture) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(picture.width, picture.height)?;
    for (dst, src) in pixmap.pixels_mut().iter_mut().zip(picture.rgba.chunks_exact(4)) {
        *dst = tiny_skia::ColorU8::from_rgba(src[0], src[1], src[2], src[3]).premultiply();
    }
    Some(pixmap)
}

impl Painter {
    pub fn new(spec: Spec, width: u32, height: u32) -> Painter {
        Painter {
            spec,
            canvas: Canvas {
                width: width as f64,
                height: height as f64,
            },
            width,
            height,
            words: HashMap::new(),
        }
    }

    /// The picture at `t` seconds, premultiplied RGBA.
    pub fn frame(&mut self, t: f64) -> Pixmap {
        let mut pixmap = Pixmap::new(self.width.max(1), self.height.max(1)).expect("a canvas");
        if let Some(bg) = self.spec.background.as_deref().and_then(motion::color) {
            pixmap.fill(Color::from_rgba8(bg[0], bg[1], bg[2], bg[3]));
        }
        let (zoom, cx, cy, turn) = self.spec.camera(t, self.canvas);
        let camera = Transform::from_translate(self.canvas.width as f32 / 2.0, self.canvas.height as f32 / 2.0)
            .pre_rotate(turn as f32)
            .pre_scale(zoom as f32, zoom as f32)
            .pre_translate(-cx as f32, -cy as f32);
        let layers = std::mem::take(&mut self.spec.layers);
        for layer in &layers {
            self.layer(&mut pixmap, layer, t, camera, 1.0, 0.0);
        }
        self.spec.layers = layers;
        pixmap
    }

    fn layer(&mut self, pixmap: &mut Pixmap, layer: &Layer, t: f64, parent: Transform, fade: f64, delay: f64) {
        let s = layer.state(t, self.canvas, delay);
        let opacity = fade * s.opacity;
        if opacity <= 0.002 || s.scale.abs() < 1e-4 {
            return;
        }
        let at = parent
            .pre_translate(s.x as f32, s.y as f32)
            .pre_rotate(s.rotate as f32)
            .pre_scale(s.scale as f32, s.scale as f32);
        let (w, h) = (s.width as f32, s.height as f32);
        let outline = match layer.kind {
            Kind::Rect => rounded(w, h, s.radius as f32),
            Kind::Ellipse => Rect::from_xywh(-w / 2.0, -h / 2.0, w, h).and_then(PathBuilder::from_oval),
            Kind::Line => {
                let mut p = PathBuilder::new();
                p.move_to(s.from.0 as f32, s.from.1 as f32);
                p.line_to(s.to.0 as f32, s.to.1 as f32);
                p.finish()
            }
            Kind::Path => layer.d.as_deref().and_then(svg_path),
            Kind::Text | Kind::Group => None,
        };
        if let Some(path) = &outline {
            if let Some(shadow) = &layer.shadow
                && layer.kind != Kind::Line
            {
                shade(pixmap, path, shadow, at, opacity * s.drawn);
            }
            // A line, or a shape given only a stroke, is drawn as an outline.
            let stroked = layer.stroke.is_some() || layer.kind == Kind::Line;
            let filled = layer.fill.is_some() || (!stroked && layer.kind != Kind::Path);
            if filled {
                let fill_opacity = opacity * if layer.draw.is_some() { s.drawn } else { 1.0 };
                let brush = match &layer.fill {
                    Some(Paint::Gradient(g)) => gradient(g, w, h, fill_opacity),
                    Some(Paint::Color(c)) => brush(rgba(c), fill_opacity),
                    None => brush([255, 255, 255, 255], fill_opacity),
                };
                pixmap.fill_path(path, &brush, FillRule::Winding, at, None);
            }
            if stroked && s.drawn > 0.0 {
                let mut stroke = Stroke {
                    width: s.stroke_width.max(0.1) as f32,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                };
                if s.drawn < 1.0 {
                    let total = length(path).max(1.0);
                    stroke.dash = StrokeDash::new(vec![(total * s.drawn as f32).max(0.01), total + 1.0], 0.0);
                }
                let color = layer.stroke.as_deref().map_or([255, 255, 255, 255], rgba);
                pixmap.stroke_path(path, &brush(color, opacity), &stroke, at, None);
            }
        }
        if layer.kind == Kind::Text && !s.text.is_empty() {
            self.words(pixmap, layer, &s, at, opacity);
        }
        let stagger = layer.stagger.unwrap_or(0.0);
        for (i, child) in layer.children.iter().enumerate() {
            self.layer(pixmap, child, t, at, opacity, delay + i as f64 * stagger);
        }
    }

    fn words(&mut self, pixmap: &mut Pixmap, layer: &Layer, s: &motion::State, at: Transform, opacity: f64) {
        let color = match &layer.fill {
            Some(Paint::Color(c)) => rgba(c),
            _ => [255, 255, 255, 255],
        };
        let font = layer.font.clone().unwrap_or_else(|| "Helvetica Neue".into());
        let (bold, italic) = (layer.bold.unwrap_or(false), layer.italic.unwrap_or(false));
        let wrap = layer.width.as_ref().map(|_| s.width as f32);
        let key = format!(
            "{font}|{}|{bold}|{italic}|{color:?}|{:?}|{wrap:?}|{}",
            s.size.round(),
            layer.align,
            s.text
        );
        let words = self
            .words
            .entry(key)
            .or_insert_with(|| {
                let picture =
                    titles::text_picture(&s.text, &font, s.size as f32, bold, italic, color, align(layer), wrap);
                to_pixmap(&picture)
            })
            .clone();
        let Some(words) = words else { return };
        let (ww, wh) = (words.width() as f32, words.height() as f32);
        let x = match align(layer) {
            Align::Left => 0.0,
            Align::Center => -ww / 2.0,
            Align::Right => -ww,
        };
        let paint = PixmapPaint {
            opacity: opacity as f32,
            quality: tiny_skia::FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        pixmap.draw_pixmap(0, 0, words.as_ref(), &paint, at.pre_translate(x, -wh / 2.0), None);
    }
}

/// A soft shadow: the shape spread out in faint layers.
fn shade(pixmap: &mut Pixmap, path: &tiny_skia::Path, shadow: &motion::Shadow, at: Transform, opacity: f64) {
    const STEPS: usize = 10;
    let color = rgba(&shadow.color);
    let blur = shadow.blur.max(0.0) as f32;
    let place = at.pre_translate(shadow.x as f32, shadow.y as f32);
    let brush = brush(color, opacity / STEPS as f64);
    for i in 0..STEPS {
        // Rings bunch toward the shape, so the shadow fades out gently.
        let spread = blur * ((i as f32 + 0.5) / STEPS as f32).powf(1.6);
        pixmap.fill_path(path, &brush, FillRule::Winding, place, None);
        if spread > 0.0 {
            let stroke = Stroke {
                width: spread * 2.0,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            pixmap.stroke_path(path, &brush, &stroke, place, None);
        }
    }
}

fn gradient(g: &motion::Gradient, w: f32, h: f32, opacity: f64) -> Brush<'static> {
    let angle = (g.angle as f32).to_radians();
    let (dx, dy) = (angle.cos() * w / 2.0, angle.sin() * h / 2.0);
    let stops: Vec<GradientStop> = g
        .stops
        .iter()
        .map(|(at, c)| {
            let c = rgba(c);
            GradientStop::new(
                *at as f32,
                Color::from_rgba8(c[0], c[1], c[2], (c[3] as f64 * opacity).round() as u8),
            )
        })
        .collect();
    let shader = LinearGradient::new(
        Point::from_xy(-dx, -dy),
        Point::from_xy(dx, dy),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(Color::WHITE));
    Brush {
        shader,
        anti_alias: true,
        ..Brush::default()
    }
}

/// Straight-alpha RGBA, as ffmpeg's `rgba` wants it.
fn straight(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.pixels() {
        let c = px.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

/// Renders the scene to `out` (a .mov), calling `progress` with 0–1. Blocking.
pub fn render(settings: &Settings, scene: &Scene, out: &Path, mut progress: impl FnMut(f32)) -> Result<()> {
    let spec = Spec::parse(&scene.source)?;
    let (w, h, fps) = (settings.width.max(2), settings.height.max(2), settings.fps.max(1));
    let frames = ((scene.duration as f64 / SECOND as f64) * fps as f64).round().max(1.0) as u64;
    let partial = out.with_extension("partial.mov");
    let mut command = crate::ffmpeg::export_command()?;
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
        ])
        .args(["-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "-"])
        .args(["-c:v", "prores_ks", "-profile:v"]);
    if scene.transparent {
        command.args(["4", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16"]);
    } else {
        command.args(["3", "-pix_fmt", "yuv422p10le"]);
    }
    command
        .args(["-an", "-f", "mov"])
        .arg(&partial)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("starting ffmpeg")?;
    let mut stdin = child.stdin.take().expect("piped");
    // Paint on this thread while ffmpeg encodes on its own.
    let mut painter = Painter::new(spec, w, h);
    let mut failed = false;
    for f in 0..frames {
        let pixmap = painter.frame(f as f64 / fps as f64);
        if stdin.write_all(&straight(&pixmap)).is_err() {
            failed = true;
            break;
        }
        progress((f + 1) as f32 / frames as f32);
    }
    drop(stdin);
    let output = child.wait_with_output()?;
    if !output.status.success() || failed {
        let _ = std::fs::remove_file(&partial);
        anyhow::bail!(
            "encoding the scene failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    std::fs::rename(&partial, out)?;
    Ok(())
}

/// Light and dark squares under a picture, so see-through reads as such.
fn checker(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32) {
    const SQUARE: f32 = 12.0;
    let (light, dark) = (brush([0x3a, 0x40, 0x52, 255], 1.0), brush([0x2a, 0x2f, 0x3d, 255], 1.0));
    let mut row = 0;
    let mut yy = y;
    while yy < y + h {
        let mut col = 0;
        let mut xx = x;
        while xx < x + w {
            if let Some(r) = Rect::from_xywh(xx, yy, SQUARE.min(x + w - xx), SQUARE.min(y + h - yy)) {
                pixmap.fill_rect(
                    r,
                    if (row + col) % 2 == 0 { &light } else { &dark },
                    Transform::identity(),
                    None,
                );
            }
            xx += SQUARE;
            col += 1;
        }
        yy += SQUARE;
        row += 1;
    }
}

fn save(pixmap: &Pixmap, path: &Path) -> Result<()> {
    let image = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), straight(pixmap))
        .ok_or_else(|| anyhow::anyhow!("bad picture"))?;
    image
        .save(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The scene alone at `t` seconds as a PNG of `size`, see-through where
/// nothing is drawn.
pub fn still_png(source: &str, size: (u32, u32), t: f64, path: &Path) -> Result<()> {
    let spec = Spec::parse(source)?;
    let mut painter = Painter::new(spec, size.0.max(2), size.1.max(2));
    save(&painter.frame(t), path)
}

/// The scene alone at `t` seconds, over a checkerboard where it is clear.
pub fn still(settings: &Settings, source: &str, t: f64, path: &Path) -> Result<()> {
    let spec = Spec::parse(source)?;
    let (w, h) = (settings.width.max(2), settings.height.max(2));
    let mut painter = Painter::new(spec, w, h);
    let frame = painter.frame(t);
    let mut out = Pixmap::new(w, h).expect("a canvas");
    checker(&mut out, 0.0, 0.0, w as f32, h as f32);
    out.draw_pixmap(
        0,
        0,
        frame.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    save(&out, path)
}

/// Frames across the scene in a grid, each with its time: motion at a glance,
/// for a reader that cannot watch it.
pub fn sheet(settings: &Settings, source: &str, seconds: f64, every: Option<f64>, path: &Path) -> Result<Vec<f64>> {
    let spec = Spec::parse(source)?;
    let (w, h) = (settings.width.max(2), settings.height.max(2));
    let every = every.unwrap_or(seconds / 11.0).max(1.0 / settings.fps.max(1) as f64);
    let mut times: Vec<f64> = (0..)
        .map(|i| ((i as f64 * every) * 100.0).round() / 100.0)
        .take_while(|t| *t < seconds - 1e-6)
        .take(48)
        .collect();
    // Always show the very end too.
    let last = ((seconds - 1.0 / settings.fps.max(1) as f64).max(0.0) * 100.0).floor() / 100.0;
    if times.last().is_none_or(|t| last - t > every / 3.0) {
        times.push(last);
    }
    const COLUMNS: usize = 4;
    const LABEL: f32 = 26.0;
    const GAP: f32 = 8.0;
    let cell_w = 360.0f32;
    let cell_h = cell_w * h as f32 / w as f32;
    let rows = times.len().div_ceil(COLUMNS);
    let (sw, sh) = (
        GAP + COLUMNS as f32 * (cell_w + GAP),
        GAP + rows as f32 * (cell_h + LABEL + GAP),
    );
    let mut out = Pixmap::new(sw.ceil() as u32, sh.ceil() as u32).expect("a sheet");
    out.fill(Color::from_rgba8(0x12, 0x15, 0x1f, 255));
    let mut painter = Painter::new(spec, w, h);
    let scale = cell_w / w as f32;
    for (i, t) in times.iter().enumerate() {
        let (col, row) = (i % COLUMNS, i / COLUMNS);
        let x = GAP + col as f32 * (cell_w + GAP);
        let y = GAP + row as f32 * (cell_h + LABEL + GAP);
        checker(&mut out, x, y, cell_w, cell_h);
        let frame = painter.frame(*t);
        let paint = PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        out.draw_pixmap(
            0,
            0,
            frame.as_ref(),
            &paint,
            Transform::from_translate(x, y).pre_scale(scale, scale),
            None,
        );
        let label = titles::text_picture(
            &format!("{t:.2}s"),
            "Menlo",
            15.0,
            false,
            false,
            [0xb8, 0xbe, 0xcc, 255],
            Align::Left,
            None,
        );
        if let Some(label) = to_pixmap(&label) {
            out.draw_pixmap(
                0,
                0,
                label.as_ref(),
                &PixmapPaint::default(),
                Transform::from_translate(x, y + cell_h + 2.0),
                None,
            );
        }
    }
    save(&out, path)?;
    Ok(times)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = pixmap.pixel(x, y).unwrap().demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn paints_shapes_where_they_go_and_leaves_the_rest_clear() {
        let spec = Spec::parse(
            r##"{ "layers": [
                { "type": "rect", "x": 100, "y": 100, "width": 80, "height": 40, "fill": "#ff0000" },
                { "type": "ellipse", "x": "75%", "y": "50%", "width": 60, "height": 60, "fill": "#0000ff", "enter": "fade" },
                { "type": "line", "x": 0, "y": 300, "from": [0, 0], "to": [400, 0], "stroke": "#00ff00", "strokeWidth": 8,
                  "draw": { "at": 0, "duration": 1, "ease": "linear" } }
            ] }"##,
        )
        .unwrap();
        let mut painter = Painter::new(spec, 400, 400);
        let start = painter.frame(0.0);
        assert_eq!(at(&start, 100, 100), [255, 0, 0, 255]);
        assert_eq!(at(&start, 5, 5)[3], 0, "empty is see-through");
        assert_eq!(at(&start, 300, 200)[3], 0, "not faded in yet");
        let half = painter.frame(0.5);
        assert!(at(&half, 300, 200)[3] > 200);
        assert_eq!(at(&half, 100, 300)[1], 255, "the first half of the line is drawn");
        assert_eq!(at(&half, 300, 300)[3], 0, "the second half is not yet");
    }

    #[test]
    fn draws_words_and_paths() {
        let spec = Spec::parse(
            r##"{ "background": "#000000", "layers": [
                { "type": "text", "x": 200, "y": 100, "text": "Hello", "size": 60, "bold": true, "fill": "#ffffff" },
                { "type": "path", "d": "M 20 300 C 120 200 280 400 380 300", "stroke": "#ffffff", "strokeWidth": 6 }
            ] }"##,
        )
        .unwrap();
        let mut painter = Painter::new(spec, 400, 400);
        let frame = painter.frame(0.0);
        let lit = |y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (0..400).map(move |x| (x, y)))
                .filter(|&(x, y)| at(&frame, x, y)[0] > 128)
                .count()
        };
        assert!(lit(70, 130) > 200, "the word is drawn");
        assert!(lit(250, 350) > 200, "the curve is drawn");
        assert_eq!(at(&frame, 5, 5), [0, 0, 0, 255], "the background fills");
    }

    #[test]
    fn measures_paths() {
        let mut p = PathBuilder::new();
        p.move_to(0.0, 0.0);
        p.line_to(30.0, 40.0);
        assert!((length(&p.finish().unwrap()) - 50.0).abs() < 1e-3);
        let circle = PathBuilder::from_oval(Rect::from_xywh(-50.0, -50.0, 100.0, 100.0).unwrap()).unwrap();
        assert!((length(&circle) - std::f32::consts::PI * 100.0).abs() < 1.0);
    }
}
