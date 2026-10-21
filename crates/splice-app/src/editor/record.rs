//! Recording the screen from inside Splice: choose a display, a window or a
//! region, record (Splice steps aside when it would be in the shot), stop from
//! the title bar, by clicking Splice in the Dock, or through `splice ctl`,
//! and the take comes in as media, laid into the chosen beat if there is one.

use super::render::icon;
use super::*;
use crate::recorder::{self, Options, Recording, Source};
use gpui::FontWeight;
use serde_json::{Value, json};
use splice_core::story::BeatId;
use std::sync::mpsc::Sender;

/// Where a take goes when it is done.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Into {
    /// Just into the media panel.
    Media,
    /// Laid into a beat.
    Beat(BeatId),
    /// On the timeline at a time, on a layer.
    At(Time, usize),
}

pub struct Active {
    pub recording: Recording,
    pub into: Into,
    /// Splice hid itself to stay out of the shot.
    pub hid: bool,
    /// `splice ctl record stop --wait` callers.
    pub waiters: Vec<Sender<String>>,
}

/// A choice in the record panel.
#[derive(Clone, Debug)]
pub struct Choice {
    pub label: String,
    pub detail: String,
    pub source: Source,
}

pub fn choices() -> Vec<Choice> {
    let mut out: Vec<Choice> = recorder::displays()
        .into_iter()
        .map(|d| Choice {
            label: if d.number == 1 {
                "Entire screen".to_string()
            } else {
                format!("Screen {}", d.number)
            },
            detail: format!("{} · {:.0}×{:.0}", d.name, d.width, d.height),
            source: Source::Display { number: d.number },
        })
        .collect();
    out.extend(recorder::windows().into_iter().map(|w| Choice {
        label: w.app.clone(),
        detail: if w.title.is_empty() {
            format!("window · {:.0}×{:.0}", w.width, w.height)
        } else {
            format!("{} · {:.0}×{:.0}", w.title, w.width, w.height)
        },
        source: Source::Window { id: w.id },
    }));
    out
}

impl Editor {
    pub(super) fn open_record(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return self.stop_recording(cx);
        }
        self.overlay = Overlay::Record {
            choices: choices(),
            index: 0,
            microphone: false,
            clicks: true,
        };
        window.focus(&self.palette_focus);
        cx.notify();
    }

    /// Where the next take goes: the chosen beat in Story view, else the media panel.
    fn take_target(&self) -> Into {
        match (self.mode, self.beat) {
            (story::Mode::Story, Some(beat)) => Into::Beat(beat),
            _ => Into::Media,
        }
    }

    pub(super) fn start_from_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Record {
            choices,
            index,
            microphone,
            clicks,
        } = &self.overlay
        else {
            return;
        };
        let Some(choice) = choices.get(*index).cloned() else {
            return;
        };
        let options = Options {
            microphone: *microphone,
            clicks: *clicks,
            seconds: None,
        };
        // A whole screen would show Splice: step aside.
        let hide = matches!(choice.source, Source::Display { .. });
        let into = self.take_target();
        self.close_overlay(window, cx);
        if let Err(e) = self.start_recording(choice.source, options, into, hide, cx) {
            self.say_error(e);
        }
    }

    pub(super) fn start_recording(
        &mut self,
        source: Source,
        options: Options,
        into: Into,
        hide: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.recording.is_some() {
            return Err("already recording; stop it first".into());
        }
        if hide {
            cx.hide();
        }
        let recording = recorder::start(source, &options).map_err(|e| e.to_string())?;
        self.recording = Some(Active {
            recording,
            into,
            hid: hide,
            waiters: Vec::new(),
        });
        self.say(if hide {
            "Recording. Click Splice in the Dock to stop."
        } else {
            "Recording"
        });
        // Tick the clock, notice a time limit, and stop when Splice comes back.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let going = this
                    .update(cx, |editor, cx| {
                        let Some(active) = &mut editor.recording else {
                            return false;
                        };
                        if active.recording.finished() {
                            editor.stop_recording(cx);
                            return false;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
        Ok(())
    }

    /// Stops recording; the take comes in when the file is finished.
    pub(super) fn stop_recording(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.recording.take() else { return };
        let Active {
            recording,
            into,
            hid,
            waiters,
        } = active;
        if hid {
            cx.activate(true);
        }
        let seconds = recording.seconds();
        self.say(format!("Finishing the recording ({seconds:.0}s)…"));
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { recording.stop() }).await;
            this.update(cx, |editor, cx| {
                match result {
                    Ok(path) => {
                        let place = match into {
                            Into::At(t, track) => Some((t, track)),
                            _ => None,
                        };
                        if let Into::Beat(beat) = into {
                            editor.fill_after_import.insert(path.clone(), beat);
                        }
                        // Answer `--wait` callers when the take is in.
                        let reply = waiters.into_iter().next();
                        editor.import_paths(vec![path], place, reply, cx);
                    }
                    Err(e) => {
                        editor.say_error(format!("Recording failed: {e}"));
                        for w in waiters {
                            let _ = w.send(crate::ctl::fail(e.to_string()).to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Splice came back to the front while it was hiding for a recording:
    /// that is the way to stop.
    pub(super) fn window_activated(&mut self, active: bool, cx: &mut Context<Self>) {
        if active
            && let Some(rec) = &self.recording
            && rec.hid
            && rec.recording.seconds() > 1.0
        {
            self.stop_recording(cx);
        }
    }

    /// The title bar's record control: a button, or the running clock.
    pub(super) fn record_button(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match &self.recording {
            Some(active) => {
                let s = active.recording.seconds() as u64;
                div()
                    .id("recording")
                    .h(px(26.0))
                    .px(px(10.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .cursor_pointer()
                    .bg(gpui::rgba(0xef6a5533))
                    .border_1()
                    .border_color(Theme::accent())
                    .child(div().size(px(8.0)).rounded_full().bg(Theme::accent()))
                    .child(
                        div()
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .text_color(Theme::text())
                            .child(format!("{}:{:02}", s / 60, s % 60)),
                    )
                    .child(div().text_size(px(11.5)).text_color(Theme::text()).child("Stop"))
                    .tooltip(render::tip("Stop recording", Some("⇧⌘R")))
                    .on_click(cx.listener(|this, _, _, cx| this.stop_recording(cx)))
                    .into_any_element()
            }
            None => render::icon_button("record", "record", true)
                .tooltip(render::tip("Record the screen", Some("⇧⌘R")))
                .on_click(cx.listener(|this, _, window, cx| this.open_record(window, cx)))
                .into_any_element(),
        }
    }

    pub(super) fn record_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let Overlay::Record {
            choices,
            index,
            microphone,
            clicks,
        } = &self.overlay
        else {
            return div().into_any_element();
        };
        let (index, microphone, clicks) = (*index, *microphone, *clicks);
        let into = match self.take_target() {
            Into::Beat(beat) => {
                let title = self
                    .doc
                    .project
                    .story
                    .beats
                    .iter()
                    .find(|b| b.id == beat)
                    .map(|b| b.title.clone())
                    .unwrap_or_default();
                format!("The take goes into the beat “{title}”.")
            }
            _ => "The take goes into the media panel.".to_string(),
        };
        let first = index.saturating_sub(3);
        let rows: Vec<AnyElement> = choices
            .iter()
            .enumerate()
            .skip(first)
            .take(7)
            .map(|(i, c)| {
                let active = i == index;
                let glyph = if matches!(c.source, Source::Display { .. }) {
                    "film"
                } else {
                    "sidebar-off"
                };
                div()
                    .id(("record-choice", i))
                    .h(px(40.0))
                    .px(px(12.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .cursor_pointer()
                    .when(active, |d| d.bg(Theme::surface0()))
                    .hover(|d| d.bg(Theme::surface0()))
                    .child(icon(glyph, if active { Theme::text() } else { Theme::overlay() }).size(px(14.0)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(if active { Theme::text() } else { Theme::subtext() })
                                    .truncate()
                                    .child(c.label.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(Theme::overlay())
                                    .truncate()
                                    .child(c.detail.clone()),
                            ),
                    )
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        if let Overlay::Record { index, .. } = &mut this.overlay {
                            *index = i;
                        }
                        if e.click_count() >= 2 {
                            this.start_from_overlay(window, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let toggle = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(6.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .border_1()
                .border_color(if on { Theme::surface2() } else { Theme::edge() })
                .when(on, |d| d.bg(Theme::surface0()))
                .child(
                    div()
                        .size(px(7.0))
                        .rounded_full()
                        .bg(if on { Theme::accent() } else { Theme::surface1() }),
                )
                .child(div().text_size(px(11.5)).text_color(Theme::text()).child(label))
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(Theme::rule())
                    .child(icon("record", Theme::accent()))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(Theme::text())
                            .child("Record"),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_size(px(11.0))
                            .text_color(Theme::overlay())
                            .child("what to capture"),
                    ),
            )
            .child(div().id("record-rows").p(px(6.0)).flex().flex_col().children(rows))
            .child(
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .border_t_1()
                    .border_color(Theme::rule())
                    .child(
                        toggle("record-mic", "Microphone", microphone).on_click(cx.listener(|this, _, _, cx| {
                            if let Overlay::Record { microphone, .. } = &mut this.overlay {
                                *microphone = !*microphone;
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        toggle("record-clicks", "Show clicks", clicks).on_click(cx.listener(|this, _, _, cx| {
                            if let Overlay::Record { clicks, .. } = &mut this.overlay {
                                *clicks = !*clicks;
                            }
                            cx.notify();
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("record-start")
                            .h(px(28.0))
                            .px(px(12.0))
                            .rounded(px(7.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .cursor_pointer()
                            .bg(Theme::accent())
                            .hover(|s| s.opacity(0.9))
                            .child(div().size(px(8.0)).rounded_full().bg(Theme::ground()))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Theme::ground())
                                    .child("Start"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.start_from_overlay(window, cx))),
                    ),
            )
            .child(
                div()
                    .px(px(16.0))
                    .pb(px(12.0))
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(Theme::overlay())
                    .child(format!(
                        "{into} Recording the whole screen hides Splice; click it in the Dock to stop. \
                         ↑↓ choose · ⏎ start · esc close"
                    )),
            )
            .into_any_element()
    }

    /// `splice ctl record …`.
    pub(super) fn ctl_record(
        &mut self,
        request: &Value,
        reply: Sender<String>,
        cx: &mut Context<Self>,
    ) -> Option<Value> {
        use crate::ctl::fail;
        let sub = request.get("sub").and_then(Value::as_str).unwrap_or("status");
        match sub {
            "sources" => Some(json!({
                "ok": true,
                "displays": recorder::displays(),
                "windows": recorder::windows(),
                "note": "Record one with `record start --display N`, `--window ID`, or `--region X,Y,W,H` (screen points).",
            })),
            "status" => Some(json!({
                "ok": true,
                "recording": self.recording.as_ref().map(|a| json!({
                    "seconds": a.recording.seconds(),
                    "source": a.recording.source,
                    "file": a.recording.path,
                })),
            })),
            "start" => {
                let source = if let Some(spec) = request.get("window") {
                    let id = match spec {
                        Value::Number(n) => n.as_u64().map(|n| n as u32),
                        Value::String(name) => {
                            let lower = name.to_lowercase();
                            recorder::windows()
                                .into_iter()
                                .find(|w| {
                                    w.app.to_lowercase().contains(&lower) || w.title.to_lowercase().contains(&lower)
                                })
                                .map(|w| w.id)
                        }
                        _ => None,
                    };
                    match id {
                        Some(id) => Source::Window { id },
                        None => return Some(fail("no such window; `splice ctl record sources` lists them")),
                    }
                } else if let Some(region) = request.get("region").and_then(Value::as_str) {
                    let n: Vec<i64> = region.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                    match n[..] {
                        [x, y, w, h] if w > 0 && h > 0 => Source::Region {
                            x: x as i32,
                            y: y as i32,
                            width: w as u32,
                            height: h as u32,
                        },
                        _ => return Some(fail("--region is X,Y,WIDTH,HEIGHT in screen points")),
                    }
                } else {
                    Source::Display {
                        number: request.get("display").and_then(Value::as_u64).unwrap_or(1) as u32,
                    }
                };
                let options = Options {
                    microphone: request.get("mic").and_then(Value::as_bool) == Some(true),
                    clicks: request.get("clicks").and_then(Value::as_bool) == Some(true),
                    seconds: request.get("for").and_then(Value::as_f64).map(|s| s.ceil() as u32),
                };
                let into = if let Some(beat) = request.get("beat").and_then(Value::as_u64) {
                    let id = BeatId(beat);
                    if self.doc.project.story.index(id).is_none() {
                        return Some(fail(format!("no beat {beat}; `splice ctl story read` lists them")));
                    }
                    Into::Beat(id)
                } else {
                    match self.time(request, "at") {
                        Ok(Some(t)) => Into::At(t, self.track(request).unwrap_or(0)),
                        Ok(None) => Into::Media,
                        Err(e) => return Some(fail(e)),
                    }
                };
                // `--hide` (shared with `track --hide`): Splice steps out of the shot.
                let hide = request.get("hidden").and_then(Value::as_bool) == Some(true);
                match self.start_recording(source, options, into, hide, cx) {
                    Ok(()) => {
                        let file = self.recording.as_ref().map(|a| a.recording.path.clone());
                        Some(json!({ "ok": true, "recording": true, "file": file }))
                    }
                    Err(e) => Some(fail(e)),
                }
            }
            "stop" => {
                let Some(active) = &mut self.recording else {
                    return Some(fail("not recording"));
                };
                let wait = request.get("wait").and_then(Value::as_bool) == Some(true);
                if wait {
                    active.waiters.push(reply);
                }
                self.stop_recording(cx);
                if wait {
                    None
                } else {
                    Some(json!({ "ok": true, "stopping": true }))
                }
            }
            other => Some(fail(format!(
                "unknown `record {other}`: use sources, start, stop or status"
            ))),
        }
    }
}
