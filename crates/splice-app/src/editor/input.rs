//! `splice ctl click / hover / drag / scroll / key / type / ui`: operating the
//! window as a person would, for assistants and tests. Mouse events go to the
//! window as AppKit's own would (`pointer`), keys through gpui's keystroke
//! dispatch; neither leaves the process, so macOS's guard on synthetic input
//! does not apply. Steps land a frame apart, so hovers, drags and drops see
//! the window redraw between them. Positions are window points: a Retina
//! screenshot's pixels halved.

use super::*;
use crate::ctl::fail;
use crate::pointer::{Button, Event, Keys, Target};
use gpui::{Keystroke, Modifiers};
use serde_json::{Value, json};
use std::sync::mpsc::Sender;

/// Between steps: long enough for a frame to land.
const STEP: Duration = Duration::from_millis(24);

fn keys(request: &Value) -> Keys {
    let on = |k: &str| request.get(k).and_then(Value::as_bool) == Some(true);
    Keys {
        shift: on("shift"),
        command: on("cmd"),
        alt: on("alt"),
        control: on("ctrl"),
    }
}

fn point_at(request: &Value, x: &str, y: &str) -> Option<(f64, f64)> {
    Some((request.get(x)?.as_f64()?, request.get(y)?.as_f64()?))
}

/// A typed character as the key that makes it.
fn keystroke_for(c: char) -> Keystroke {
    let (key, typed) = match c {
        '\n' => ("enter".to_string(), None),
        '\t' => ("tab".to_string(), None),
        ' ' => ("space".to_string(), Some(" ".to_string())),
        c => (c.to_lowercase().to_string(), Some(c.to_string())),
    };
    Keystroke {
        modifiers: Modifiers {
            shift: c.is_uppercase(),
            ..Modifiers::default()
        },
        key,
        key_char: typed,
    }
}

impl Editor {
    pub(super) fn ctl_input(
        &mut self,
        cmd: &str,
        request: &Value,
        reply: Sender<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Value> {
        let held = keys(request);
        let button = if request.get("right").and_then(Value::as_bool) == Some(true) {
            Button::Right
        } else {
            Button::Left
        };
        let mut steps: Vec<Event> = Vec::new();
        match cmd {
            "ui" => {
                let size = window.viewport_size();
                let stage = self.stage_bounds.get();
                let timeline = self.timeline_bounds.get();
                let rect = |b: Bounds<Pixels>| {
                    json!({ "x": f32::from(b.origin.x), "y": f32::from(b.origin.y),
                            "width": f32::from(b.size.width), "height": f32::from(b.size.height) })
                };
                let layers: Vec<Value> = self
                    .stage_layers
                    .borrow()
                    .iter()
                    .map(|(clip, b, _)| json!({ "clip": clip.0, "bounds": rect(*b) }))
                    .collect();
                let mouse = window.mouse_position();
                return Some(json!({
                    "ok": true,
                    "window": { "width": f32::from(size.width), "height": f32::from(size.height),
                                "scale": window.scale_factor() },
                    "mouse": { "x": f32::from(mouse.x), "y": f32::from(mouse.y) },
                    "stage": rect(stage),
                    "timeline": rect(timeline),
                    "pictures": layers,
                    "overlay": !matches!(self.overlay, Overlay::None),
                    "editor": self.workbench.as_ref().map(|b| b.media.0),
                    "record": (self.mode == story::Mode::Record).then(|| {
                        let (scene, bubble) = self.rec.drawn;
                        json!({ "scene": scene.map(rect), "bubble": bubble.map(rect) })
                    }),
                    "note": "Positions are window points: a screenshot's pixels divided by `scale`.",
                }));
            }
            "click" | "hover" => {
                let Some((x, y)) = point_at(request, "x", "y") else {
                    return Some(fail(format!("{cmd} expects X Y in window points")));
                };
                steps.push(Event::Move { x, y, held: None });
                if cmd == "click" {
                    let count = request.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 3) as isize;
                    for n in 1..=count {
                        steps.push(Event::Down { x, y, button, count: n });
                        steps.push(Event::Up { x, y, button, count: n });
                    }
                }
            }
            "drag" => {
                let (Some((x, y)), Some((x2, y2))) = (point_at(request, "x", "y"), point_at(request, "x2", "y2"))
                else {
                    return Some(fail("drag expects X1 Y1 X2 Y2 in window points"));
                };
                steps.push(Event::Move { x, y, held: None });
                steps.push(Event::Down { x, y, button, count: 1 });
                // A smooth path, so drag thresholds and hover targets trip.
                const PATH: usize = 16;
                for i in 1..=PATH {
                    let t = i as f64 / PATH as f64;
                    steps.push(Event::Move {
                        x: x + (x2 - x) * t,
                        y: y + (y2 - y) * t,
                        held: Some(button),
                    });
                }
                steps.push(Event::Up {
                    x: x2,
                    y: y2,
                    button,
                    count: 1,
                });
            }
            "scroll" => {
                let Some((x, y)) = point_at(request, "x", "y") else {
                    return Some(fail("scroll expects X Y [--dy N] [--dx N]"));
                };
                let dx = request.get("dx").and_then(Value::as_f64).unwrap_or(0.0);
                let dy = request.get("dy").and_then(Value::as_f64).unwrap_or(0.0);
                steps.push(Event::Move { x, y, held: None });
                steps.push(Event::Scroll { x, y, dx, dy });
            }
            "key" => {
                let keys: Vec<Keystroke> = match request.get("keys").and_then(Value::as_array) {
                    Some(list) => {
                        let mut out = Vec::new();
                        for k in list.iter().filter_map(Value::as_str) {
                            match Keystroke::parse(k) {
                                Ok(stroke) => out.push(stroke),
                                Err(_) => {
                                    return Some(fail(format!("`{k}` is not a key, e.g. cmd-k, escape, shift-left")));
                                }
                            }
                        }
                        out
                    }
                    None => return Some(fail("key expects keys, e.g. `key cmd-t` or `key escape`")),
                };
                return self.send_keys(keys, reply, window, cx);
            }
            "type" => {
                let Some(text) = request.get("text").and_then(Value::as_str) else {
                    return Some(fail("type expects the text"));
                };
                let keys = text.chars().map(keystroke_for).collect();
                return self.send_keys(keys, reply, window, cx);
            }
            _ => return Some(fail(format!("unknown input `{cmd}`"))),
        }
        let Some(target) = Target::of(window) else {
            return Some(fail("the window cannot take events"));
        };
        let count = steps.len();
        cx.spawn_in(window, async move |_, cx| {
            for step in steps {
                // On the main thread, outside any gpui update: the window
                // takes it as AppKit's own.
                target.send(step, held);
                cx.background_executor().timer(STEP).await;
            }
            let _ = reply.send(json!({ "ok": true, "events": count }).to_string());
        })
        .detach();
        None
    }

    fn send_keys(
        &mut self,
        keys: Vec<Keystroke>,
        reply: Sender<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Value> {
        cx.spawn_in(window, async move |_, cx| {
            let mut handled = Vec::new();
            for key in keys {
                let name = key.to_string();
                match cx.update(|window, cx| window.dispatch_keystroke(key, cx)) {
                    Ok(done) => handled.push(json!({ "key": name, "handled": done })),
                    Err(_) => break,
                }
                cx.background_executor().timer(STEP).await;
            }
            let _ = reply.send(json!({ "ok": true, "keys": handled }).to_string());
        })
        .detach();
        None
    }
}
