//! The stage: every picture under the playhead, placed by its look exactly as
//! export places it, and moved or resized with the mouse.

use super::*;
use gpui::{Hsla, Stateful};
use splice_core::look::Pose;
use splice_core::model::Source;

/// Something held with the mouse until it is let go. Changes land in the
/// project live and become one undo step on release.
pub enum Grab {
    /// Dragging a picture across the stage.
    Move {
        clip: ClipId,
        from: Point<Pixels>,
        start: (f32, f32),
        stage: (f32, f32),
        before: Project,
    },
    /// Dragging a corner handle: the picture grows or shrinks about its centre.
    Scale {
        clip: ClipId,
        center: Point<Pixels>,
        from: f32,
        start: f32,
        before: Project,
    },
    /// Dragging an inspector slider.
    Slider { field: inspector::Field, before: Project },
    /// Dragging notes in a score's roll.
    Note(workbench::NoteGrab),
}

/// A picture's clip, the part of the stage it shows in (inside its mask),
/// and its centre, which it grows and shrinks about.
pub type Placed = (ClipId, Bounds<Pixels>, Point<Pixels>);

pub struct Layer {
    pub clip: ClipId,
    pub image: Arc<RenderImage>,
    /// Size at scale 1, in canvas pixels.
    pub natural: (f64, f64),
    pub pose: Pose,
    /// The part of the picture its mask lets through: left, top, right,
    /// bottom, as fractions of its frame.
    pub visible: [f32; 4],
}

/// What a mask lets through of a picture's frame.
fn visible_part(mask: Option<&splice_core::look::Mask>) -> [f32; 4] {
    match mask {
        Some(m) if !m.invert => {
            let (w, h) = (m.width / 2.0, m.height / 2.0);
            [
                (m.x - w).clamp(0.0, 1.0),
                (m.y - h).clamp(0.0, 1.0),
                (m.x + w).clamp(0.0, 1.0),
                (m.y + h).clamp(0.0, 1.0),
            ]
        }
        _ => [0.0, 0.0, 1.0, 1.0],
    }
}

/// The part of `rect` that `visible` names.
fn sub_rect(rect: Bounds<Pixels>, visible: [f32; 4]) -> Bounds<Pixels> {
    let (w, h) = (rect.size.width, rect.size.height);
    let origin = point(rect.left() + w * visible[0], rect.top() + h * visible[1]);
    let size = size(w * (visible[2] - visible[0]), h * (visible[3] - visible[1]));
    Bounds::new(origin, size)
}

/// Snaps to the centre lines within this fraction of the canvas.
const SNAP: f32 = 0.012;
/// How near a corner handle counts as holding it, in points.
const HANDLE: f32 = 9.0;

/// Where a picture sits on a stage drawn at `stage`.
pub fn layer_rect(stage: Bounds<Pixels>, canvas: (u32, u32), natural: (f64, f64), pose: &Pose) -> Bounds<Pixels> {
    let (sw, sh) = (f32::from(stage.size.width), f32::from(stage.size.height));
    let k = sw / canvas.0.max(1) as f32;
    let size = size(
        px((natural.0 * pose.scale) as f32 * k),
        px((natural.1 * pose.scale) as f32 * k),
    );
    let center = point(
        stage.center().x + px(pose.x as f32 * sw),
        stage.center().y + px(pose.y as f32 * sh),
    );
    Bounds::centered_at(center, size)
}

fn corners(rect: Bounds<Pixels>) -> [Point<Pixels>; 4] {
    [
        rect.origin,
        point(rect.right(), rect.top()),
        point(rect.left(), rect.bottom()),
        point(rect.right(), rect.bottom()),
    ]
}

impl Editor {
    /// The pictures under the playhead, bottom first.
    pub(super) fn layers(&mut self) -> Vec<Layer> {
        let project = &self.doc.project;
        let canvas = (project.settings.width, project.settings.height);
        let t = self.player.position;
        let mut layers = Vec::new();
        for (clip, source) in project.visual_stack(t) {
            let pose = clip.look.pose(clip.duration, t - clip.start);
            let (image, rendered) = match source {
                Source::Media(media) => {
                    let filters = clip.look.pixel_filters();
                    let Some(image) = self.player.frame(clip.id, media, filters.as_deref(), &self.library) else {
                        continue;
                    };
                    (image.clone(), None)
                }
                Source::Generated(generator) => {
                    let Some((image, size)) = self.drawings.get(generator, clip.look.mask.as_ref(), canvas) else {
                        continue;
                    };
                    (image, Some(size))
                }
            };
            layers.push(Layer {
                clip: clip.id,
                image,
                natural: export::natural_size(source, rendered, canvas.0, canvas.1),
                pose,
                visible: visible_part(clip.look.mask.as_ref()),
            });
        }
        layers
    }

    /// The black canvas, its pictures, and the selected one's handles.
    pub(super) fn render_stage(&mut self, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let settings = self.doc.project.settings;
        let canvas_size = (settings.width, settings.height);
        let layers = self.layers();
        for image in self.drawings.sweep() {
            self.player.retire(vec![image]);
        }
        let stage_cell = self.stage_bounds.clone();
        let placed = self.stage_layers.clone();
        placed.borrow_mut().clear();

        let ground = canvas(
            move |bounds, _, _| {
                // The largest rectangle with the canvas's shape, with room to breathe.
                let (bw, bh) = (
                    f32::from(bounds.size.width) - 48.0,
                    f32::from(bounds.size.height) - 36.0,
                );
                let aspect = settings.width as f32 / settings.height.max(1) as f32;
                let (w, h) = if bw / bh.max(1.0) > aspect {
                    (bh * aspect, bh)
                } else {
                    (bw, bw / aspect)
                };
                let stage = Bounds::centered_at(bounds.center(), size(px(w.max(1.0)), px(h.max(1.0))));
                stage_cell.set(stage);
                stage
            },
            move |_, stage, window, _| {
                window.paint_shadows(stage, Corners::all(px(3.0)), &render::shadow(0.55, 28.0, 10.0));
                window.paint_quad(quad(
                    stage,
                    Corners::all(px(3.0)),
                    gpui::black(),
                    px(0.0),
                    transparent_black(),
                    Default::default(),
                ));
            },
        )
        .absolute()
        .inset_0();

        // One element per picture, so each carries its own opacity.
        let pictures = layers.into_iter().map(|layer| {
            let cell = self.stage_bounds.clone();
            let placed = placed.clone();
            let opacity = layer.pose.opacity.clamp(0.0, 1.0) as f32;
            div().absolute().inset_0().opacity(opacity).child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let stage = cell.get();
                        let rect = layer_rect(stage, canvas_size, layer.natural, &layer.pose);
                        placed
                            .borrow_mut()
                            .push((layer.clip, sub_rect(rect, layer.visible), rect.center()));
                        window.with_content_mask(Some(gpui::ContentMask { bounds: stage }), |window| {
                            let _ = window.paint_image(rect, Corners::default(), layer.image, 0, false);
                        });
                    },
                )
                .size_full(),
            )
        });

        // The selected picture's outline and corner handles, and snap guides.
        let selected = (self.selection.len() == 1).then(|| self.selection[0]);
        let guides = match self.grab {
            Some(Grab::Move { .. }) => self.guides,
            _ => (false, false),
        };
        let cell = self.stage_bounds.clone();
        let outline_from = placed.clone();
        let overlay = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let stage = cell.get();
                let accent: Hsla = Theme::accent().into();
                if guides.0 {
                    let x = stage.center().x;
                    window.paint_quad(gpui::fill(
                        Bounds::new(point(x - px(0.5), stage.top()), size(px(1.0), stage.size.height)),
                        accent,
                    ));
                }
                if guides.1 {
                    let y = stage.center().y;
                    window.paint_quad(gpui::fill(
                        Bounds::new(point(stage.left(), y - px(0.5)), size(stage.size.width, px(1.0))),
                        accent,
                    ));
                }
                let Some(rect) = selected.and_then(|id| {
                    outline_from
                        .borrow()
                        .iter()
                        .rev()
                        .find(|(c, _, _)| *c == id)
                        .map(|(_, r, _)| *r)
                }) else {
                    return;
                };
                window.paint_quad(quad(
                    rect,
                    Corners::default(),
                    transparent_black(),
                    px(1.5),
                    accent,
                    Default::default(),
                ));
                for corner in corners(rect) {
                    let handle = Bounds::centered_at(corner, size(px(9.0), px(9.0)));
                    window.paint_quad(quad(
                        handle,
                        Corners::all(px(2.0)),
                        Hsla::from(Theme::text()),
                        px(1.5),
                        accent,
                        Default::default(),
                    ));
                }
            },
        )
        .absolute()
        .inset_0();

        div()
            .id("stage")
            .flex_1()
            .min_h_0()
            .relative()
            .child(ground)
            .children(pictures)
            .child(overlay)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::stage_down))
    }

    fn stage_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let p = event.position;
        let placed = self.stage_layers.borrow().clone();
        let stage = self.stage_bounds.get();
        let (sw, sh) = (f32::from(stage.size.width), f32::from(stage.size.height));

        // A corner of the selected picture: resize.
        if let [selected] = self.selection[..]
            && let Some((_, rect, center)) = placed.iter().rev().find(|(c, _, _)| *c == selected)
        {
            let near = corners(*rect)
                .into_iter()
                .any(|c| (f32::from(c.x - p.x)).abs() <= HANDLE && (f32::from(c.y - p.y)).abs() <= HANDLE);
            if near && let Some(clip) = self.doc.project.clip(selected) {
                let center = *center;
                let from = f32::from((p.x - center.x).abs().max((p.y - center.y).abs())).max(1.0);
                self.grab = Some(Grab::Scale {
                    clip: selected,
                    center,
                    from,
                    start: clip.look.scale,
                    before: self.doc.project.clone(),
                });
                return cx.notify();
            }
        }

        // The topmost picture under the pointer: select it and move it.
        match placed.iter().rev().find(|(_, rect, _)| rect.contains(&p)) {
            Some((id, _, _)) => {
                let id = *id;
                if event.modifiers.shift || event.modifiers.platform {
                    if !self.selection.contains(&id) {
                        self.selection.push(id);
                    }
                } else {
                    self.selection = vec![id];
                }
                let Some(clip) = self.doc.project.clip(id) else { return };
                if event.click_count >= 2 && clip.generator.as_ref().is_some_and(|g| matches!(g, Generator::Text(_))) {
                    self.begin_title_edit(id, window, cx);
                    return cx.notify();
                }
                self.grab = Some(Grab::Move {
                    clip: id,
                    from: p,
                    start: (clip.look.x, clip.look.y),
                    stage: (sw.max(1.0), sh.max(1.0)),
                    before: self.doc.project.clone(),
                });
            }
            None => self.selection.clear(),
        }
        cx.notify();
    }

    /// Follows the pointer while something is held.
    pub(super) fn grab_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        match &self.grab {
            Some(Grab::Move {
                clip,
                from,
                start,
                stage,
                ..
            }) => {
                let mut x = start.0 + f32::from(at.x - from.x) / stage.0;
                let mut y = start.1 + f32::from(at.y - from.y) / stage.1;
                self.guides = (x.abs() < SNAP, y.abs() < SNAP);
                if self.guides.0 {
                    x = 0.0;
                }
                if self.guides.1 {
                    y = 0.0;
                }
                let clip = *clip;
                self.doc.project.set_look(clip, |look| {
                    look.x = x;
                    look.y = y;
                });
            }
            Some(Grab::Scale {
                clip,
                center,
                from,
                start,
                ..
            }) => {
                let reach = f32::from((at.x - center.x).abs().max((at.y - center.y).abs()));
                let scale = (start * reach / from).clamp(0.05, 8.0);
                let clip = *clip;
                self.doc.project.set_look(clip, |look| look.scale = scale);
            }
            Some(Grab::Slider { field, .. }) => {
                let field = *field;
                self.slide(field, at, cx);
            }
            Some(Grab::Note(_)) => return self.note_drag(at, cx),
            None => return,
        }
        cx.notify();
    }

    /// Lets go: what changed becomes one undo step.
    pub(super) fn grab_end(&mut self, cx: &mut Context<Self>) {
        let before = match self.grab.take() {
            Some(Grab::Move { before, .. } | Grab::Scale { before, .. } | Grab::Slider { before, .. }) => before,
            Some(Grab::Note(grab)) => {
                self.note_drop(grab.before, cx);
                return cx.notify();
            }
            None => return,
        };
        self.guides = (false, false);
        self.doc.commit(before);
        self.after_edit();
        cx.notify();
    }
}
