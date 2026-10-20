//! The timeline's geometry, hit testing and painting. Interaction state lives
//! in the editor; this module turns positions into meaning and draws.
//!
//! Rows run top to bottom: an empty row for "drop here to make a new layer",
//! then the tracks from the top layer down to track 0.

use crate::library::PEAK_BUCKET;
use crate::theme::{MONO_FONT, Theme, UI_FONT};
use gpui::{
    App, Bounds, Corners, Font, Hsla, Pixels, Point, RenderImage, Rgba, SharedString, TextRun, Window, bounds, font,
    point, px, quad, size, transparent_black,
};
use splice_core::model::{ClipId, Generator, MediaId, MediaKind, Project};
use splice_core::time::{SECOND, Time, from_secs, ruler_label, secs};
use std::collections::HashMap;
use std::sync::Arc;

pub const GUTTER: f32 = 76.0;
pub const RULER: f32 = 26.0;
pub const ROW: f32 = 54.0;
pub const GAP: f32 = 6.0;
pub const TOP_PAD: f32 = 6.0;
/// Grab width of a clip's trim handles.
pub const EDGE: f32 = 7.0;
/// Snap distance.
pub const SNAP: f32 = 8.0;

#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Pixels per second.
    pub pps: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            pps: 60.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
        }
    }
}

impl View {
    pub fn x(&self, t: Time) -> f32 {
        GUTTER + secs(t) as f32 * self.pps - self.scroll_x
    }

    pub fn t(&self, x: f32) -> Time {
        from_secs(((x - GUTTER + self.scroll_x) / self.pps) as f64)
    }

    pub fn px_to_time(&self, dx: f32) -> Time {
        from_secs((dx / self.pps) as f64)
    }

    fn row_y(&self, row: usize) -> f32 {
        RULER + TOP_PAD + row as f32 * (ROW + GAP) - self.scroll_y
    }

    pub fn track_y(&self, track: usize, tracks: usize) -> f32 {
        self.row_y(tracks - track)
    }

    /// The track under `y`; `tracks` itself means the new-layer row.
    pub fn track_at(&self, y: f32, tracks: usize) -> usize {
        let row = ((y - RULER - TOP_PAD + self.scroll_y) / (ROW + GAP)).floor().max(0.0) as usize;
        tracks - row.min(tracks)
    }

    pub fn content_height(&self, tracks: usize) -> f32 {
        RULER + TOP_PAD + (tracks + 1) as f32 * (ROW + GAP)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GutterButton {
    Hide,
    Mute,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Ruler,
    Gutter(usize, Option<GutterButton>),
    Body(ClipId),
    Start(ClipId),
    End(ClipId),
    Empty,
}

/// What is under a point given relative to the timeline's top left.
pub fn hit(project: &Project, view: &View, p: Point<f32>) -> Hit {
    if p.y < RULER {
        return Hit::Ruler;
    }
    let n = project.tracks.len();
    let track = view.track_at(p.y, n);
    let y0 = view.track_y(track.min(n), n);
    let within_row = p.y >= y0 && p.y < y0 + ROW;
    if p.x < GUTTER {
        if track >= n || !within_row {
            return Hit::Empty;
        }
        let button = match ((p.y - y0) / (ROW / 2.0)) as i32 {
            0 => GutterButton::Hide,
            _ => GutterButton::Mute,
        };
        return Hit::Gutter(track, (p.x > 8.0).then_some(button));
    }
    if track >= n || !within_row {
        return Hit::Empty;
    }
    for clip in &project.tracks[track].clips {
        let (x0, x1) = (view.x(clip.start), view.x(clip.end()));
        if p.x < x0 || p.x >= x1 {
            continue;
        }
        let edge = EDGE.min((x1 - x0) / 3.0);
        return if p.x < x0 + edge {
            Hit::Start(clip.id)
        } else if p.x >= x1 - edge {
            Hit::End(clip.id)
        } else {
            Hit::Body(clip.id)
        };
    }
    Hit::Empty
}

/// The nearest snap target within reach of any of `candidates`, as a shift.
pub fn snap(view: &View, points: &[Time], candidates: &[Time]) -> Option<(Time, Time)> {
    let reach = view.px_to_time(SNAP);
    candidates
        .iter()
        .flat_map(|&c| points.iter().map(move |&p| (p - c, p)))
        .filter(|(shift, _)| shift.abs() <= reach)
        .min_by_key(|(shift, _)| shift.abs())
}

/// Per-media images and peaks the painter needs, cloned out of the library.
#[derive(Clone, Default)]
pub struct ClipArt {
    pub name: SharedString,
    pub kind: Option<MediaKind>,
    pub strip: Vec<(Time, Arc<RenderImage>)>,
    pub thumb: Option<Arc<RenderImage>>,
    pub peaks: Option<Arc<Vec<u8>>>,
    pub size: (u32, u32),
    pub offline: bool,
}

/// A media drag hovering over the timeline.
#[derive(Clone, Copy)]
pub struct Ghost {
    pub start: Time,
    pub track: usize,
    pub duration: Time,
}

pub struct Frame {
    pub project: Project,
    pub view: View,
    pub selection: Vec<ClipId>,
    pub position: Time,
    pub art: HashMap<MediaId, ClipArt>,
    pub ghost: Option<Ghost>,
    pub snap: Option<Time>,
}

fn hsla(c: Rgba) -> Hsla {
    c.into()
}

fn text(window: &mut Window, cx: &mut App, s: &str, at: Point<Pixels>, size_px: f32, color: Rgba, f: Font) {
    if s.is_empty() {
        return;
    }
    let run = TextRun {
        len: s.len(),
        font: f,
        color: hsla(color),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(SharedString::from(s.to_string()), px(size_px), &[run], None);
    let _ = line.paint(at, px(size_px * 1.3), window, cx);
}

fn rect(window: &mut Window, b: Bounds<Pixels>, fill: impl Into<Hsla>, radius: f32) {
    window.paint_quad(quad(
        b,
        Corners::all(px(radius)),
        fill.into(),
        px(0.0),
        transparent_black(),
        Default::default(),
    ));
}

fn outline(window: &mut Window, b: Bounds<Pixels>, color: impl Into<Hsla>, width: f32, radius: f32) {
    window.paint_quad(quad(
        b,
        Corners::all(px(radius)),
        transparent_black(),
        px(width),
        color.into(),
        Default::default(),
    ));
}

/// Picks a ruler step that leaves room for labels.
fn tick_step(pps: f32) -> Time {
    const STEPS: &[Time] = &[
        SECOND / 10,
        SECOND / 5,
        SECOND / 2,
        SECOND,
        2 * SECOND,
        5 * SECOND,
        10 * SECOND,
        15 * SECOND,
        30 * SECOND,
        60 * SECOND,
        120 * SECOND,
        300 * SECOND,
        600 * SECOND,
    ];
    STEPS
        .iter()
        .copied()
        .find(|&s| secs(s) as f32 * pps >= 80.0)
        .unwrap_or(600 * SECOND)
}

pub fn paint(b: Bounds<Pixels>, frame: Frame, window: &mut Window, cx: &mut App) {
    let o = b.origin;
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let at = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
    let area = |x: f32, y: f32, width: f32, height: f32| bounds(at(x, y), size(px(width), px(height)));
    let view = frame.view;
    let project = &frame.project;
    let n = project.tracks.len();
    let ui = font(UI_FONT);
    let mono = font(MONO_FONT);

    rect(window, b, Theme::panel(), 0.0);

    // Rows and clips, clipped to the right of the gutter and below the ruler.
    let body = area(GUTTER, RULER, (w - GUTTER).max(0.0), (h - RULER).max(0.0));
    window.with_content_mask(Some(gpui::ContentMask { bounds: body }), |window| {
        for track in 0..=n {
            let y = view.track_y(track, n);
            if y + ROW < RULER || y > h {
                continue;
            }
            if track < n {
                rect(window, area(GUTTER, y, w - GUTTER, ROW), Theme::lane(), 0.0);
            }
            if track == n && frame.ghost.is_some_and(|g| g.track >= n) {
                outline(
                    window,
                    area(GUTTER + 2.0, y + 2.0, w - GUTTER - 4.0, ROW - 4.0),
                    Theme::surface1(),
                    1.0,
                    4.0,
                );
            }
        }
        for (track_index, track) in project.tracks.iter().enumerate() {
            let y = view.track_y(track_index, n);
            if y + ROW < RULER || y > h {
                continue;
            }
            for clip in &track.clips {
                let (x0, x1) = (view.x(clip.start), view.x(clip.end()));
                if x1 < GUTTER || x0 > w {
                    continue;
                }
                let selected = frame.selection.contains(&clip.id);
                let cb = area(x0, y, (x1 - x0).max(1.0), ROW);
                // Titles in a warm amber; colours as themselves.
                let (art, fill) = match &clip.generator {
                    Some(generator) => {
                        let art = ClipArt {
                            name: generator.name().into(),
                            ..ClipArt::default()
                        };
                        let fill = match generator {
                            Generator::Text(_) => Theme::clip_text(),
                            Generator::Color { color } => gpui::rgba(*color),
                        };
                        (art, fill)
                    }
                    None => {
                        let art = frame.art.get(&clip.media).cloned().unwrap_or_default();
                        let fill = if art.kind == Some(MediaKind::Audio) {
                            Theme::clip_audio()
                        } else {
                            Theme::clip()
                        };
                        (art, fill)
                    }
                };
                rect(window, cb, fill, 5.0);
                window.with_content_mask(Some(gpui::ContentMask { bounds: cb }), |window| {
                    paint_clip_art(window, &view, clip, &art, x0, x1, y, w, &area);
                    if track.hidden || track.muted {
                        rect(window, cb, Theme::scrim(), 5.0);
                    }
                    // Name on a quiet backdrop.
                    let label: String = if art.offline {
                        format!("{} (offline)", art.name)
                    } else {
                        art.name.to_string()
                    };
                    let label_x = x0.max(GUTTER) + 6.0;
                    if x1 - label_x > 24.0 {
                        let chars = (((x1 - label_x - 8.0) / 6.2) as usize).max(1);
                        let shown: String = if label.chars().count() > chars {
                            label.chars().take(chars.saturating_sub(1)).chain(['…']).collect()
                        } else {
                            label
                        };
                        let tw = shown.chars().count() as f32 * 6.2 + 8.0;
                        rect(window, area(label_x - 3.0, y + 3.0, tw, 15.0), Theme::scrim(), 3.0);
                        text(
                            window,
                            cx,
                            &shown,
                            at(label_x, y + 4.0),
                            10.5,
                            Theme::text(),
                            ui.clone(),
                        );
                    }
                });
                if selected {
                    outline(window, cb, Theme::accent(), 2.0, 5.0);
                    // Trim handles.
                    rect(
                        window,
                        area(x0 + 1.0, y + ROW / 2.0 - 9.0, 3.0, 18.0),
                        Theme::accent(),
                        1.5,
                    );
                    rect(
                        window,
                        area(x1 - 4.0, y + ROW / 2.0 - 9.0, 3.0, 18.0),
                        Theme::accent(),
                        1.5,
                    );
                } else {
                    outline(window, cb, Theme::crust(), 1.0, 5.0);
                }
            }
        }
        if let Some(ghost) = frame.ghost {
            let gx = view.x(ghost.start);
            let gy = view.track_y(ghost.track.min(n), n);
            let gw = (secs(ghost.duration) as f32 * view.pps).max(6.0);
            rect(window, area(gx, gy, gw, ROW), Theme::accent_soft(), 5.0);
            outline(window, area(gx, gy, gw, ROW), Theme::accent(), 1.0, 5.0);
        }
        if let Some(t) = frame.snap {
            let x = view.x(t);
            rect(window, area(x - 0.5, RULER, 1.0, h - RULER), Theme::subtext(), 0.0);
        }
    });

    // Ruler.
    rect(window, area(0.0, 0.0, w, RULER), Theme::panel(), 0.0);
    rect(window, area(0.0, RULER - 1.0, w, 1.0), Theme::rule(), 0.0);
    let ruler = area(GUTTER, 0.0, (w - GUTTER).max(0.0), RULER);
    window.with_content_mask(Some(gpui::ContentMask { bounds: ruler }), |window| {
        let step = tick_step(view.pps);
        let minor = step / 5;
        let first = view.t(GUTTER).max(0) / minor * minor;
        let mut t = first;
        while view.x(t) < w {
            let x = view.x(t);
            if t % step == 0 {
                rect(window, area(x, RULER - 9.0, 1.0, 8.0), Theme::overlay(), 0.0);
                text(
                    window,
                    cx,
                    &ruler_label(t),
                    at(x + 4.0, 5.0),
                    10.0,
                    Theme::muted(),
                    mono.clone(),
                );
            } else {
                rect(window, area(x, RULER - 5.0, 1.0, 4.0), Theme::surface2(), 0.0);
            }
            t += minor;
        }
    });

    // Gutter; its controls are elements laid over it.
    rect(window, area(0.0, RULER, GUTTER, h - RULER), Theme::panel(), 0.0);
    rect(window, area(GUTTER - 1.0, 0.0, 1.0, h), Theme::rule(), 0.0);

    // Playhead.
    let x = view.x(frame.position);
    if x >= GUTTER - 1.0 && x <= w {
        rect(window, area(x - 0.75, 0.0, 1.5, h), Theme::accent(), 0.0);
        rect(window, area(x - 5.0, 0.0, 10.0, 10.0), Theme::accent(), 2.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_clip_art(
    window: &mut Window,
    view: &View,
    clip: &splice_core::model::Clip,
    art: &ClipArt,
    x0: f32,
    x1: f32,
    y: f32,
    w: f32,
    area: &dyn Fn(f32, f32, f32, f32) -> Bounds<Pixels>,
) {
    let visible_from = x0.max(GUTTER);
    let visible_to = x1.min(w);
    let has_wave = art.peaks.is_some();
    let picture_h = if has_wave && art.kind != Some(MediaKind::Audio) {
        ROW - 16.0
    } else {
        ROW
    };

    // Picture: a filmstrip for video, the image repeated for stills.
    if matches!(art.kind, Some(MediaKind::Video) | Some(MediaKind::Image)) {
        let (sw, sh) = art.size;
        let aspect = if sh > 0 { sw as f32 / sh as f32 } else { 16.0 / 9.0 };
        let tile_w = (picture_h * aspect).max(8.0);
        let first = ((visible_from - x0) / tile_w).floor().max(0.0) as i32;
        let mut i = first;
        loop {
            let tx = x0 + i as f32 * tile_w;
            if tx > visible_to {
                break;
            }
            let image = match art.kind {
                Some(MediaKind::Video) => {
                    let source = clip.source_in + view.px_to_time(tx - x0 + tile_w / 2.0);
                    let index = art.strip.partition_point(|(t, _)| *t <= source).saturating_sub(1);
                    art.strip
                        .get(index)
                        .map(|(_, img)| img.clone())
                        .or_else(|| art.thumb.clone())
                }
                _ => art.thumb.clone(),
            };
            if let Some(image) = image {
                let _ = window.paint_image(area(tx, y, tile_w, picture_h), Corners::default(), image, 0, false);
            }
            i += 1;
        }
    }

    // Sound: bars from the peaks.
    if let Some(peaks) = &art.peaks {
        let (top, height) = if art.kind == Some(MediaKind::Audio) {
            (y + 18.0, ROW - 22.0)
        } else {
            (y + ROW - 16.0, 16.0)
        };
        if art.kind != Some(MediaKind::Audio) {
            rect(window, area(x0, top, x1 - x0, height), Theme::clip_audio(), 0.0);
        }
        let mid = top + height / 2.0;
        let mut x = visible_from;
        while x < visible_to {
            let source = clip.source_in + view.px_to_time(x - x0);
            let span = view.px_to_time(2.0).max(PEAK_BUCKET);
            let from = (source / PEAK_BUCKET).max(0) as usize;
            let to = (((source + span) / PEAK_BUCKET) as usize)
                .max(from + 1)
                .min(peaks.len());
            let peak = peaks.get(from..to).and_then(|s| s.iter().max()).copied().unwrap_or(0) as f32 / 255.0;
            let bar = (peak * clip.volume.min(1.5) * height).clamp(1.0, height);
            rect(window, area(x, mid - bar / 2.0, 1.4, bar), Theme::wave(), 0.0);
            x += 2.0;
        }
    }
}
