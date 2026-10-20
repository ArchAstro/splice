//! How the editor looks. After Stet: the system title bar is hidden and the
//! traffic lights float over Splice's own ink bar; panels are quiet surfaces
//! separated by hairlines; vermilion marks only where you are.

use super::*;
use crate::icons;
use gpui::{AnyView, App, BoxShadow, FontWeight, Hsla, Rgba, Stateful, Svg, hsla, svg};

pub const TITLEBAR: f32 = 40.0;
/// The crust showing between panels; also the grab area of each divider.
const GAP: f32 = 8.0;
const STATUS: f32 = 26.0;
const BIN_MIN: f32 = 180.0;
const TIMELINE_MIN: f32 = 170.0;
/// The least the viewer keeps when the other panels grow.
const VIEWER_MIN: (f32, f32) = (360.0, 220.0);
const WORDMARK_FONT: &str = "Iowan Old Style";
/// Cards in the left panel: the narrowest they get, the space between them,
/// and their thumbnails' height for their width.
pub(super) const CARD_MIN: f32 = 72.0;
pub(super) const CARD_GAP: f32 = 10.0;
pub(super) const CARD_ASPECT: f32 = 9.0 / 16.0;
/// The panel's side padding.
pub(super) const SHELF_PAD: f32 = 14.0;

/// The width of a card, so whole cards fill a row of a panel this wide.
pub(super) fn card_width(panel: f32) -> f32 {
    // Less the panel's border and a little slack for rounding.
    let inner = (panel - 2.0 * SHELF_PAD - 4.0).max(CARD_MIN);
    let columns = ((inner + CARD_GAP) / (CARD_MIN + CARD_GAP)).floor().max(1.0);
    ((inner - CARD_GAP * (columns - 1.0)) / columns).floor()
}

/// A hover label: what a control does and its key.
pub struct Tip {
    text: SharedString,
    keys: Option<SharedString>,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .mt_1()
            .px_2()
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(Theme::surface0())
            .border_1()
            .border_color(Theme::surface1())
            .shadow(shadow(0.35, 10.0, 3.0))
            .font_family(UI_FONT)
            .text_size(px(11.0))
            .text_color(Theme::text())
            .flex()
            .items_center()
            .gap_2()
            .child(self.text.clone())
            .when_some(self.keys.clone(), |d, keys| {
                d.child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(10.0))
                        .text_color(Theme::muted())
                        .child(keys),
                )
            })
    }
}

pub(super) fn tip(
    text: &'static str,
    keys: Option<&'static str>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| {
        cx.new(|_| Tip {
            text: text.into(),
            keys: keys.map(Into::into),
        })
        .into()
    }
}

pub(super) fn shadow(alpha: f32, blur: f32, y: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0.0, 0.0, 0.0, alpha),
        offset: point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.0),
    }]
}

pub fn icon(name: &str, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(icons::path(name))
        .size(px(15.0))
        .flex_none()
        .text_color(color.into())
}

/// A square icon control that brightens on hover.
pub(super) fn icon_button(id: &'static str, name: &str, enabled: bool) -> Stateful<gpui::Div> {
    let group: SharedString = format!("{id}-group").into();
    let color: Rgba = if enabled { Theme::subtext() } else { Theme::surface2() };
    div()
        .id(id)
        .group(group.clone())
        .size(px(28.0))
        .flex_none()
        .rounded(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(Theme::surface0())))
        .child(icon(name, color).when(enabled, |i| i.group_hover(group, |s| s.text_color(Theme::text()))))
}

fn hairline() -> gpui::Div {
    div().w(px(1.0)).h(px(16.0)).mx_1().bg(Theme::surface0())
}

/// A raised surface on the crust.
pub(super) fn panel() -> gpui::Div {
    div()
        .rounded(px(10.0))
        .overflow_hidden()
        .bg(Theme::panel())
        .border_1()
        .border_color(Theme::edge())
}

pub(super) fn caps(text: &str) -> gpui::Div {
    div()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(Theme::overlay())
        .child(text.to_uppercase())
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Advance playback and keep frames coming while anything is in flight.
        let stage = self.stage_bounds.get();
        let scale = window.scale_factor();
        self.player.preview = (
            ((f32::from(stage.size.width) * scale) as u32).clamp(160, 1920),
            ((f32::from(stage.size.height) * scale) as u32).clamp(90, 1920),
        );
        let tick = self.player.tick(&self.doc.project, &self.library);
        if tick.started {
            self.start_sound();
        }
        if tick.ended {
            self.audio.stop();
        }
        if self.player.is_playing() {
            self.follow_playhead();
        }
        let timeline_size = self.timeline_bounds.get().size;
        if self.auto_fit && f32::from(timeline_size.width) > 0.0 && !self.player.is_playing() {
            let before = (self.view.pps, self.view.scroll_x);
            self.zoom_fit();
            if before != (self.view.pps, self.view.scroll_x) {
                window.request_animation_frame();
            }
        }
        // Few layers: center them in the timeline rather than leaving a gap below.
        // The empty new-layer row above them stays out of the reckoning.
        let n = self.doc.project.tracks.len() as f32;
        let content = self.view.content_height(self.doc.project.tracks.len());
        let room = f32::from(timeline_size.height);
        if room > 0.0 && content < room {
            let row = timeline::ROW + timeline::GAP;
            let natural_top = timeline::RULER + timeline::TOP_PAD + row;
            let centered_top = timeline::RULER + ((room - timeline::RULER - n * row) / 2.0).floor();
            self.view.scroll_y = (natural_top - centered_top).min(0.0);
        } else if !self.scrolled_layers {
            // More layers than fit: show the bottom ones, where the main
            // footage is, until the timeline is scrolled.
            self.view.scroll_y = (content - room).max(0.0);
        } else if self.view.scroll_y < 0.0 {
            self.view.scroll_y = 0.0;
        }
        if content < room {
            self.scrolled_layers = false;
        }
        for image in self.player.take_retired() {
            let _ = window.drop_image(image);
        }
        if self.player.busy() {
            window.request_animation_frame();
        }

        // Typing a title ends when the text field loses the keyboard.
        let choosing_font = matches!(self.overlay, Overlay::Fonts { .. });
        if self.title_edit.is_some() && !choosing_font && !self.text_focus.is_focused(window) {
            self.end_title_edit(cx);
        }

        let title = format!("{} — Splice", self.doc.name());
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }

        // Panel sizes: what was chosen, within what the window can give.
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let inspector_open = self.layout.inspector_open;
        let inspector = if inspector_open {
            inspector::INSPECTOR_WIDTH + GAP
        } else {
            0.0
        };
        let max_bin = (vw - VIEWER_MIN.0 - 3.0 * GAP - inspector).max(BIN_MIN);
        let max_timeline = (vh - TITLEBAR - STATUS - VIEWER_MIN.1 - GAP).max(TIMELINE_MIN);
        self.limits = (max_bin, max_timeline);
        let bin_width = self.layout.bin_width.clamp(BIN_MIN, max_bin);
        let timeline_height = self.layout.timeline_height.clamp(TIMELINE_MIN, max_timeline);
        let bin_open = self.layout.bin_open;
        let resizing = self.resizing;

        div()
            .id("splice")
            .key_context("Splice")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(Theme::crust())
            .text_color(Theme::text())
            .font_family(UI_FONT)
            .text_sm()
            .on_action(cx.listener(Self::play_pause))
            .on_action(cx.listener(Self::split))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::ripple_delete))
            .on_action(cx.listener(Self::duplicate))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::deselect))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::versions))
            .on_action(cx.listener(Self::cycle_aspect))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::ask_agent))
            .on_action(cx.listener(|this, _: &Export, _, cx| this.export(Quality::Standard, cx)))
            .on_action(cx.listener(|this, _: &ExportDraft, _, cx| this.export(Quality::Draft, cx)))
            .on_action(cx.listener(|this, _: &StepBack, _, cx| this.step(-this.doc.project.frame(), cx)))
            .on_action(cx.listener(|this, _: &StepForward, _, cx| this.step(this.doc.project.frame(), cx)))
            .on_action(cx.listener(|this, _: &JumpBack, _, cx| this.step(-SECOND, cx)))
            .on_action(cx.listener(|this, _: &JumpForward, _, cx| this.step(SECOND, cx)))
            .on_action(cx.listener(|this, _: &GoStart, _, cx| this.go_start(cx)))
            .on_action(cx.listener(|this, _: &GoEnd, _, cx| this.go_end(cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.zoom(1.4, None);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.zoom(1.0 / 1.4, None);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomFit, _, cx| {
                this.zoom_fit();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _, cx| this.toggle_track(true, cx)))
            .on_action(cx.listener(|this, _: &ToggleHide, _, cx| this.toggle_track(false, cx)))
            .on_action(cx.listener(|this, _: &RevealProject, _, cx| cx.reveal_path(this.doc.path())))
            .on_action(cx.listener(|this, _: &ToggleMedia, _, cx| this.toggle_media(cx)))
            .on_action(cx.listener(|this, _: &ResetLayout, _, cx| {
                this.layout = Layout::default();
                this.layout.store();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleInspector, _, cx| {
                this.layout.inspector_open = !this.layout.inspector_open;
                this.layout.store();
                cx.notify();
            }))
            .on_action(cx.listener(Self::add_text))
            .on_action(cx.listener(|this, _: &ChooseFont, window, cx| {
                if matches!(this.overlay, Overlay::Fonts { .. }) {
                    return this.close_overlay(window, cx);
                }
                match this.inspected().filter(|id| {
                    this.doc
                        .project
                        .clip(*id)
                        .is_some_and(|c| matches!(c.generator, Some(Generator::Text(_))))
                }) {
                    Some(id) => this.open_fonts(id, window, cx),
                    None => this.say("Select a title to choose its font"),
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewScore, window, cx| this.new_score(window, cx)))
            .on_action(cx.listener(|this, _: &NewScene, window, cx| this.new_scene(window, cx)))
            .on_action(cx.listener(|this, _: &ShowMedia, _, cx| this.show_shelf(Shelf::Media, cx)))
            .on_action(cx.listener(|this, _: &ShowTextStyles, _, cx| this.show_shelf(Shelf::Text, cx)))
            .on_action(cx.listener(|this, _: &ShowColours, _, cx| this.show_shelf(Shelf::Color, cx)))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, window, _| window.zoom_window())
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let place = this.ghost.take().map(|g| (g.start, g.track));
                this.import_paths(paths.paths().to_vec(), place, None, cx);
            }))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .id("workspace")
                    .key_context("Workspace")
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .px(px(GAP))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .when(bin_open, |d| d.child(self.render_bin(bin_width, cx)))
                            .child(self.divider(true, bin_open, bin_width, cx))
                            .map(|d| {
                                if self.workbench.is_some() {
                                    d.child(self.render_workbench(window, cx))
                                } else {
                                    d.child(self.render_viewer(cx)).when(inspector_open, |d| {
                                        d.child(div().w(px(GAP)).flex_none())
                                            .child(self.render_inspector(window, cx))
                                    })
                                }
                            }),
                    )
                    .child(self.divider(false, true, timeline_height, cx))
                    .child(
                        panel()
                            .h(px(timeline_height))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .child(self.render_toolbar(cx))
                            .child(self.render_timeline(cx)),
                    ),
            )
            .child(self.render_status(cx))
            .children(self.render_overlay(cx))
            // While a divider is held, a sheet over everything follows the pointer
            // and keeps the resize cursor, whatever it passes over.
            // The same while something is dragged on the stage or a slider.
            .when(resizing.is_some() || self.grab.is_some(), |d| {
                d.child(
                    div()
                        .id("resizing")
                        .occlude()
                        .absolute()
                        .inset_0()
                        .map(|d| match (resizing, &self.grab) {
                            (Some(Resize::Bin { .. }), _) => d.cursor_col_resize(),
                            (Some(Resize::Timeline { .. }), _) => d.cursor_row_resize(),
                            (_, Some(stage::Grab::Move { .. })) => d.cursor_grab(),
                            (_, Some(stage::Grab::Scale { .. })) => d.cursor(gpui::CursorStyle::ResizeUpRightDownLeft),
                            _ => d.cursor_pointer(),
                        })
                        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                            if event.pressed_button == Some(MouseButton::Left) {
                                this.resize_to(event.position, cx);
                                this.grab_to(event.position, cx);
                            } else {
                                this.end_resize(cx);
                                this.grab_end(cx);
                            }
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.end_resize(cx);
                                this.grab_end(cx);
                            }),
                        ),
                )
            })
    }
}

impl Editor {
    fn go_start(&mut self, cx: &mut Context<Self>) {
        self.seek(0, cx);
        self.follow_playhead();
    }

    fn go_end(&mut self, cx: &mut Context<Self>) {
        self.seek(self.doc.project.duration(), cx);
        self.follow_playhead();
    }

    fn show_shelf(&mut self, shelf: Shelf, cx: &mut Context<Self>) {
        self.shelf = shelf;
        if !self.layout.bin_open {
            self.layout.bin_open = true;
            self.layout.store();
        }
        cx.notify();
    }

    fn toggle_media(&mut self, cx: &mut Context<Self>) {
        self.layout.bin_open = !self.layout.bin_open;
        self.layout.store();
        cx.notify();
    }

    /// The strip of crust between two panels, which drags to resize them.
    /// `vertical` is the one beside the media panel; double-click resets it.
    fn divider(&mut self, vertical: bool, open: bool, current: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let id = if vertical { "bin-divider" } else { "timeline-divider" };
        let held = matches!(
            (self.resizing, vertical),
            (Some(Resize::Bin { .. }), true) | (Some(Resize::Timeline { .. }), false)
        );
        let group: SharedString = format!("{id}-group").into();
        div()
            .id(id)
            .group(group.clone())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .map(|d| {
                if vertical {
                    d.w(px(GAP)).h_full().cursor_col_resize()
                } else {
                    d.h(px(GAP)).w_full().cursor_row_resize()
                }
            })
            .child(
                div()
                    .rounded_full()
                    .map(|d| {
                        if vertical {
                            d.w(px(2.0)).h(px(40.0))
                        } else {
                            d.h(px(2.0)).w(px(40.0))
                        }
                    })
                    .bg(if held { Theme::accent() } else { Theme::crust() })
                    .when(!held, |d| d.group_hover(group, |s| s.bg(Theme::surface2()))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        let fresh = Layout::default();
                        if vertical {
                            this.layout.bin_width = fresh.bin_width;
                            this.layout.bin_open = true;
                        } else {
                            this.layout.timeline_height = fresh.timeline_height;
                        }
                        this.layout.store();
                    } else if vertical {
                        let width = if open { current } else { 0.0 };
                        this.resizing = Some(Resize::Bin {
                            from: f32::from(event.position.x),
                            width,
                        });
                    } else {
                        this.resizing = Some(Resize::Timeline {
                            from: f32::from(event.position.y),
                            height: current,
                        });
                    }
                    cx.notify();
                }),
            )
    }

    fn resize_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let (max_bin, max_timeline) = self.limits;
        match self.resizing {
            Some(Resize::Bin { from, width }) => {
                // Drag it most of the way closed and the media panel folds away.
                let w = width + f32::from(at.x) - from;
                self.layout.bin_open = w >= BIN_MIN * 0.6;
                if self.layout.bin_open {
                    self.layout.bin_width = w.clamp(BIN_MIN, max_bin);
                }
            }
            Some(Resize::Timeline { from, height }) => {
                self.layout.timeline_height = (height - (f32::from(at.y) - from)).clamp(TIMELINE_MIN, max_timeline);
            }
            None => return,
        }
        cx.notify();
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.layout.store();
            cx.notify();
        }
    }

    fn render_titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = if self.doc.edited { "autosaved" } else { "saved" };
        let export = self.export.as_ref().map(|job| job.lock().unwrap().progress);

        let export_button = match export {
            Some(progress) => div()
                .id("export")
                .occlude()
                .h(px(26.0))
                .w(px(132.0))
                .rounded(px(7.0))
                .relative()
                .overflow_hidden()
                .bg(Theme::surface0())
                .cursor_pointer()
                .tooltip(tip("Cancel the export", None))
                .on_click(cx.listener(|this, _, _, cx| this.cancel_export(cx)))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(relative(progress))
                        .bg(Theme::accent_soft()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.5))
                        .text_color(Theme::text())
                        .child(format!("Exporting {:.0}%", progress * 100.0)),
                ),
            None if self.doc.project.is_empty() => div()
                .id("export")
                .occlude()
                .h(px(26.0))
                .px(px(11.0))
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .bg(Theme::surface0())
                .text_color(Theme::overlay())
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .tooltip(tip("Nothing to export yet", None))
                .child(icon("export", Theme::overlay()).size(px(13.0)))
                .child("Export"),
            None => div()
                .id("export")
                .occlude()
                .h(px(26.0))
                .px(px(11.0))
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .bg(Theme::text())
                .text_color(Theme::ground())
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .hover(|s| s.bg(Theme::subtext()))
                .tooltip(tip("Export a movie", Some("⌘E")))
                .on_click(cx.listener(|this, _, _, cx| this.export(Quality::Standard, cx)))
                .child(icon("export", Theme::ground()).size(px(13.0)))
                .child("Export"),
        };

        div()
            .id("titlebar")
            .h(px(TITLEBAR))
            .flex_none()
            .relative()
            .flex()
            .items_center()
            .bg(Theme::crust())
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            // The name, centered on the window rather than the space left over.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(Theme::subtext())
                            .child(self.doc.name()),
                    )
                    .child(div().size(px(3.0)).rounded_full().bg(Theme::surface2()))
                    .child(div().text_size(px(11.5)).text_color(Theme::overlay()).child(state)),
            )
            .child(
                div()
                    .pl(px(84.0))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .font_family(WORDMARK_FONT)
                            .text_size(px(18.0))
                            .line_height(px(20.0))
                            .text_color(Theme::text())
                            .child("splice"),
                    )
                    .child(div().ml(px(2.0)).mt(px(2.0)).w(px(1.5)).h(px(15.0)).bg(Theme::accent()))
                    .child(
                        div().id("title-left").occlude().ml(px(14.0)).child(
                            icon_button(
                                "toggle-media",
                                if self.layout.bin_open { "sidebar" } else { "sidebar-off" },
                                true,
                            )
                            .tooltip(tip(
                                if self.layout.bin_open {
                                    "Hide media"
                                } else {
                                    "Show media"
                                },
                                Some("⌘1"),
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_media(cx))),
                        ),
                    ),
            )
            .child(
                div()
                    .id("title-controls")
                    .occlude()
                    .ml_auto()
                    .pr(px(10.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.render_presence(cx))
                    .child(
                        icon_button("versions", "history", true)
                            .tooltip(tip("Versions", Some("⇧⌘H")))
                            .on_click(cx.listener(|t, _, w, cx| t.versions(&Versions, w, cx))),
                    )
                    .child(
                        icon_button("commands", "command", true)
                            .tooltip(tip("Commands", Some("⌘/")))
                            .on_click(cx.listener(|t, _, w, cx| t.open_palette(&CommandMenu, w, cx))),
                    )
                    .child(div().w(px(6.0)))
                    .child(export_button),
            )
    }

    /// The assistant in the title bar: who is listening, or working, and a
    /// way to message them.
    fn render_presence(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let open = cx.listener(|this, _, window, cx| this.ask_agent(&AskAgent, window, cx));
        let Some(presence) = self.agent.presence() else {
            return icon_button("ask", "message", true)
                .tooltip(tip("Ask the assistant", Some("⇧⌘A")))
                .on_click(open)
                .into_any_element();
        };
        let (name, working) = match presence {
            agent::Presence::Listening(name) => (name, false),
            agent::Presence::Working(name) => (name, true),
        };
        div()
            .id("presence")
            .h(px(26.0))
            .pl(px(9.0))
            .pr(px(10.0))
            .mr_1()
            .rounded_full()
            .border_1()
            .border_color(Theme::edge())
            .bg(Theme::panel())
            .flex()
            .items_center()
            .gap(px(7.0))
            .cursor_pointer()
            .hover(|s| s.border_color(Theme::surface2()))
            .tooltip(tip(
                if working {
                    "Working on your last message"
                } else {
                    "Listening — send a message"
                },
                Some("⇧⌘A"),
            ))
            .on_click(open)
            .child(div().size(px(7.0)).rounded_full().map(|d| {
                if working {
                    d.border_1().border_color(Theme::subtext())
                } else {
                    d.bg(Theme::wave())
                }
            }))
            .child(div().text_size(px(11.5)).text_color(Theme::text()).child(name))
            .when(working, |d| {
                d.child(div().text_size(px(11.0)).text_color(Theme::overlay()).child("working"))
            })
            .into_any_element()
    }

    fn render_bin(&mut self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        self.card_width = card_width(width);
        let card = self.card_width;
        let items: Vec<AnyElement> = self
            .doc
            .project
            .media
            .iter()
            .map(|media| {
                let assets = self.library.get(&media.hash);
                let thumb = assets.and_then(|a| a.thumb.clone());
                let loading = assets.is_some_and(|a| a.loading);
                let offline = assets.is_some_and(|a| a.path.is_none() && !a.loading);
                let selected = self.bin_selection == Some(media.id);
                let id = media.id;
                let group: SharedString = format!("media-{}", id.0).into();
                let drag = MediaDrag {
                    media: id,
                    name: media.name.clone().into(),
                    duration: if media.is_timed() {
                        media.duration
                    } else {
                        splice_core::model::IMAGE_DURATION
                    },
                    generator: None,
                };
                let (kind_icon, badge) = match media.kind {
                    MediaKind::Image => ("image", None),
                    MediaKind::Audio if media.recipe.is_some() => ("score", Some(clock(media.duration))),
                    MediaKind::Audio => ("music", Some(clock(media.duration))),
                    MediaKind::Video if media.recipe.is_some() => ("scene", Some(clock(media.duration))),
                    MediaKind::Video => ("film", Some(clock(media.duration))),
                };
                div()
                    .id(("media", id.0 as usize))
                    .group(group.clone())
                    .w(px(card))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .cursor_grab()
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        window.focus(&this.focus);
                        this.bin_selection = Some(id);
                        if event.click_count() >= 2 && this.doc.project.media(id).is_some_and(|m| m.recipe.is_some()) {
                            return this.open_workbench(id, window, cx);
                        }
                        if event.click_count() >= 2 {
                            let at = this.player.position;
                            if let Some(clip) = this.edit(cx, |p| p.insert_clip(id, 0, at)) {
                                this.selection = vec![clip];
                            }
                        }
                        cx.notify();
                    }))
                    .on_drag(drag, |drag: &MediaDrag, _, _, cx| {
                        cx.new(|_| DragChip(drag.name.clone()))
                    })
                    .child(
                        div()
                            .w(px(card))
                            .h(px(card * CARD_ASPECT))
                            .rounded(px(7.0))
                            .overflow_hidden()
                            .bg(Theme::crust())
                            .border_1()
                            .border_color(if selected { Theme::accent() } else { Theme::surface0() })
                            .when(!selected, |d| {
                                d.group_hover(group.clone(), |s| s.border_color(Theme::surface2()))
                            })
                            .relative()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(thumb, |d, image| {
                                d.child(img(image).size_full().rounded(px(6.0)).object_fit(ObjectFit::Cover))
                            })
                            .when(media.kind == MediaKind::Audio, |d| {
                                let made = media.recipe.is_some();
                                d.bg(Theme::clip_audio())
                                    .child(icon(if made { "score" } else { "music" }, Theme::wave()).size(px(20.0)))
                            })
                            .when(loading, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .bg(Theme::scrim())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(Theme::subtext())
                                                .child("preparing…"),
                                        ),
                                )
                            })
                            .when(offline, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .bg(Theme::scrim())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(div().text_size(px(10.0)).text_color(Theme::error()).child("offline")),
                                )
                            })
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(4.0))
                                    .right(px(4.0))
                                    .h(px(15.0))
                                    .px(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(Theme::scrim())
                                    .flex()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(icon(kind_icon, Theme::subtext()).size(px(9.0)))
                                    .when_some(badge, |d, badge| {
                                        d.child(
                                            div()
                                                .font_family(MONO_FONT)
                                                .text_size(px(8.5))
                                                .text_color(Theme::subtext())
                                                .child(badge),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .px(px(1.0))
                            .text_size(px(10.5))
                            .w(px(card))
                            .text_color(if selected { Theme::text() } else { Theme::muted() })
                            .truncate()
                            .child(media.name.clone()),
                    )
                    .into_any_element()
            })
            .collect();
        let count = items.len();
        let mut items = items;
        let mut makes = self.make_cards(cx);
        if count > 0 {
            items.append(&mut makes);
        }
        let tabs = self.shelf_tabs(count, cx);
        let action = self.shelf_header_action(cx);
        let on_media = self.shelf == Shelf::Media;
        let shelf = (!on_media).then(|| self.render_shelf(cx));

        panel()
            .id("bin")
            .w(px(width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.0))
                    .pl(px(8.0))
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(tabs)
                    .children(action),
            )
            .children(shelf)
            .when(on_media, |d| {
                d.child(
                    div()
                        .id("bin-items")
                        .flex_1()
                        .overflow_y_scroll()
                        .px(px(SHELF_PAD))
                        .pb_3()
                        .when(count == 0, |d| {
                            d.child(
                                div()
                                    .id("bin-empty")
                                    .mt_1()
                                    .h(px(150.0))
                                    .rounded(px(10.0))
                                    .border_1()
                                    .border_dashed()
                                    .border_color(Theme::surface1())
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(Theme::lane()))
                                    .on_click(cx.listener(|this, _, window, cx| this.import(&Import, window, cx)))
                                    .child(
                                        div()
                                            .size(px(32.0))
                                            .rounded_full()
                                            .bg(Theme::surface0())
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(icon("plus", Theme::subtext())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(Theme::subtext())
                                            .child("Drop media here"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(Theme::overlay())
                                            .child("videos, images, sound · ⌘I"),
                                    ),
                            )
                        })
                        .when(count == 0, |d| {
                            d.child(div().mt(px(14.0)).flex().flex_wrap().gap(px(CARD_GAP)).children(makes))
                        })
                        .when(count > 0, |d| {
                            d.child(div().flex().flex_wrap().gap(px(CARD_GAP)).children(items))
                        }),
                )
            })
    }

    fn render_viewer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = &self.doc.project;
        let settings = project.settings;
        let empty = project.is_empty();
        let position = self.player.position;
        let duration = project.duration();
        let fps = settings.fps;
        let playing = self.player.is_playing();
        let aspect_label = ASPECTS
            .iter()
            .find(|(_, w, h)| {
                (*w as u64 * settings.height as u64).abs_diff(*h as u64 * settings.width as u64)
                    < (*h as u64 * settings.width as u64) / 100
            })
            .map_or_else(
                || format!("{}×{}", settings.width, settings.height),
                |(n, _, _)| n.to_string(),
            );

        // What to do first, given what there is to work with.
        let hint = if project.media.is_empty() {
            "Import with ⌘I, or drop files anywhere"
        } else if self.layout.bin_open {
            "Drag media onto the timeline, or double-click it"
        } else {
            "Show media (⌘1), then drag it onto the timeline"
        };
        let roomy = f32::from(self.stage_bounds.get().size.width) > 560.0;
        let stage = self.render_stage(cx);

        let transport_button = |id: &'static str, name: &'static str| icon_button(id, name, duration > 0);

        panel()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(stage.when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(icon("film", Theme::surface2()).size(px(28.0)))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(Theme::muted())
                                .child("Nothing on the timeline yet"),
                        )
                        .child(div().text_size(px(11.0)).text_color(Theme::overlay()).child(hint)),
                )
            }))
            .child(
                div()
                    .h(px(50.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .font_family(MONO_FONT)
                            .text_size(px(11.5))
                            .flex()
                            .gap(px(6.0))
                            .child(div().text_color(Theme::text()).child(timecode(position, fps)))
                            // The length too, when there is room for it.
                            .when(roomy, |d| {
                                d.child(div().text_color(Theme::surface2()).child("/"))
                                    .child(div().text_color(Theme::overlay()).child(timecode(duration, fps)))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_none()
                            .mx(px(12.0))
                            .child(
                                transport_button("go-start", "start")
                                    .tooltip(tip("Go to start", Some("Home")))
                                    .on_click(cx.listener(|this, _, _, cx| this.go_start(cx))),
                            )
                            .child(
                                div()
                                    .id("play")
                                    .size(px(36.0))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .map(|d| {
                                        if duration > 0 {
                                            d.cursor_pointer()
                                                .bg(Theme::text())
                                                .shadow(shadow(0.4, 12.0, 3.0))
                                                .hover(|s| s.bg(Theme::subtext()))
                                        } else {
                                            d.bg(Theme::surface0())
                                        }
                                    })
                                    .tooltip(tip(if playing { "Pause" } else { "Play" }, Some("Space")))
                                    .child(
                                        icon(
                                            if playing { "pause" } else { "play" },
                                            if duration > 0 {
                                                Theme::ground()
                                            } else {
                                                Theme::surface2()
                                            },
                                        )
                                        .size(px(16.0))
                                        .when(!playing, |i| i.ml(px(2.0))),
                                    )
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.play_pause(&PlayPause, window, cx)),
                                    ),
                            )
                            .child(
                                transport_button("go-end", "end")
                                    .tooltip(tip("Go to end", Some("End")))
                                    .on_click(cx.listener(|this, _, _, cx| this.go_end(cx))),
                            ),
                    )
                    .child(
                        div().flex_1().min_w_0().flex().justify_end().child(
                            div()
                                .id("aspect")
                                .h(px(24.0))
                                .px(px(9.0))
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(Theme::surface0())
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .cursor_pointer()
                                .hover(|s| s.border_color(Theme::surface2()))
                                .tooltip(tip("Canvas shape — click to change", None))
                                .child(
                                    div()
                                        .size(px(9.0))
                                        .rounded(px(2.0))
                                        .border_1()
                                        .border_color(Theme::muted()),
                                )
                                .child(
                                    div()
                                        .font_family(MONO_FONT)
                                        .text_size(px(10.5))
                                        .text_color(Theme::subtext())
                                        .child(aspect_label),
                                )
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.cycle_aspect(&CycleAspect, window, cx)),
                                ),
                        ),
                    ),
            )
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_selection = !self.selection.is_empty();
        let has_clips = !self.doc.project.is_empty();
        div()
            .h(px(40.0))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(Theme::edge())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        icon_button("undo", "undo", self.doc.can_undo())
                            .tooltip(tip("Undo", Some("⌘Z")))
                            .on_click(cx.listener(|t, _, w, cx| t.undo(&Undo, w, cx))),
                    )
                    .child(
                        icon_button("redo", "redo", self.doc.can_redo())
                            .tooltip(tip("Redo", Some("⇧⌘Z")))
                            .on_click(cx.listener(|t, _, w, cx| t.redo(&Redo, w, cx))),
                    )
                    .child(hairline())
                    .child(
                        icon_button("split", "split", has_clips)
                            .tooltip(tip("Split at playhead", Some("S")))
                            .on_click(cx.listener(|t, _, w, cx| t.split(&Split, w, cx))),
                    )
                    .child(
                        icon_button("delete", "trash", has_selection)
                            .tooltip(tip("Delete", Some("⌫")))
                            .on_click(cx.listener(|t, _, w, cx| t.delete(&Delete, w, cx))),
                    )
                    .child(
                        icon_button("ripple", "ripple", has_selection)
                            .tooltip(tip("Delete and close the gap", Some("⇧⌫")))
                            .on_click(cx.listener(|t, _, w, cx| t.ripple_delete(&RippleDelete, w, cx))),
                    )
                    .child(
                        icon_button("duplicate", "copy", has_selection)
                            .tooltip(tip("Duplicate", Some("⌘D")))
                            .on_click(cx.listener(|t, _, w, cx| t.duplicate(&Duplicate, w, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        icon_button("zoom-out", "zoom-out", true)
                            .tooltip(tip("Zoom out", Some("⌘-")))
                            .on_click(cx.listener(|t, _, _, cx| {
                                t.zoom(1.0 / 1.4, None);
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button("zoom-fit", "fit", true)
                            .tooltip(tip("Fit the whole movie", Some("⌘0")))
                            .on_click(cx.listener(|t, _, _, cx| {
                                t.zoom_fit();
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button("zoom-in", "zoom-in", true)
                            .tooltip(tip("Zoom in", Some("⌘=")))
                            .on_click(cx.listener(|t, _, _, cx| {
                                t.zoom(1.4, None);
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_timeline(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let art: HashMap<MediaId, ClipArt> = self
            .doc
            .project
            .media
            .iter()
            .map(|m| {
                let a = self.library.get(&m.hash);
                (
                    m.id,
                    ClipArt {
                        name: m.name.clone().into(),
                        kind: Some(m.kind),
                        strip: a.map(|a| a.strip.clone()).unwrap_or_default(),
                        thumb: a.and_then(|a| a.thumb.clone()),
                        peaks: a.and_then(|a| a.peaks.clone()),
                        size: (m.width, m.height),
                        offline: a.is_some_and(|a| a.path.is_none() && !a.loading),
                    },
                )
            })
            .collect();
        let frame = timeline::Frame {
            project: self.doc.project.clone(),
            view: self.view,
            selection: self.selection.clone(),
            position: self.player.position,
            art,
            ghost: self.ghost,
            snap: self.snap_line,
        };
        let cell = self.timeline_bounds.clone();
        let entity: Entity<Editor> = cx.entity();

        let surface = canvas(
            move |bounds, window, _| {
                cell.set(bounds);
                window.insert_hitbox(bounds, HitboxBehavior::Normal)
            },
            move |bounds, hitbox, window, cx| {
                timeline::paint(bounds, frame, window, cx);
                let e = entity.clone();
                let hb = hitbox.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble() && event.button == MouseButton::Left && hb.is_hovered(window) {
                        e.update(cx, |editor, cx| editor.timeline_down(event, window, cx));
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase.bubble() {
                        e.update(cx, |editor, cx| editor.timeline_move(event, cx));
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase.bubble() {
                        e.update(cx, |editor, cx| editor.timeline_up(event, cx));
                    }
                });
                let e = entity;
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase.bubble() && hitbox.is_hovered(window) {
                        e.update(cx, |editor, cx| editor.timeline_scroll(event, cx));
                    }
                });
            },
        )
        .size_full();

        // Per-track controls, laid over the painted gutter so they can hover and click.
        let n = self.doc.project.tracks.len();
        let view = self.view;
        let project = &self.doc.project;
        let controls = project.tracks.iter().enumerate().map(|(i, track)| {
            let y = view.track_y(i, n) - timeline::RULER;
            // Only the toggles this layer's clips can use; both while it is empty.
            let (mut picture, mut sound) = (track.clips.is_empty(), track.clips.is_empty());
            for clip in &track.clips {
                match project.source(clip) {
                    Some(splice_core::model::Source::Media(m)) => {
                        picture |= m.kind != MediaKind::Audio;
                        sound |= m.has_audio;
                    }
                    _ => picture = true,
                }
            }
            // Hidden or muted stays in reach, so it can be undone.
            picture |= track.hidden;
            sound |= track.muted;
            let spacer = || div().size(px(22.0)).flex_none();
            let toggle = |id: &'static str, on_icon: &'static str, off_icon: &'static str, off: bool| {
                let group: SharedString = format!("{id}-{i}").into();
                div()
                    .id((id, i))
                    .group(group.clone())
                    .size(px(22.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::surface0()))
                    .child(
                        icon(
                            if off { off_icon } else { on_icon },
                            if off { Theme::accent() } else { Theme::overlay() },
                        )
                        .size(px(13.0))
                        .when(!off, |icon| icon.group_hover(group, |s| s.text_color(Theme::text()))),
                    )
            };
            div()
                .absolute()
                .top(px(y))
                .left_0()
                .w(px(timeline::GUTTER - 1.0))
                .h(px(timeline::ROW))
                .pl(px(10.0))
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    div()
                        .w(px(14.0))
                        .font_family(MONO_FONT)
                        .text_size(px(10.0))
                        .text_color(Theme::surface2())
                        .child((i + 1).to_string()),
                )
                .when(!picture, |d| d.child(spacer()))
                .when(picture, |d| {
                    d.child(
                        toggle("hide", "eye", "eye-off", track.hidden)
                            .tooltip(tip(
                                if track.hidden {
                                    "Show this layer"
                                } else {
                                    "Hide this layer"
                                },
                                None,
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(cx, |p| p.tracks[i].hidden = !p.tracks[i].hidden);
                            })),
                    )
                })
                .when(!sound, |d| d.child(spacer()))
                .when(sound, |d| {
                    d.child(
                        toggle("mute", "volume", "volume-off", track.muted)
                            .tooltip(tip(
                                if track.muted {
                                    "Unmute this layer"
                                } else {
                                    "Mute this layer"
                                },
                                None,
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(cx, |p| p.tracks[i].muted = !p.tracks[i].muted);
                            })),
                    )
                })
        });

        div()
            .id("timeline")
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<MediaDrag>, _, cx| {
                let duration = event.drag(cx).duration;
                this.hover_drop(event.event.position, duration, cx);
            }))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                this.hover_drop(event.event.position, 3 * SECOND, cx);
            }))
            .on_drop(cx.listener(|this, drag: &MediaDrag, _, cx| {
                if let Some(ghost) = this.ghost.take()
                    && let Some(clip) = this.edit(cx, |p| match &drag.generator {
                        Some(generator) => {
                            Some(p.insert_generated(generator.clone(), ghost.track, ghost.start, drag.duration))
                        }
                        None => p.insert_clip(drag.media, ghost.track, ghost.start),
                    })
                {
                    this.selection = vec![clip];
                }
                cx.notify();
            }))
            .child(surface)
            .child(
                div()
                    .id("gutter")
                    .occlude()
                    .absolute()
                    .top(px(timeline::RULER))
                    .left_0()
                    .w(px(timeline::GUTTER - 1.0))
                    .bottom_0()
                    .overflow_hidden()
                    .children(controls),
            )
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.doc.project.settings;
        let message = self.status.as_ref().map(|s| (s.text.clone(), s.error));
        div()
            .h(px(STATUS))
            .flex_none()
            .px(px(14.0))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .bg(Theme::crust())
            .text_size(px(11.0))
            .text_color(Theme::overlay())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .when(self.working > 0, |d| {
                        d.child(div().size(px(5.0)).rounded_full().bg(Theme::muted()))
                            .child("Preparing media…")
                    })
                    .when_some(message, |d, (text, error)| {
                        d.child(
                            div()
                                .truncate()
                                .text_color(if error { Theme::error() } else { Theme::subtext() })
                                .child(text),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_none()
                    .child(
                        div()
                            .font_family(MONO_FONT)
                            .text_size(px(10.0))
                            .child(format!("{}×{} · {} fps", s.width, s.height, s.fps)),
                    )
                    .child(
                        div()
                            .id("menu")
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .cursor_pointer()
                            .hover(|d| d.text_color(Theme::text()))
                            .child("Commands")
                            .child(div().font_family(MONO_FONT).text_size(px(10.0)).child("⌘/"))
                            .on_click(cx.listener(|t, _, w, cx| t.open_palette(&CommandMenu, w, cx))),
                    ),
            )
    }

    fn render_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = || {
            div()
                .id("overlay-panel")
                .w(px(560.0))
                .max_h(px(460.0))
                .mt(px(TITLEBAR + 56.0))
                .flex()
                .flex_col()
                .rounded(px(12.0))
                .overflow_hidden()
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::surface1())
                .shadow(shadow(0.6, 40.0, 16.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        const ROWS_SHOWN: usize = 9;
        type Pick = fn(&mut Editor, usize, &mut Window, &mut Context<Editor>);
        let scroll = self.overlay_scroll.clone();
        let rows = |items: Vec<(String, String, bool, Option<&'static str>)>, cx: &mut Context<Self>, on_pick: Pick| {
            div()
                .id("overlay-rows")
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .px(px(6.0))
                .pb(px(6.0))
                .children(
                    items
                        .into_iter()
                        .enumerate()
                        .map(move |(i, (label, detail, active, glyph))| {
                            div()
                                .id(("row", i))
                                .h(px(32.0))
                                .px(px(10.0))
                                .rounded(px(7.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_4()
                                .cursor_pointer()
                                .when(active, |d| d.bg(Theme::surface0()))
                                .hover(|d| d.bg(Theme::surface0()))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .when_some(glyph, |d, glyph| {
                                            d.child(
                                                icon(glyph, if active { Theme::accent() } else { Theme::overlay() })
                                                    .size(px(13.0)),
                                            )
                                        })
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .text_color(if active { Theme::text() } else { Theme::subtext() })
                                                .child(label),
                                        ),
                                )
                                .when(!detail.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(Theme::ground())
                                            .font_family(MONO_FONT)
                                            .text_size(px(10.0))
                                            .text_color(Theme::muted())
                                            .child(detail),
                                    )
                                })
                                .on_click(cx.listener(move |this, _, window, cx| on_pick(this, i, window, cx)))
                        }),
                )
        };
        let footer = |hint: &'static str| {
            div()
                .h(px(30.0))
                .flex_none()
                .px(px(14.0))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(Theme::rule())
                .text_size(px(10.5))
                .text_color(Theme::overlay())
                .child(hint)
        };

        let content = match &self.overlay {
            Overlay::None => return None,
            Overlay::Palette { query, index } => {
                let matches = Self::palette_matches(query);
                let index = (*index).min(matches.len().saturating_sub(1));
                if let Overlay::Palette { index: stored, .. } = &mut self.overlay {
                    *stored = index;
                }
                let items: Vec<_> = matches
                    .iter()
                    .enumerate()
                    .map(|(i, (l, k, _))| (l.to_string(), k.to_string(), i == index, None))
                    .collect();
                let none = items.is_empty();
                let query = self.overlay_query();
                panel()
                    .child(
                        div()
                            .h(px(48.0))
                            .flex_none()
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .border_b_1()
                            .border_color(Theme::rule())
                            .child(icon("command", Theme::overlay()))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .text_size(px(14.0))
                                    .when(!query.is_empty(), |d| d.text_color(Theme::text()).child(query.clone()))
                                    .child(div().w(px(1.5)).h(px(17.0)).bg(Theme::accent()))
                                    .when(query.is_empty(), |d| {
                                        d.child(div().ml(px(2.0)).text_color(Theme::overlay()).child("Search commands"))
                                    }),
                            ),
                    )
                    .child(div().h(px(6.0)).flex_none())
                    .when(none, |d| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .py_3()
                                .text_size(px(12.0))
                                .text_color(Theme::overlay())
                                .child("No matching command"),
                        )
                    })
                    .child(rows(items, cx, |this, i, window, cx| {
                        let query = this.overlay_query();
                        let action = Self::palette_matches(&query).into_iter().nth(i).map(|(_, _, a)| a);
                        this.close_overlay(window, cx);
                        if let Some(action) = action {
                            window.dispatch_action(action, cx);
                        }
                    }))
                    .child(footer("↑↓ move  ·  ⏎ run  ·  esc close"))
            }
            Overlay::Fonts {
                clip,
                query,
                index,
                list,
            } => {
                let clip = *clip;
                let matches = Self::font_matches(list, query);
                let index = (*index).min(matches.len().saturating_sub(1));
                let current = match self.doc.project.clip(clip).and_then(|c| c.generator.as_ref()) {
                    Some(Generator::Text(t)) => t.font.clone(),
                    _ => String::new(),
                };
                let query = query.clone();
                let count = matches.len();
                // Only the rows near the choice: there can be hundreds of fonts.
                // As many as fit the panel with its search and footer.
                let first = index.saturating_sub(4);
                let rows: Vec<AnyElement> =
                    matches
                        .into_iter()
                        .enumerate()
                        .skip(first)
                        .take(ROWS_SHOWN)
                        .map(|(i, font)| {
                            let active = i == index;
                            let chosen = font == current;
                            let pick = font.clone();
                            div()
                                .id(("font", i))
                                .h(px(38.0))
                                .px(px(12.0))
                                .rounded(px(7.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .cursor_pointer()
                                .when(active, |d| d.bg(Theme::surface0()))
                                .hover(|d| d.bg(Theme::surface0()))
                                .child(
                                    div()
                                        .font_family(SharedString::from(font.clone()))
                                        .text_size(px(17.0))
                                        .text_color(if active { Theme::text() } else { Theme::subtext() })
                                        .child(font.clone()),
                                )
                                .when(chosen, |d| {
                                    d.child(div().size(px(6.0)).rounded_full().bg(Theme::accent()))
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.choose_font(clip, pick.clone(), window, cx)
                                }))
                                .into_any_element()
                        })
                        .collect();
                panel()
                    .child(
                        div()
                            .h(px(48.0))
                            .flex_none()
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .border_b_1()
                            .border_color(Theme::rule())
                            .child(icon("text", Theme::overlay()))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .text_size(px(14.0))
                                    .when(!query.is_empty(), |d| d.text_color(Theme::text()).child(query.clone()))
                                    .child(div().w(px(1.5)).h(px(17.0)).bg(Theme::accent()))
                                    .when(query.is_empty(), |d| {
                                        d.child(div().ml(px(2.0)).text_color(Theme::overlay()).child("Search fonts"))
                                    }),
                            )
                            .child(
                                div()
                                    .ml_auto()
                                    .text_size(px(11.0))
                                    .text_color(Theme::overlay())
                                    .child(format!("{count} on this Mac")),
                            ),
                    )
                    .child(div().id("font-rows").p(px(6.0)).flex().flex_col().children(rows))
                    .child(footer("↑↓ move  ·  ⏎ use  ·  esc close"))
            }
            Overlay::Instruments {
                media,
                part,
                query,
                index,
            } => {
                let (part, query) = (*part, query.clone());
                let current = match self.doc.project.media(*media).and_then(|m| m.recipe.as_ref()) {
                    Some(splice_core::model::Recipe::Score(s)) => s.parts.get(part).map(|p| {
                        if p.drums || splice_core::score::is_drums(&p.instrument) {
                            None
                        } else {
                            Some(
                                p.program
                                    .unwrap_or_else(|| splice_core::score::program_for(&p.instrument)),
                            )
                        }
                    }),
                    _ => None,
                };
                let matches = Self::instrument_matches(&query);
                let count = matches.len();
                let index = (*index).min(count.saturating_sub(1));
                let first = index.saturating_sub(4);
                let rows: Vec<AnyElement> =
                    matches
                        .into_iter()
                        .enumerate()
                        .skip(first)
                        .take(ROWS_SHOWN)
                        .map(|(i, (choice, name))| {
                            let active = i == index;
                            let chosen = current == Some(choice);
                            let family = match choice {
                                None => "drums".to_string(),
                                Some(p) => [
                                    "piano",
                                    "chromatic percussion",
                                    "organ",
                                    "guitar",
                                    "bass",
                                    "strings",
                                    "ensemble",
                                    "brass",
                                    "reed",
                                    "pipe",
                                    "synth lead",
                                    "synth pad",
                                    "synth effects",
                                    "world",
                                    "percussive",
                                    "sound effects",
                                ][p as usize / 8]
                                    .to_string(),
                            };
                            div()
                                .id(("instrument-row", i))
                                .h(px(34.0))
                                .px(px(12.0))
                                .rounded(px(7.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .cursor_pointer()
                                .when(active, |d| d.bg(Theme::surface0()))
                                .hover(|d| d.bg(Theme::surface0()))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(13.0))
                                        .text_color(if active { Theme::text() } else { Theme::subtext() })
                                        .child(name),
                                )
                                .child(div().text_size(px(10.5)).text_color(Theme::overlay()).child(family))
                                .when(chosen, |d| {
                                    d.child(div().size(px(6.0)).rounded_full().bg(Theme::accent()))
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.choose_instrument(part, choice, window, cx)
                                }))
                                .into_any_element()
                        })
                        .collect();
                panel()
                    .child(
                        div()
                            .h(px(48.0))
                            .flex_none()
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .border_b_1()
                            .border_color(Theme::rule())
                            .child(icon("score", Theme::overlay()))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .text_size(px(14.0))
                                    .when(!query.is_empty(), |d| d.text_color(Theme::text()).child(query.clone()))
                                    .child(div().w(px(1.5)).h(px(17.0)).bg(Theme::accent()))
                                    .when(query.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .ml(px(2.0))
                                                .text_color(Theme::overlay())
                                                .child("Search instruments"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .ml_auto()
                                    .text_size(px(11.0))
                                    .text_color(Theme::overlay())
                                    .child(format!("{count} General MIDI")),
                            ),
                    )
                    .child(div().id("instrument-rows").p(px(6.0)).flex().flex_col().children(rows))
                    .child(footer("↑↓ move  ·  ⏎ use  ·  esc close"))
            }
            Overlay::Ask { text } => {
                let presence = self.agent.presence();
                let name = match &presence {
                    Some(agent::Presence::Listening(n) | agent::Presence::Working(n)) => n.clone(),
                    None => "the assistant".into(),
                };
                let connected = presence.is_some();
                let state = match presence {
                    Some(agent::Presence::Listening(_)) => "listening".to_string(),
                    Some(agent::Presence::Working(_)) => "busy — this will wait its turn".to_string(),
                    None => "not connected".to_string(),
                };
                let fps = self.doc.project.settings.fps;
                let n = self.selection.len();
                let context = format!(
                    "Sends the playhead ({}){}",
                    timecode(self.player.position, fps),
                    match n {
                        0 => String::new(),
                        1 => " and the selected clip".into(),
                        n => format!(" and the {n} selected clips"),
                    }
                );
                let lines: Vec<String> = text.split('\n').map(String::from).collect();
                let last = lines.len() - 1;
                let empty = text.is_empty();
                panel()
                    .child(
                        div()
                            .h(px(48.0))
                            .flex_none()
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .border_b_1()
                            .border_color(Theme::rule())
                            .child(icon("message", Theme::overlay()))
                            .child(div().text_size(px(14.0)).text_color(Theme::text()).child(format!("Ask {name}")))
                            .child(div().text_size(px(11.5)).text_color(Theme::overlay()).child(state)),
                    )
                    .child(
                        div()
                            .min_h(px(84.0))
                            .px(px(16.0))
                            .py(px(14.0))
                            .flex()
                            .flex_col()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .child(div().w(px(1.5)).h(px(17.0)).bg(Theme::accent()))
                                        .child(
                                            div()
                                                .ml(px(2.0))
                                                .text_color(Theme::overlay())
                                                .child("Trim the pauses out of the interview, put the logo over the first five seconds…"),
                                        ),
                                )
                            })
                            .when(!empty, |d| {
                                d.children(lines.into_iter().enumerate().map(|(i, line)| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .min_h(px(20.0))
                                        .text_color(Theme::text())
                                        .child(line)
                                        .when(i == last, |d| d.child(div().w(px(1.5)).h(px(17.0)).bg(Theme::accent())))
                                }))
                            }),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .pb(px(12.0))
                            .text_size(px(11.0))
                            .text_color(Theme::overlay())
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(context)
                            .when(!connected, |d| d.child("An assistant connects by running splice ctl wait.")),
                    )
                    .child(footer("⏎ send  ·  ⇧⏎ new line  ·  esc close"))
            }
            Overlay::Versions { list, index } => {
                let now = SystemTime::now();
                let items: Vec<_> = list
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let (kind, glyph) = match v.kind {
                            VersionKind::Save => ("Saved version", "history"),
                            VersionKind::Auto => ("Autosave", "layers"),
                        };
                        let label = v.label.clone().map_or(kind.to_string(), |l| format!("{kind} · {l}"));
                        (label, ago(now, v.created), i == *index, Some(glyph))
                    })
                    .collect();
                panel()
                    .child(
                        div()
                            .h(px(48.0))
                            .flex_none()
                            .px(px(16.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .border_b_1()
                            .border_color(Theme::rule())
                            .child(icon("history", Theme::overlay()))
                            .child(div().text_size(px(14.0)).text_color(Theme::text()).child("Versions"))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(Theme::overlay())
                                    .child("kept inside this project file"),
                            ),
                    )
                    .child(div().h(px(6.0)).flex_none())
                    .child(rows(items, cx, |this, i, window, cx| {
                        if let Overlay::Versions { list, .. } = &this.overlay
                            && let Some(v) = list.get(i)
                        {
                            let id = v.id;
                            this.restore(id, window, cx);
                        }
                    }))
                    .child(footer("Restoring is an edit — ⌘Z brings back what you had"))
            }
        };

        Some(
            div()
                .id("overlay")
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .bg(Theme::scrim())
                .key_context("Overlay")
                .track_focus(&self.palette_focus)
                .on_key_down(cx.listener(Self::overlay_key))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_overlay(window, cx)),
                )
                .child(content)
                .into_any_element(),
        )
    }

    pub(super) fn overlay_query(&self) -> String {
        match &self.overlay {
            Overlay::Palette { query, .. } => query.clone(),
            _ => String::new(),
        }
    }
}

/// A length as a clock reads it: `0:08`, `1:05`.
fn clock(t: Time) -> String {
    let s = secs(t).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub(super) fn ago(now: SystemTime, then: SystemTime) -> String {
    let s = now.duration_since(then).unwrap_or_default().as_secs();
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}
