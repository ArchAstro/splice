//! How symbols look in the editor: a use's inspector (its placeholders first,
//! then its state against your symbols), marking placeholders in a symbol's
//! own window, the banner and see-through stage there, the shelf's cards, and
//! the sheets for naming symbols and managing them.

use super::inspector::{SWATCHES, Typing};
use super::render::{icon, tip};
use super::reuse::{LibraryOp, Naming, UseState};
use super::*;
use crate::symbols;
use gpui::{FontWeight, ObjectFit};
use splice_core::model::{Clip, Recipe};
use splice_core::symbol::{Fill, Symbol, Takes};

/// What the menu on a symbol's card offers.
pub const MENU: &[(&str, &str)] = &[
    ("Add at the playhead", "plus"),
    ("Edit symbol", "symbol"),
    ("Rename…", "text"),
    ("Duplicate…", "copy"),
    ("Show in Finder", "folder"),
    ("Move to Trash", "trash"),
];

/// A small text button.
fn link(id: impl Into<SharedString>, label: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .text_size(px(10.5))
        .text_color(Theme::overlay())
        .cursor_pointer()
        .hover(|s| s.text_color(Theme::text()))
        .child(label.into())
}

/// A button the width of its row.
fn wide(id: &'static str, label: impl Into<SharedString>, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(10.0))
        .flex_1()
        .rounded(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .cursor_pointer()
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .map(|d| {
            if primary {
                d.bg(Theme::text())
                    .text_color(Theme::ground())
                    .hover(|s| s.bg(Theme::subtext()))
            } else {
                d.border_1()
                    .border_color(Theme::edge())
                    .text_color(Theme::subtext())
                    .hover(|s| s.bg(Theme::lane()).text_color(Theme::text()))
            }
        })
        .child(label.into())
}

fn note(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(11.0))
        .line_height(px(16.0))
        .text_color(Theme::overlay())
        .child(text.into())
}

impl Editor {
    /// Whether the open project is a symbol in your symbols.
    pub(super) fn editing_a_symbol(&self) -> bool {
        self.doc.path().parent() == Some(symbols::folder().as_path())
    }

    /// A box of words that types in place: the caret shows while typing.
    /// `hint` shows, dim, while it is empty.
    fn words_box(
        &mut self,
        key: String,
        id: ClipId,
        what: Typing,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .title_edit
            .as_ref()
            .filter(|e| e.clip == id && e.what == what && self.text_focus.is_focused(window))
            .map(|e| (e.cursor.min(text.len()), e.all));
        let mut lines: Vec<AnyElement> = Vec::new();
        let mut offset = 0;
        for line in text.split('\n') {
            let end = offset + line.len();
            let row = div().min_h(px(18.0)).flex().items_center().flex_wrap();
            let row = match active {
                Some((cursor, false)) if cursor >= offset && cursor <= end => {
                    let (a, b) = line.split_at(cursor - offset);
                    row.child(a.to_string())
                        .child(div().w(px(1.5)).h(px(16.0)).bg(Theme::accent()))
                        .child(b.to_string())
                }
                _ => row.child(line.to_string()),
            };
            lines.push(row.into_any_element());
            offset = end + 1;
        }
        let begin = what.clone();
        let field = div()
            .id(SharedString::from(key))
            .min_h(px(32.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .bg(Theme::crust())
            .border_1()
            .border_color(if active.is_some() {
                Theme::surface2()
            } else {
                Theme::edge()
            })
            .text_size(px(13.0))
            .line_height(px(18.0))
            .text_color(Theme::text())
            .cursor_text()
            .when(active.is_some_and(|(_, all)| all), |d| d.bg(Theme::accent_soft()))
            .children(lines)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.begin_inspector_typing(id, begin.clone(), window, cx);
                    cx.notify();
                }),
            );
        // Only the box being typed in takes the keys.
        match active {
            Some(_) => field
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
                .into_any_element(),
            None => field.into_any_element(),
        }
    }

    /// For a use of a symbol: its placeholders to fill, then what it is and
    /// how it stands against your symbols.
    pub(super) fn symbol_sections(
        &mut self,
        id: ClipId,
        media: MediaId,
        symbol: &Symbol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let rendering = self
            .doc
            .project
            .media(media)
            .and_then(|m| self.made.renders.get(&m.hash))
            .map(|r| r.progress());

        // Placeholders first: they are why a use is selected.
        let placeholders = symbol.placeholders();
        let mut fields = self.section("Placeholders");
        if let Some(p) = rendering {
            fields = fields.child(
                div().h(px(2.0)).rounded_full().bg(Theme::lane()).child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(Theme::accent())
                        .w(gpui::relative(p.clamp(0.05, 1.0))),
                ),
            );
        }
        if placeholders.is_empty() {
            fields = fields.child(note(
                "None yet. Edit the symbol and switch on “Each use can change this” for a title, colour, picture or sound.",
            ));
        }
        for placeholder in placeholders {
            let pid = placeholder.id.clone();
            let now = symbol.shown(&placeholder);
            let changed = symbol.fills.contains_key(&pid);
            let reset = pid.clone();
            let header = div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(Theme::subtext())
                        .child(placeholder.label.clone()),
                )
                .when(changed, |d| {
                    d.child(
                        link(format!("reset-{pid}"), "Reset")
                            .tooltip(tip("Back to what the symbol has", None))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let _ = this.change_use(media, cx, |s| s.unfill(&reset));
                                cx.notify();
                            })),
                    )
                });
            let body: AnyElement = match &now {
                Fill::Text { text } => {
                    let typing = self
                        .title_edit
                        .as_ref()
                        .is_some_and(|e| e.clip == id && e.what == Typing::Fill(pid.clone()));
                    let field = self.words_box(
                        format!("fill-{pid}"),
                        id,
                        Typing::Fill(pid.clone()),
                        text.clone(),
                        window,
                        cx,
                    );
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(field)
                        .when(typing, |d| {
                            d.child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(Theme::overlay())
                                    .child("The picture follows as you type · ⌘↩ or Esc to finish"),
                            )
                        })
                        .into_any_element()
                }
                Fill::Color { color } => {
                    let color = *color;
                    let swatches: Vec<AnyElement> = SWATCHES
                        .iter()
                        .map(|&c| {
                            let pid = pid.clone();
                            self.swatch(
                                format!("fill-{pid}-{c:08x}"),
                                Some(c),
                                c == color,
                                cx,
                                move |this, cx| {
                                    let _ = this.fill_symbol(media, &pid, Fill::Color { color: c }, cx);
                                },
                            )
                            .into_any_element()
                        })
                        .collect();
                    let hex_typing = self
                        .title_edit
                        .as_ref()
                        .filter(|e| e.clip == id && e.what == Typing::Hex(pid.clone()))
                        .map(|e| e.scratch.clone());
                    let hex = hex_typing.unwrap_or_else(|| format!("#{:06x}", color >> 8));
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(div().flex().flex_wrap().gap(px(4.0)).children(swatches))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .size(px(22.0))
                                        .rounded(px(5.0))
                                        .border_1()
                                        .border_color(Theme::edge())
                                        .bg(gpui::rgba(color)),
                                )
                                .child(div().w(px(110.0)).font_family(MONO_FONT).child(self.words_box(
                                    format!("hex-{pid}"),
                                    id,
                                    Typing::Hex(pid.clone()),
                                    hex,
                                    window,
                                    cx,
                                )))
                                .child(div().text_size(px(10.0)).text_color(Theme::overlay()).child("hex, ↩")),
                        )
                        .into_any_element()
                }
                Fill::Media { media: shown } => self.media_slot(media, &placeholder.id, placeholder.takes, shown, cx),
            };
            fields = fields.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .pb(px(4.0))
                    .child(header)
                    .child(body),
            );
        }
        out.push(fields.into_any_element());

        // What it is, and how it stands against your symbols.
        let state = self.use_state(symbol);
        let uses = self.uses_of(symbol);
        let stale: Vec<MediaId> = uses
            .iter()
            .copied()
            .filter(|m| {
                matches!(self.doc.project.media(*m).and_then(|m| m.recipe.as_ref()),
                    Some(Recipe::Symbol(s)) if matches!(self.use_state(s), UseState::Newer { .. } | UseState::Different { .. }))
            })
            .collect();
        let (line, accent) = match &state {
            UseState::Current => (
                match uses.len() {
                    1 => "Edits to the symbol reach every use; what you fill in above is this use’s own.".to_string(),
                    n => format!("Used {n} times in this movie; what you fill in above is this use’s own."),
                },
                false,
            ),
            UseState::Newer { drops, .. } | UseState::Different { drops, .. } => {
                let what = if matches!(state, UseState::Newer { .. }) {
                    "There is a newer version in your symbols."
                } else {
                    "Your symbols have a different version of this (changed elsewhere, or older)."
                };
                let lost = if drops.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> = drops
                        .iter()
                        .map(|d| format!("{} ({})", super::reuse::shown(&d.fill), d.label))
                        .collect();
                    format!(" Updating would drop {}.", list.join(", "))
                };
                (format!("{what}{lost}"), true)
            }
            UseState::Missing => (
                "Not in your symbols on this Mac. This movie keeps its own copy, so it still plays; keep it to reuse it."
                    .to_string(),
                true,
            ),
        };
        let mut row = div().flex().gap(px(6.0));
        match &state {
            UseState::Newer { .. } | UseState::Different { .. } => {
                let one = vec![media];
                row = row.child(
                    wide("symbol-update", "Update", true)
                        .tooltip(tip(
                            "Take the version in your symbols, keeping what you filled in",
                            None,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_uses(one.clone(), None, cx);
                        })),
                );
                if stale.len() > 1 {
                    let all = stale.clone();
                    row = row.child(
                        wide("symbol-update-all", format!("Update all {}", stale.len()), false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.update_uses(all.clone(), None, cx);
                            },
                        )),
                    );
                }
            }
            UseState::Missing => {
                let name = symbol.name.clone();
                row = row.child(wide("symbol-keep", "Keep in my symbols…", true).on_click(cx.listener(
                    move |this, _, window, cx| {
                        let text = if symbols::taken(&name) {
                            format!("{name} 2")
                        } else {
                            name.clone()
                        };
                        this.open_naming(Naming::Keep(media), text, window, cx);
                    },
                )));
            }
            UseState::Current => {}
        }
        let host = self.doc.project.settings;
        let own = symbol.project.settings;
        let fit = symbol.shape_differs(host).then(|| {
            let reflow = symbol.reflow;
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(note(format!(
                    "Made for {}×{}; this movie is {}×{}.",
                    own.width, own.height, host.width, host.height
                )))
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(self.chip("fit-own", "Keep its shape", !reflow, cx, move |this, cx| {
                            let _ = this.change_use(media, cx, |s| {
                                s.reflow = false;
                                Ok(())
                            });
                        }))
                        .child(self.chip("fit-canvas", "Fit this movie", reflow, cx, move |this, cx| {
                            let _ = this.change_use(media, cx, |s| {
                                s.reflow = true;
                                Ok(())
                            });
                        })),
                )
        });
        let in_library = !matches!(state, UseState::Missing);
        let edit_name = match &state {
            UseState::Newer { name, .. } | UseState::Different { name, .. } => name.clone(),
            _ => symbol.name.clone(),
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .child(icon("symbol", Theme::accent()).size(px(13.0)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(Theme::text())
                    .child(symbol.name.clone()),
            )
            .when(in_library, |d| {
                d.child(
                    link("symbol-edit", "Edit")
                        .tooltip(tip("Open the symbol in its own window", None))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_symbol(&edit_name);
                            cx.notify();
                        })),
                )
            })
            .child(
                link("symbol-break", "Break apart")
                    .tooltip(tip("Turn this use back into its clips, to edit freely", Some("⌥⌘B")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.break_apart(id, cx);
                    })),
            );
        out.push(
            self.section("Symbol")
                .child(header)
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(16.0))
                        .text_color(if accent { Theme::accent() } else { Theme::overlay() })
                        .child(line),
                )
                .when(!matches!(state, UseState::Current), |d| d.child(row))
                .when_some(fit, |d, fit| d.child(fit))
                .into_any_element(),
        );
        out
    }

    /// What can go in a picture or sound placeholder: this project's media of
    /// that sort, the current one ringed, and Import. Media dropped on it goes
    /// in too.
    fn media_slot(&self, media: MediaId, pid: &str, takes: Takes, shown: &Media, cx: &mut Context<Self>) -> AnyElement {
        let choices: Vec<AnyElement> = self
            .doc
            .project
            .media
            .iter()
            .filter(|m| takes.accepts(m.kind) && !matches!(m.recipe, Some(Recipe::Symbol(_))))
            .map(|m| {
                let on = m.hash == shown.hash;
                let thumb = self.library.get(&m.hash).and_then(|a| a.thumb.clone());
                let (choice, pid) = (m.clone(), pid.to_string());
                let sound = m.kind == MediaKind::Audio;
                div()
                    .id(SharedString::from(format!("slot-{pid}-{}", m.id.0)))
                    .w(px(64.0))
                    .h(px(40.0))
                    .rounded(px(6.0))
                    .overflow_hidden()
                    .bg(Theme::crust())
                    .border_2()
                    .border_color(if on { Theme::accent() } else { Theme::edge() })
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| match thumb.filter(|_| !sound) {
                        Some(image) => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
                        None => d.child(icon(if sound { "music" } else { "image" }, Theme::overlay()).size(px(14.0))),
                    })
                    .tooltip(tip(m.name.clone(), None))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Err(e) = this.fill_symbol(
                            media,
                            &pid,
                            Fill::Media {
                                media: Box::new(choice.clone()),
                            },
                            cx,
                        ) {
                            this.say_error(e);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let import = div()
            .id(SharedString::from(format!("slot-import-{pid}")))
            .w(px(64.0))
            .h(px(40.0))
            .rounded(px(6.0))
            .border_1()
            .border_dashed()
            .border_color(Theme::surface2())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(Theme::lane()))
            .child(icon("plus", Theme::overlay()).size(px(12.0)))
            .child(div().text_size(px(9.5)).text_color(Theme::overlay()).child("Import"))
            .on_click(cx.listener(|this, _, window, cx| this.import(&Import, window, cx)));
        let size = (shown.width > 0 && takes == Takes::Picture).then(|| format!(" · {}×{}", shown.width, shown.height));
        let drop_pid = pid.to_string();
        div()
            .id(SharedString::from(format!("slot-{pid}")))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(7.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(Theme::edge())
            .drag_over::<MediaDrag>(|s, _, _, _| s.border_color(Theme::accent()).bg(Theme::surface0()))
            .on_drop(cx.listener(move |this, drag: &MediaDrag, _, cx| {
                cx.stop_propagation();
                if let Some(m) = this
                    .doc
                    .project
                    .media(drag.media)
                    .cloned()
                    .filter(|_| drag.generator.is_none() && drag.symbol.is_none())
                    && let Err(e) = this.fill_symbol(media, &drop_pid, Fill::Media { media: Box::new(m) }, cx)
                {
                    this.say_error(e);
                }
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(Theme::muted())
                    .truncate()
                    .child(format!("{}{}", shown.name, size.unwrap_or_default())),
            )
            .child(div().flex().flex_wrap().gap(px(4.0)).children(choices).child(import))
            .child(note(match takes {
                Takes::Sound => "A sound from this movie, or drag one here.",
                _ => "A picture or video from this movie, or drag one here. A video shorter than the slot plays once.",
            }))
            .into_any_element()
    }

    /// In a symbol's own window: whether this clip is a placeholder, and its name.
    pub(super) fn placeholder_section(
        &mut self,
        id: ClipId,
        clip: &Clip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = clip.placeholder.is_some();
        let mut section = self.section("Placeholder").child(self.toggle(
            "placeholder-toggle",
            "Each use can change this",
            on,
            cx,
            move |this, cx| {
                let label = (!on).then_some("");
                this.edit(cx, |p| p.set_placeholder(id, label));
            },
        ));
        if on {
            let label = clip.placeholder.as_ref().map(|t| t.label.clone()).unwrap_or_default();
            section = section
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(Theme::overlay())
                        .child("Its name, as uses see it"),
                )
                .child(self.words_box("placeholder-label".into(), id, Typing::Label, label, window, cx));
        }
        section
            .child(note(
                "Give two clips the same name and they change together. Renaming keeps what uses filled in.",
            ))
            .into_any_element()
    }

    /// In a symbol's own window, with nothing selected: the hold, the stretch
    /// that a longer or shorter use stretches.
    pub(super) fn hold_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.editing_a_symbol() {
            return None;
        }
        let project = &self.doc.project;
        let whole = project.duration();
        let fps = project.settings.fps;
        let at = self.player.position.clamp(0, whole);
        let (a, b) = project.hold.unwrap_or((0, whole));
        let shown = match project.hold {
            Some(_) => format!("{} to {}", timecode(a, fps), timecode(b, fps)),
            None => "All of it, evenly".into(),
        };
        let set = |this: &mut Editor, cx: &mut Context<Editor>, hold: Option<(Time, Time)>| {
            this.edit(cx, |p| p.hold = hold.filter(|(a, b)| a < b));
        };
        Some(
            self.section("Hold")
                .child(note(
                    "A use made longer or shorter (drag its end) stretches this part; what comes before and after keeps its timing.",
                ))
                .child(
                    div()
                        .font_family(MONO_FONT)
                        .text_size(px(12.0))
                        .text_color(Theme::text())
                        .child(shown),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(self.chip("hold-start", "Starts here", false, cx, move |this, cx| {
                            set(this, cx, Some((at, b.max(at + SECOND / 10))));
                        }))
                        .child(self.chip("hold-end", "Ends here", false, cx, move |this, cx| {
                            set(this, cx, Some((a.min(at - SECOND / 10).max(0), at)));
                        }))
                        .child(self.chip("hold-all", "All", project.hold.is_none(), cx, move |this, cx| {
                            set(this, cx, None);
                        })),
                )
                .into_any_element(),
        )
    }

    /// Across the top of a symbol's own window: what it is, and whether its
    /// changes are published yet.
    pub(super) fn symbol_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.editing_a_symbol() {
            return None;
        }
        let unpublished = self.doc.edited;
        Some(
            div()
                .h(px(30.0))
                .flex_none()
                .mb(px(render::GAP))
                .px(px(12.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .bg(Theme::accent_soft())
                .child(icon("symbol", Theme::accent()).size(px(13.0)))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Theme::text())
                        .child("Symbol"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(Theme::subtext())
                        .child(if unpublished {
                            "Changes here reach other movies when you publish them."
                        } else {
                            "Published. Movies that use it can update to this version."
                        }),
                )
                .when(unpublished, |d| {
                    d.child(
                        div()
                            .id("publish")
                            .h(px(22.0))
                            .px(px(10.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .bg(Theme::text())
                            .text_color(Theme::ground())
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .hover(|s| s.bg(Theme::subtext()))
                            .child("Publish  ⌘S")
                            .on_click(cx.listener(|this, _, window, cx| this.save(&Save, window, cx))),
                    )
                })
                .into_any_element(),
        )
    }

    /// A symbol in your symbols: its poster on a see-through ground, its
    /// length, and a menu on hover.
    pub(super) fn symbol_card(&self, entry: symbols::Entry, cx: &mut Context<Self>) -> AnyElement {
        let card = self.card_width;
        let name = entry.name.clone();
        let group: SharedString = format!("symbol-{name}").into();
        let duration = entry.info.as_ref().map_or(3 * SECOND, |i| i.duration.max(SECOND / 10));
        let face = match entry.poster() {
            Some(poster) => img(poster)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => icon("symbol", Theme::overlay()).size(px(20.0)).into_any_element(),
        };
        let drag = MediaDrag {
            media: MediaId(0),
            name: name.clone().into(),
            duration,
            generator: None,
            symbol: Some(name.clone().into()),
        };
        let (add, menu) = (name.clone(), name.clone());
        let badge = |child: AnyElement| {
            div()
                .absolute()
                .h(px(16.0))
                .px(px(4.0))
                .rounded(px(4.0))
                .bg(gpui::rgba(0x000000a6))
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(child)
        };
        div()
            .id(SharedString::from(format!("symbol-card-{name}")))
            .group(group.clone())
            .w(px(card))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .cursor_grab()
            .child(
                div()
                    .w(px(card))
                    .h(px(card * render::CARD_ASPECT))
                    .rounded(px(7.0))
                    .overflow_hidden()
                    .relative()
                    .bg(Theme::crust())
                    .border_1()
                    .border_color(Theme::edge())
                    .group_hover(group.clone(), |s| s.border_color(Theme::surface2()))
                    .child(super::shelf::dark_checkerboard())
                    .child(
                        div()
                            .absolute()
                            .inset(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(face),
                    )
                    .child(
                        badge(icon("symbol", Theme::text()).size(px(10.0)).into_any_element())
                            .top(px(4.0))
                            .left(px(4.0)),
                    )
                    .child(
                        badge(
                            div()
                                .font_family(MONO_FONT)
                                .text_size(px(9.5))
                                .text_color(Theme::text())
                                .child(format!("0:{:02}", secs(duration).round() as u64))
                                .into_any_element(),
                        )
                        .bottom(px(4.0))
                        .right(px(4.0)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("symbol-menu-{name}")))
                            .absolute()
                            .top(px(4.0))
                            .right(px(4.0))
                            .size(px(20.0))
                            .rounded(px(5.0))
                            .bg(gpui::rgba(0x000000b3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .invisible()
                            .group_hover(group.clone(), |s| s.visible())
                            .child(icon("more", Theme::text()).size(px(12.0)))
                            .tooltip(tip("Edit, rename, duplicate, show in Finder, move to Trash", None))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.overlay = Overlay::SymbolMenu {
                                    name: menu.clone(),
                                    index: 0,
                                };
                                window.focus(&this.palette_focus);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .w(px(card))
                    .px(px(1.0))
                    .text_size(px(10.5))
                    .text_color(Theme::muted())
                    .truncate()
                    .child(name),
            )
            .tooltip(tip("Click to add at the playhead, or drag onto the timeline", None))
            .on_drag(drag, |drag: &MediaDrag, _, _, cx| {
                cx.new(|_| DragChip(drag.name.clone()))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.add_symbol(add.clone(), None, None, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// Does what the symbol menu's row `index` says.
    pub(super) fn symbol_menu_pick(&mut self, name: &str, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(window, cx);
        let name = name.to_string();
        match index {
            0 => self.add_symbol(name, None, None, cx),
            1 => self.edit_symbol(&name),
            2 => self.open_naming(Naming::Rename(name.clone()), name, window, cx),
            3 => {
                let mut copy = format!("{name} copy");
                let mut n = 2;
                while symbols::taken(&copy) {
                    copy = format!("{name} copy {n}");
                    n += 1;
                }
                self.open_naming(Naming::Duplicate(name), copy, window, cx);
            }
            4 => cx.reveal_path(&symbols::file(&name)),
            5 => self.library_op(LibraryOp::Delete(name), None, cx),
            _ => {}
        }
        cx.notify();
    }

    /// The symbol menu and the naming sheet, as overlay content.
    pub(super) fn symbol_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.overlay {
            Overlay::SymbolMenu { name, index } => {
                let index = *index;
                let rows: Vec<AnyElement> = MENU
                    .iter()
                    .enumerate()
                    .map(|(i, (label, glyph))| {
                        let active = i == index;
                        let danger = i == MENU.len() - 1;
                        let pick = name.clone();
                        div()
                            .id(("symbol-menu-row", i))
                            .h(px(32.0))
                            .px(px(12.0))
                            .rounded(px(7.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .cursor_pointer()
                            .when(active, |d| d.bg(Theme::lane()))
                            .hover(|s| s.bg(Theme::lane()))
                            .child(icon(glyph, if danger { Theme::accent() } else { Theme::overlay() }).size(px(13.0)))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(if danger { Theme::accent() } else { Theme::text() })
                                    .child(*label),
                            )
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.symbol_menu_pick(&pick, i, window, cx)),
                            )
                            .into_any_element()
                    })
                    .collect();
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(44.0))
                                .px(px(16.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .border_b_1()
                                .border_color(Theme::rule())
                                .child(icon("symbol", Theme::accent()).size(px(14.0)))
                                .child(div().text_size(px(14.0)).text_color(Theme::text()).child(name.clone())),
                        )
                        .child(div().p(px(6.0)).flex().flex_col().children(rows))
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(14.0))
                                .flex()
                                .items_center()
                                .border_t_1()
                                .border_color(Theme::rule())
                                .text_size(px(10.5))
                                .text_color(Theme::overlay())
                                .child("↑↓ move  ·  ⏎ choose  ·  esc close  ·  Trash keeps it until emptied"),
                        )
                        .into_any_element(),
                )
            }
            Overlay::SymbolName { naming, text, all } => {
                let (title, action, detail): (String, &str, String) = match naming {
                    Naming::Make { clips, .. } => {
                        let n = clips.len();
                        (
                            "Make a symbol".into(),
                            "Make",
                            format!(
                                "{n} clip{} become one use of it, here; it goes in your symbols for any movie.",
                                if n == 1 { "" } else { "s" },
                            ),
                        )
                    }
                    Naming::Rename(from) => (
                        format!("Rename {from}"),
                        "Rename",
                        "Movies that use it follow the new name.".into(),
                    ),
                    Naming::Duplicate(from) => (
                        format!("Duplicate {from}"),
                        "Duplicate",
                        "A symbol of its own: changing one leaves the other.".into(),
                    ),
                    Naming::Keep(_) => (
                        "Keep in my symbols".into(),
                        "Keep",
                        "Puts this movie’s copy in your symbols, to reuse and edit.".into(),
                    ),
                };
                // Which titles each use can change: ticked ones become placeholders.
                let choices: Vec<AnyElement> = match naming {
                    Naming::Make { clips, plain, .. } => {
                        let mut titles: Vec<(usize, Time, ClipId, String)> = self
                            .doc
                            .project
                            .clips()
                            .filter(|(_, c)| clips.contains(&c.id))
                            .filter_map(|(t, c)| match &c.generator {
                                Some(Generator::Text(title)) => Some((
                                    t,
                                    c.start,
                                    c.id,
                                    title.text.lines().next().unwrap_or("").trim().to_string(),
                                )),
                                _ => None,
                            })
                            .collect();
                        titles.sort_by_key(|(t, start, _, _)| (*t, *start));
                        titles
                            .into_iter()
                            .map(|(_, _, clip, words)| {
                                let on = !plain.contains(&clip);
                                div()
                                    .id(("symbol-title-choice", clip.0 as usize))
                                    .h(px(28.0))
                                    .px(px(8.0))
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(Theme::lane()))
                                    .child(
                                        div()
                                            .size(px(14.0))
                                            .rounded(px(4.0))
                                            .border_1()
                                            .border_color(if on { Theme::accent() } else { Theme::surface2() })
                                            .when(on, |d| d.bg(Theme::accent()))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(on, |d| {
                                                d.child(div().size(px(6.0)).rounded_full().bg(Theme::ground()))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(12.0))
                                            .text_color(if on { Theme::text() } else { Theme::subtext() })
                                            .child(format!("“{words}”")),
                                    )
                                    .child(div().text_size(px(10.5)).text_color(Theme::overlay()).child(if on {
                                        "each use can change it"
                                    } else {
                                        "stays as it is"
                                    }))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Overlay::SymbolName {
                                            naming: Naming::Make { plain, .. },
                                            ..
                                        } = &mut this.overlay
                                        {
                                            if let Some(i) = plain.iter().position(|c| *c == clip) {
                                                plain.remove(i);
                                            } else {
                                                plain.push(clip);
                                            }
                                        }
                                        cx.notify();
                                    }))
                                    .into_any_element()
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                };
                let taken = symbols::taken(text.trim())
                    && !matches!(naming, Naming::Rename(from) if from.eq_ignore_ascii_case(text.trim()));
                let (all, text) = (*all, text.clone());
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(44.0))
                                .px(px(16.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .border_b_1()
                                .border_color(Theme::rule())
                                .child(icon("symbol", Theme::accent()).size(px(14.0)))
                                .child(div().text_size(px(14.0)).text_color(Theme::text()).child(title)),
                        )
                        .child(
                            div()
                                .p(px(16.0))
                                .flex()
                                .flex_col()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .h(px(34.0))
                                        .px(px(10.0))
                                        .rounded(px(8.0))
                                        .bg(Theme::crust())
                                        .border_1()
                                        .border_color(if taken { Theme::accent() } else { Theme::surface2() })
                                        .flex()
                                        .items_center()
                                        .text_size(px(14.0))
                                        .text_color(Theme::text())
                                        .child(
                                            div()
                                                .when(all && !text.is_empty(), |d| {
                                                    d.bg(Theme::accent_soft()).rounded(px(3.0))
                                                })
                                                .child(text),
                                        )
                                        .when(!all, |d| d.child(div().w(px(1.5)).h(px(17.0)).bg(Theme::accent()))),
                                )
                                .child(if taken {
                                    note("You already have a symbol with this name.").text_color(Theme::accent())
                                } else {
                                    note(detail)
                                })
                                .when(!choices.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .pt(px(4.0))
                                                    .text_size(px(10.5))
                                                    .text_color(Theme::overlay())
                                                    .child("PLACEHOLDERS: what each use can change"),
                                            )
                                            .children(choices),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .h(px(30.0))
                                .px(px(14.0))
                                .flex()
                                .items_center()
                                .border_t_1()
                                .border_color(Theme::rule())
                                .text_size(px(10.5))
                                .text_color(Theme::overlay())
                                .child(format!("⏎ {action}  ·  esc cancel")),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }
}
