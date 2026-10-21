//! Symbols in the editor: making one from clips or media, using one from your
//! symbols, filling its placeholders, keeping uses up to date, breaking one
//! apart, and managing the library. A use of a symbol is made media
//! (`Recipe::Symbol`) and renders like a scene. Each use has its own fills:
//! placing or duplicating a use makes another one (see `Project::insert_clip`).

use super::*;
use crate::symbols;
use splice_core::model::Recipe;
use splice_core::symbol::{Dropped, Fill, Symbol};
use std::collections::HashSet;

/// What the editor knows about your symbols.
#[derive(Default)]
pub struct Shelf {
    /// Newest first, each with what its file says once read.
    pub list: Vec<symbols::Entry>,
    /// What each symbol's file said, by name, and when it was read.
    infos: HashMap<String, (SystemTime, symbols::Info, Arc<Project>)>,
    /// A read of the library is under way.
    reading: bool,
    /// Posters drawn or being drawn this session, by name and version, so a
    /// failed one is not tried again and again.
    drawing: HashSet<(String, String)>,
    /// Typing a fill: which keystroke last scheduled a preview render.
    pub(super) typed: u64,
    /// Published versions already announced, so each is said once.
    announced: HashSet<String>,
}

/// A use, measured against your symbols.
#[derive(Clone, Debug, PartialEq)]
pub enum UseState {
    /// The same as the published symbol.
    Current,
    /// Your symbols have a later version of it.
    Newer { name: String, drops: Vec<Dropped> },
    /// Your symbols have a version that did not come from this one (an
    /// older one, or changed elsewhere).
    Different { name: String, drops: Vec<Dropped> },
    /// Not in your symbols on this Mac.
    Missing,
}

/// What a naming sheet is for.
#[derive(Clone, Debug, PartialEq)]
pub enum Naming {
    /// Make a symbol of these clips (or this media).
    Make {
        clips: Vec<ClipId>,
        media: Option<MediaId>,
        /// Titles among them to leave as they are, not placeholders.
        plain: Vec<ClipId>,
    },
    Rename(String),
    Duplicate(String),
    /// Keep a use's own copy in your symbols.
    Keep(MediaId),
}

impl Editor {
    /// Rereads your symbols in the background: what is there, and inside
    /// each file that changed. Then draws posters for new versions.
    pub(super) fn poll_symbols(&mut self, cx: &mut Context<Self>) {
        if self.symbols.reading {
            return;
        }
        self.symbols.reading = true;
        let known: HashMap<String, (SystemTime, symbols::Info, Arc<Project>)> = self.symbols.infos.clone();
        cx.spawn(async move |this, cx| {
            let (list, infos) = cx
                .background_spawn(async move {
                    let mut list = symbols::list();
                    let mut infos = HashMap::new();
                    for entry in &mut list {
                        let read = match known.get(&entry.name) {
                            Some(k) if k.0 == entry.modified => Some(k.clone()),
                            _ => symbols::info(&entry.name).ok().and_then(|info| {
                                let (_, project) = symbols::read(&entry.name).ok()?;
                                Some((entry.modified, info, Arc::new(project)))
                            }),
                        };
                        if let Some(read) = read {
                            entry.info = Some(read.1.clone());
                            infos.insert(entry.name.clone(), read);
                        }
                    }
                    (list, infos)
                })
                .await;
            this.update(cx, |editor, cx| {
                editor.symbols.reading = false;
                editor.symbols.infos = infos;
                editor.symbols.list = list;
                editor.draw_posters(cx);
                editor.announce_updates();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Says once when a symbol this movie uses has a newer version published,
    /// since the uses themselves only show it when selected.
    fn announce_updates(&mut self) {
        let mut newer: Vec<(String, usize)> = Vec::new();
        for media in &self.doc.project.media {
            let Some(Recipe::Symbol(symbol)) = &media.recipe else {
                continue;
            };
            if let UseState::Newer { name, .. } = self.use_state(symbol)
                && let Some((_, info, _)) = self.library_of(symbol)
                && !self.symbols.announced.contains(&info.version)
            {
                match newer.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, count)) => *count += 1,
                    None => newer.push((name, 1)),
                }
            }
        }
        for media in &self.doc.project.media {
            if let Some(Recipe::Symbol(symbol)) = &media.recipe
                && let Some((_, info, _)) = self.library_of(symbol)
            {
                let version = info.version.clone();
                self.symbols.announced.insert(version);
            }
        }
        if let Some((name, count)) = newer.first() {
            let uses = if *count == 1 {
                "its use".to_string()
            } else {
                format!("its {count} uses")
            };
            self.say(format!(
                "{name} has a newer version in your symbols: select {uses} to update"
            ));
        }
    }

    fn draw_posters(&mut self, cx: &mut Context<Self>) {
        for entry in self.symbols.list.clone() {
            let Some(info) = &entry.info else { continue };
            if entry.poster().is_some() || !self.symbols.drawing.insert((entry.name.clone(), info.version.clone())) {
                continue;
            }
            let Some((_, _, project)) = self.symbols.infos.get(&entry.name).cloned() else {
                continue;
            };
            let (name, version) = (entry.name.clone(), info.version.clone());
            cx.spawn(async move |this, cx| {
                let task = name.clone();
                let result = cx
                    .background_spawn(async move {
                        symbols::draw_poster(&project, &symbols::file(&task), &symbols::poster(&task, &version))
                    })
                    .await;
                if let Err(e) = result {
                    eprintln!("splice: no poster for {name}: {e}");
                }
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }
    }

    /// The library symbol a use came from: by identity, else by name.
    fn library_of(&self, symbol: &Symbol) -> Option<(&symbols::Entry, &symbols::Info, &Arc<Project>)> {
        let by = |e: &&symbols::Entry| match &e.info {
            Some(info) if !symbol.id.is_empty() => info.id == symbol.id,
            _ => symbol.id.is_empty() && e.name == symbol.name,
        };
        let entry = self.symbols.list.iter().find(by)?;
        let (_, info, project) = self.symbols.infos.get(&entry.name)?;
        Some((entry, info, project))
    }

    /// The name a use's symbol has in your symbols now.
    pub(super) fn library_name(&self, symbol: &Symbol) -> Option<String> {
        self.library_of(symbol).map(|(e, _, _)| e.name.clone())
    }

    /// Whether a newer version of a use's symbol is published: cheap, for
    /// drawing every frame.
    pub(super) fn has_newer(&self, symbol: &Symbol) -> bool {
        self.library_of(symbol)
            .is_some_and(|(_, info, _)| info.version != symbol.version && info.lineage.contains(&symbol.version))
    }

    /// A use against your symbols: current, newer, different or missing.
    pub(super) fn use_state(&self, symbol: &Symbol) -> UseState {
        let Some((entry, info, project)) = self.library_of(symbol) else {
            return UseState::Missing;
        };
        if info.version == symbol.version {
            return UseState::Current;
        }
        let name = entry.name.clone();
        let drops = symbol.would_drop(project);
        if info.lineage.contains(&symbol.version) {
            UseState::Newer { name, drops }
        } else {
            UseState::Different { name, drops }
        }
    }

    /// The uses of `symbol` in this project.
    pub(super) fn uses_of(&self, symbol: &Symbol) -> Vec<MediaId> {
        self.doc
            .project
            .media
            .iter()
            .filter(|m| match &m.recipe {
                Some(Recipe::Symbol(s)) => {
                    (!s.id.is_empty() && s.id == symbol.id) || (s.id.is_empty() && s.name == symbol.name)
                }
                _ => false,
            })
            .map(|m| m.id)
            .collect()
    }

    /// The identity of the symbol this window edits, if it is one.
    fn own_symbol(&self) -> Option<String> {
        if !self.editing_a_symbol() {
            return None;
        }
        let name = self.doc.name();
        self.symbols
            .infos
            .get(&name)
            .map(|(_, info, _)| info.id.clone())
            .or_else(|| symbols::info(&name).ok().map(|i| i.id))
    }

    /// A name for a new symbol: what its media or first title says, else
    /// "Symbol", not taken in your symbols.
    pub(super) fn suggest_name(&self, clips: &[ClipId], media: Option<MediaId>) -> String {
        let base = media
            .and_then(|m| self.doc.project.media(m))
            .map(|m| {
                Path::new(&m.name)
                    .file_stem()
                    .map_or(m.name.clone(), |s| s.to_string_lossy().to_string())
            })
            .or_else(|| {
                clips
                    .iter()
                    .filter_map(|id| self.doc.project.clip(*id))
                    .find_map(|c| match &c.generator {
                        Some(Generator::Text(t)) => t.text.lines().next().map(|l| l.trim().chars().take(32).collect()),
                        _ => None,
                    })
            })
            .filter(|l: &String| !l.is_empty())
            .unwrap_or_else(|| "Symbol".into());
        let base = symbols::clean_name(&base).unwrap_or_else(|_| "Symbol".into());
        let mut name = base.clone();
        let mut n = 2;
        while symbols::taken(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        name
    }

    /// Asks for a name, then makes the selected clips a symbol.
    pub(super) fn make_symbol_action(&mut self, _: &MakeSymbol, window: &mut Window, cx: &mut Context<Self>) {
        let clips = self.selection.clone();
        if clips.is_empty() {
            self.say("Select the clips to make into a symbol");
            return cx.notify();
        }
        let text = self.suggest_name(&clips, None);
        self.open_naming(
            Naming::Make {
                clips,
                media: None,
                plain: Vec::new(),
            },
            text,
            window,
            cx,
        );
    }

    /// Opens the naming sheet for `naming`, the field holding `text`.
    pub(super) fn open_naming(&mut self, naming: Naming, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Overlay::SymbolName {
            naming,
            text,
            all: true,
        };
        window.focus(&self.palette_focus);
        cx.notify();
    }

    /// Does what a naming sheet was for, with the name typed.
    pub(super) fn finish_naming(&mut self, naming: Naming, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = match symbols::clean_name(&name) {
            Ok(n) => n,
            Err(e) => {
                self.say_error(e.to_string());
                return cx.notify();
            }
        };
        self.close_overlay(window, cx);
        match naming {
            Naming::Make { clips, media, plain } => {
                self.make_symbol(Some(&name), clips, &plain, media, false, None, cx)
            }
            Naming::Rename(from) => self.library_op(LibraryOp::Rename(from, name), None, cx),
            Naming::Duplicate(from) => self.library_op(LibraryOp::Duplicate(from, name), None, cx),
            Naming::Keep(media) => self.keep_symbol(media, &name, None, cx),
        }
    }

    /// Turns clips into a symbol in your symbols. Unless `keep`, they are
    /// replaced on the timeline by one use of it. With one piece of media and
    /// no clips, the symbol is that media.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn make_symbol(
        &mut self,
        name: Option<&str>,
        clips: Vec<ClipId>,
        plain: &[ClipId],
        media: Option<MediaId>,
        keep: bool,
        reply: Option<Reply>,
        cx: &mut Context<Self>,
    ) {
        let piece = match media {
            Some(id) => {
                let Some(m) = self.doc.project.media(id).cloned() else {
                    return self.fail(&reply, "no such media".into(), cx);
                };
                if matches!(m.recipe, Some(Recipe::Symbol(_))) {
                    return self.fail(&reply, format!("{} is already a use of a symbol", m.name), cx);
                }
                let mut piece = Project::default();
                piece.settings = self.doc.project.settings;
                let id = piece.add_media(m);
                piece.insert_clip(id, 0, 0);
                Some(piece)
            }
            None => self.doc.project.extract_with(&clips, plain),
        };
        let Some(piece) = piece.filter(|p| p.duration() > 0) else {
            return self.fail(&reply, "Select the clips to make into a symbol".into(), cx);
        };
        let name = match name {
            Some(n) => match symbols::clean_name(n) {
                Ok(n) => n,
                Err(e) => return self.fail(&reply, e.to_string(), cx),
            },
            None => self.suggest_name(&clips, media),
        };
        if symbols::taken(&name) {
            return self.fail(
                &reply,
                format!("There is already a symbol called {name:?}; pick another name"),
                cx,
            );
        }
        let from = self.doc.path().to_path_buf();
        self.working += 1;
        self.say(format!("Making the symbol {name}…"));
        let saved = piece.clone();
        let task_name = name.clone();
        let wait = self.made_wait;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { symbols::save(&task_name, &saved, &from, None) })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                editor.poll_symbols(cx);
                let id = match result {
                    Ok(id) => id,
                    Err(e) => return editor.fail(&reply, format!("Could not make the symbol: {e}"), cx),
                };
                if keep || media.is_some() {
                    editor.say(format!("{name} is in your symbols"));
                    answer(&reply, serde_json::json!({ "ok": true, "symbol": name }));
                    return cx.notify();
                }
                let symbol = Symbol::new(&id, &name, piece);
                let duration = symbol.project.duration();
                let made = editor.made_media(name.clone(), Recipe::Symbol(symbol));
                let mut clip = None;
                let media = editor.edit(cx, |p| {
                    let id = p.add_media(made);
                    clip = p.replace_with(&clips, id, duration);
                    id
                });
                editor.selection = clip.into_iter().collect();
                editor.layout.inspector_open = true;
                editor.say(format!(
                    "{name} is in your symbols, and this is one use of it. Undo puts the clips back; {name} stays in your symbols"
                ));
                if let Some(reply) = reply {
                    let extra = serde_json::json!({ "symbol": name, "clip": clip.map(|c| c.0) });
                    if let Some(out) = editor.answer_made(media, extra, wait, reply.clone()) {
                        let _ = reply.send(out.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Brings the library symbol `name` into the project and, with `place`
    /// (else at the playhead, over what is there), onto the timeline.
    pub(super) fn add_symbol(
        &mut self,
        name: String,
        place: Option<(Time, usize)>,
        reply: Option<Reply>,
        cx: &mut Context<Self>,
    ) {
        let into = self.doc.path().to_path_buf();
        let own = self.own_symbol();
        let wait = self.made_wait;
        self.working += 1;
        let task_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let (id, project) = symbols::read(&task_name)?;
                    symbols::copy_media(&project, &symbols::file(&task_name), &Store::open(&into)?)?;
                    anyhow::Ok((id, project))
                })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                let (id, project) = match result {
                    Ok(p) => p,
                    Err(e) => return editor.fail(&reply, format!("Could not use the symbol {name}: {e}"), cx),
                };
                let symbol = Symbol::new(&id, &name, project);
                // A symbol cannot hold itself, at any depth.
                if own.as_deref().is_some_and(|own| symbol.uses(own)) {
                    let here = editor.doc.name();
                    return editor.fail(
                        &reply,
                        format!("{name} cannot go inside {here}: it would hold itself"),
                        cx,
                    );
                }
                let duration = symbol.project.duration();
                let made = editor.made_media(name.clone(), Recipe::Symbol(symbol));
                let at = place.map_or(editor.player.position, |(t, _)| t);
                let track = place.map_or_else(|| editor.layer_above(at, duration), |(_, track)| track);
                let mut clip = None;
                let id = editor.edit(cx, |p| {
                    let id = p.add_media(made);
                    clip = p.insert_clip(id, track, at);
                    id
                });
                editor.selection = clip.into_iter().collect();
                editor.layout.inspector_open = true;
                if let Some(reply) = reply {
                    let extra = serde_json::json!({ "clip": clip.map(|c| c.0) });
                    if let Some(out) = editor.answer_made(id, extra, wait, reply.clone()) {
                        let _ = reply.send(out.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Changes one use's symbol with `f`; it re-renders.
    pub(super) fn change_use(
        &mut self,
        media: MediaId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Symbol) -> anyhow::Result<()>,
    ) -> Result<(), String> {
        let Some(Recipe::Symbol(mut symbol)) = self.doc.project.media(media).and_then(|m| m.recipe.clone()) else {
            return Err("that is not a use of a symbol".into());
        };
        f(&mut symbol).map_err(|e| e.to_string())?;
        self.edit(cx, |p| p.set_recipe(media, Recipe::Symbol(symbol)));
        Ok(())
    }

    /// Puts `fill` in one use's placeholder `key` (its label or id).
    pub(super) fn fill_symbol(
        &mut self,
        media: MediaId,
        key: &str,
        fill: Fill,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.change_use(media, cx, |s| s.fill(key, fill))
    }

    /// Takes the published version for these uses (all of one symbol),
    /// keeping fills that still fit; says which do not.
    pub(super) fn update_uses(&mut self, uses: Vec<MediaId>, reply: Option<Reply>, cx: &mut Context<Self>) {
        let Some(Recipe::Symbol(first)) = uses
            .first()
            .and_then(|m| self.doc.project.media(*m))
            .and_then(|m| m.recipe.clone())
        else {
            return self.fail(&reply, "nothing to update".into(), cx);
        };
        let Some((entry, _, _)) = self.library_of(&first) else {
            return self.fail(&reply, format!("{} is not in your symbols", first.name), cx);
        };
        let name = entry.name.clone();
        let into = self.doc.path().to_path_buf();
        self.working += 1;
        cx.spawn(async move |this, cx| {
            let task = name.clone();
            let result = cx
                .background_spawn(async move {
                    let (_, project) = symbols::read(&task)?;
                    symbols::copy_media(&project, &symbols::file(&task), &Store::open(&into)?)?;
                    anyhow::Ok(project)
                })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                let project = match result {
                    Ok(p) => p,
                    Err(e) => return editor.fail(&reply, format!("Could not update: {e}"), cx),
                };
                let mut dropped: Vec<Dropped> = Vec::new();
                editor.edit(cx, |p| {
                    for media in &uses {
                        let Some(Recipe::Symbol(mut symbol)) = p.media(*media).and_then(|m| m.recipe.clone()) else {
                            continue;
                        };
                        dropped.extend(symbol.update(project.clone(), &name));
                        p.set_recipe(*media, Recipe::Symbol(symbol));
                    }
                });
                let lost: Vec<String> = dropped
                    .iter()
                    .map(|d| format!("{} ({})", shown(&d.fill), d.label))
                    .collect();
                let places = match uses.len() {
                    1 => String::new(),
                    n => format!(" in {n} places"),
                };
                if lost.is_empty() {
                    editor.say(format!("Updated {name}{places}"));
                } else {
                    editor.say_error(format!(
                        "Updated {name}{places}. These had no place left and were dropped (undo brings them back): {}",
                        lost.join(", ")
                    ));
                }
                if let Some(reply) = reply {
                    let mut out = editor.edited_with(serde_json::json!({
                        "updated": uses.iter().map(|m| m.0).collect::<Vec<_>>(),
                        "dropped": dropped,
                    }));
                    out["symbol"] = serde_json::json!(name);
                    let _ = reply.send(out.to_string());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Puts a use's own copy into your symbols as `name`, keeping its identity
    /// when that is free, so this use and others like it follow it again.
    pub(super) fn keep_symbol(&mut self, media: MediaId, name: &str, reply: Option<Reply>, cx: &mut Context<Self>) {
        let Some(Recipe::Symbol(symbol)) = self.doc.project.media(media).and_then(|m| m.recipe.clone()) else {
            return self.fail(&reply, "that is not a use of a symbol".into(), cx);
        };
        if symbols::taken(name) {
            return self.fail(&reply, format!("There is already a symbol called {name:?}"), cx);
        }
        let from = self.doc.path().to_path_buf();
        let (task, project) = (name.to_string(), (*symbol.project).clone());
        let id = (!symbol.id.is_empty() && self.library_of(&symbol).is_none()).then(|| symbol.id.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { symbols::save(&task, &project, &from, id.as_deref()) })
                .await;
            this.update(cx, |editor, cx| {
                editor.poll_symbols(cx);
                match result {
                    Ok(_) => {
                        editor.say(format!("{} is in your symbols", symbol.name));
                        answer(&reply, serde_json::json!({ "ok": true }));
                    }
                    Err(e) => editor.fail(&reply, format!("Could not keep it: {e}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Puts a use's clips back on the timeline, as they are in it.
    pub(super) fn break_apart(&mut self, clip: ClipId, cx: &mut Context<Self>) -> Vec<ClipId> {
        let made = self.edit(cx, |p| p.break_apart(clip));
        if made.is_empty() {
            self.say("Select a use of a symbol to break apart");
        } else {
            self.selection = made.clone();
            self.say("Broke the symbol apart: its clips are yours to edit, and no longer follow the symbol");
        }
        cx.notify();
        made
    }

    pub(super) fn break_apart_action(&mut self, _: &BreakApart, _: &mut Window, cx: &mut Context<Self>) {
        let first = self.selection.iter().copied().find(|id| {
            self.doc
                .project
                .clip(*id)
                .and_then(|c| self.doc.project.media(c.media))
                .is_some_and(|m| matches!(m.recipe, Some(Recipe::Symbol(_))))
        });
        match first {
            Some(clip) => {
                self.break_apart(clip, cx);
            }
            None => {
                self.say("Select a use of a symbol to break apart");
                cx.notify();
            }
        }
    }

    /// Changes your symbols: rename, copy, move to the Trash.
    pub(super) fn library_op(&mut self, op: LibraryOp, reply: Option<Reply>, cx: &mut Context<Self>) {
        let task = op.clone();
        self.working += 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match &task {
                        LibraryOp::Rename(from, to) => symbols::rename(from, to).map(|()| None),
                        LibraryOp::Duplicate(from, to) => symbols::duplicate(from, to).map(|()| None),
                        LibraryOp::Delete(name) => symbols::delete(name).map(Some),
                    }
                })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                editor.poll_symbols(cx);
                match result {
                    Ok(trashed) => {
                        let text = match &op {
                            LibraryOp::Rename(from, to) => format!("Renamed {from} to {to}; its uses follow it"),
                            LibraryOp::Duplicate(from, to) => format!("Copied {from} as {to}, a symbol of its own"),
                            LibraryOp::Delete(name) => {
                                format!("Moved {name} to the Trash; projects that use it keep their copy")
                            }
                        };
                        editor.say(text);
                        answer(&reply, serde_json::json!({ "ok": true, "trashed": trashed }));
                    }
                    Err(e) => editor.fail(&reply, e.to_string(), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn edit_symbol(&mut self, name: &str) {
        match symbols::open_window(name) {
            Ok(()) => self.say(format!(
                "Opened {name}. Save (⌘S) there to publish it; projects using it can then update"
            )),
            Err(e) => self.say_error(format!("Could not open {name}: {e}")),
        }
    }

    /// While a fill is typed, its use re-renders shortly after each pause,
    /// so the picture keeps up with the words.
    pub(super) fn preview_typed_fill(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.title_edit.as_ref().map(|e| &e.what),
            Some(super::inspector::Typing::Fill(_))
        ) {
            return;
        }
        self.symbols.typed += 1;
        let typed = self.symbols.typed;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(450)).await;
            this.update(cx, |editor, cx| {
                if editor.symbols.typed == typed && editor.title_edit.is_some() {
                    editor.reconcile_made(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn fail(&mut self, reply: &Option<Reply>, e: String, cx: &mut Context<Self>) {
        answer(reply, crate::ctl::fail(&e));
        self.say_error(e);
        cx.notify();
    }

    /// A use's placeholders and state, as `symbol read` reports them.
    fn symbol_json(&self, symbol: &Symbol) -> serde_json::Value {
        use serde_json::json;
        let state = self.use_state(symbol);
        let library = match &state {
            UseState::Current => json!({ "state": "current" }),
            UseState::Newer { name, drops } => json!({ "state": "newer", "name": name, "would_drop": drops }),
            UseState::Different { name, drops } => json!({ "state": "different", "name": name, "would_drop": drops }),
            UseState::Missing => json!({ "state": "missing" }),
        };
        let host = self.doc.project.settings;
        json!({
            "id": symbol.id,
            "name": symbol.name,
            "version": symbol.version,
            "size": format!("{}x{}", symbol.project.settings.width, symbol.project.settings.height),
            "fit": if symbol.reflow { "canvas" } else { "own" },
            "shape_differs": symbol.shape_differs(host),
            "duration": secs(symbol.project.duration()),
            "library": library,
            "update_available": matches!(state, UseState::Newer { .. } | UseState::Different { .. }),
            "placeholders": symbol.placeholders().iter().map(|p| json!({
                "id": p.id,
                "label": p.label,
                "takes": p.takes,
                "default": fill_json(&p.default),
                "now": fill_json(&symbol.shown(p)),
                "filled": symbol.fills.contains_key(&p.id),
            })).collect::<Vec<_>>(),
        })
    }

    /// A library symbol as `symbol list` and `symbol read --name` report it.
    fn library_json(&self, name: &str) -> Option<serde_json::Value> {
        use serde_json::json;
        let (_, info, project) = self.symbols.infos.get(name)?;
        let symbol = Symbol::new(&info.id, name, (**project).clone());
        Some(json!({
            "name": name,
            "id": info.id,
            "version": info.version,
            "duration": secs(info.duration),
            "size": format!("{}x{}", info.settings.width, info.settings.height),
            "placeholders": symbol.placeholders().iter().map(|p| json!({
                "id": p.id, "label": p.label, "takes": p.takes, "default": fill_json(&p.default),
            })).collect::<Vec<_>>(),
            "file": symbols::file(name),
        }))
    }

    /// Reads a library symbol now, for a request that needs it this moment.
    fn read_now(&mut self, name: &str) {
        if let Ok(info) = symbols::info(name)
            && let Ok((_, project)) = symbols::read(name)
        {
            let modified = std::fs::metadata(symbols::file(name))
                .and_then(|m| m.modified())
                .unwrap_or_else(|_| SystemTime::now());
            self.symbols
                .infos
                .insert(name.to_string(), (modified, info, Arc::new(project)));
        }
    }

    pub(super) fn ctl_symbol(
        &mut self,
        cmd: &str,
        request: &serde_json::Value,
        reply: Reply,
        cx: &mut Context<Self>,
    ) -> Option<serde_json::Value> {
        use crate::ctl::fail;
        use serde_json::{Value, json};
        let text = |key: &str| request.get(key).and_then(Value::as_str);
        let flag = |key: &str| request.get(key).and_then(Value::as_bool) == Some(true);
        let wait = flag("wait");
        self.made_wait = wait;
        if cmd == "placeholder" {
            if !self.editing_a_symbol() {
                return Some(fail(
                    "placeholders are marked in a symbol's own window: run `symbol edit --name N`, then this with --project N",
                ));
            }
            let clips = match self.clips_of(request) {
                Ok(c) if !c.is_empty() => c,
                Ok(_) => return Some(fail("give the clip with --clip C")),
                Err(e) => return Some(fail(e)),
            };
            let label = if flag("none") {
                None
            } else {
                Some(text("label").unwrap_or(""))
            };
            self.edit(cx, |p| {
                for id in &clips {
                    p.set_placeholder(*id, label);
                }
            });
            let tags: Vec<Value> = clips
                .iter()
                .filter_map(|c| self.doc.project.clip(*c))
                .map(|c| json!({ "clip": c.id.0, "placeholder": c.placeholder }))
                .collect();
            return Some(self.edited_with(json!({
                "marked": tags,
                "note": "save (splice ctl save) to publish it to projects that use this symbol",
            })));
        }
        if cmd == "hold" {
            if !self.editing_a_symbol() {
                return Some(fail(
                    "a hold is set in a symbol's own window: `symbol edit --name N`, then --project N",
                ));
            }
            let hold = if flag("clear") {
                None
            } else {
                match (self.time(request, "start"), self.time(request, "end")) {
                    (Ok(Some(a)), Ok(Some(b))) if a < b => Some((a, b.min(self.doc.project.duration()))),
                    (Err(e), _) | (_, Err(e)) => return Some(fail(e)),
                    _ => return Some(fail("give --start T --end T (start before end), or --clear")),
                }
            };
            self.edit(cx, |p| p.hold = hold);
            return Some(self.edited_with(json!({
                "hold": hold.map(|(a, b)| [secs(a), secs(b)]),
                "note": "save to publish",
            })));
        }
        // The use a request names: --media, or --clip of it.
        let used = |this: &Self| -> Result<(Media, Symbol), String> {
            let id = match request.get("media") {
                Some(m) => this.find_media(m)?,
                None => {
                    let clips = this.clips_of(request)?;
                    let clip = clips
                        .first()
                        .and_then(|c| this.doc.project.clip(*c))
                        .ok_or("give --media M or --clip C")?;
                    clip.media
                }
            };
            let media = this.doc.project.media(id).cloned().ok_or("no such media")?;
            match &media.recipe {
                Some(Recipe::Symbol(s)) => Ok((media.clone(), s.clone())),
                _ => Err(format!("{} is not a use of a symbol", media.name)),
            }
        };
        match text("sub").unwrap_or("list") {
            "list" => {
                let names: Vec<String> = symbols::list().into_iter().map(|e| e.name).collect();
                let list: Vec<Value> = names
                    .iter()
                    .map(|name| {
                        if !self.symbols.infos.contains_key(name) {
                            self.read_now(name);
                        }
                        self.library_json(name).unwrap_or_else(|| json!({ "name": name }))
                    })
                    .collect();
                self.poll_symbols(cx);
                Some(json!({ "ok": true, "folder": symbols::folder(), "symbols": list }))
            }
            "make" => {
                let media = match request.get("media") {
                    Some(m) => match self.find_media(m) {
                        Ok(id) => Some(id),
                        Err(e) => return Some(fail(e)),
                    },
                    None => None,
                };
                let clips = match self.clips_of(request) {
                    Ok(c) if c.is_empty() && media.is_none() => self.selection.clone(),
                    Ok(c) => c,
                    Err(e) => return Some(fail(e)),
                };
                self.make_symbol(text("name"), clips, &[], media, flag("keep"), Some(reply), cx);
                None
            }
            "add" => {
                let Some(name) = text("name") else {
                    return Some(fail("give the symbol's --name (see `symbol list`)"));
                };
                let place = match self.time(request, "at") {
                    Ok(Some(t)) => Some((t, self.track(request).unwrap_or(0))),
                    Ok(None) => None,
                    Err(e) => return Some(fail(e)),
                };
                self.add_symbol(name.to_string(), place, Some(reply), cx);
                None
            }
            "read" => {
                let by_name = text("name").filter(|_| request.get("media").is_none() && request.get("clips").is_none());
                if let Some(name) = by_name {
                    self.read_now(name);
                    return Some(match self.library_json(name) {
                        Some(v) => json!({ "ok": true, "symbol": v }),
                        None => fail(format!("there is no symbol called {name:?} in your symbols")),
                    });
                }
                match used(self) {
                    Ok((media, symbol)) => Some(json!({
                        "ok": true,
                        "media": self.made_json(media.id),
                        "symbol": self.symbol_json(&symbol),
                    })),
                    Err(e) => Some(fail(e)),
                }
            }
            "fill" => {
                let (media, _) = match used(self) {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Some(label) = text("label") else {
                    return Some(fail("give the placeholder's --label (see `symbol read`)"));
                };
                let changed = if flag("reset") {
                    self.change_use(media.id, cx, |s| s.unfill(label))
                } else {
                    let fill = if let Some(words) = text("text") {
                        Fill::Text {
                            text: words.to_string(),
                        }
                    } else if let Some(color) = text("color") {
                        match super::agent::parse_color(color) {
                            Some(c) => Fill::Color { color: c },
                            None => return Some(fail(format!("{color} is not a colour"))),
                        }
                    } else if let Some(other) = request.get("use") {
                        let found = self
                            .find_media(other)
                            .ok()
                            .and_then(|id| self.doc.project.media(id).cloned());
                        match found {
                            Some(m) if !matches!(m.recipe, Some(Recipe::Symbol(_))) => {
                                Fill::Media { media: Box::new(m) }
                            }
                            Some(m) => {
                                return Some(fail(format!("{} is a use of a symbol; fill with plain media", m.name)));
                            }
                            None => return Some(fail("--use names media in this project (import it first)")),
                        }
                    } else {
                        return Some(fail("give --text WORDS, --color C, --use MEDIA, or --reset"));
                    };
                    self.fill_symbol(media.id, label, fill, cx)
                };
                if let Err(e) = changed {
                    return Some(fail(e));
                }
                self.answer_made(media.id, json!({}), wait, reply)
            }
            "length" => {
                let (media, _) = match used(self) {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let Ok(Some(length)) = self.time(request, "duration") else {
                    return Some(fail("give --duration D, how long this use should run"));
                };
                if let Err(e) = self.change_use(media.id, cx, |s| {
                    s.set_length(length);
                    Ok(())
                }) {
                    return Some(fail(e));
                }
                self.answer_made(media.id, json!({}), wait, reply)
            }
            "fit" => {
                let (media, _) = match used(self) {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let reflow = match text("as") {
                    Some("canvas") => true,
                    Some("own") => false,
                    _ => {
                        return Some(fail(
                            "give --as canvas (lay it out for this movie's shape) or --as own (keep its shape)",
                        ));
                    }
                };
                if let Err(e) = self.change_use(media.id, cx, |s| {
                    s.reflow = reflow;
                    Ok(())
                }) {
                    return Some(fail(e));
                }
                self.answer_made(media.id, json!({}), wait, reply)
            }
            "update" => {
                let uses: Vec<MediaId> = if flag("all") {
                    self.doc
                        .project
                        .media
                        .iter()
                        .filter_map(|m| match &m.recipe {
                            Some(Recipe::Symbol(s))
                                if matches!(self.use_state(s), UseState::Newer { .. } | UseState::Different { .. })
                                    && text("name").is_none_or(|n| n.eq_ignore_ascii_case(&s.name)) =>
                            {
                                Some(m.id)
                            }
                            _ => None,
                        })
                        .collect()
                } else {
                    match used(self) {
                        Ok((m, _)) => vec![m.id],
                        Err(e) => return Some(fail(e)),
                    }
                };
                if uses.is_empty() {
                    return Some(json!({ "ok": true, "updated": [], "note": "every use is up to date" }));
                }
                // One symbol at a time; the last answers.
                let mut groups: Vec<(String, Vec<MediaId>)> = Vec::new();
                for m in uses {
                    let Some(Recipe::Symbol(s)) = self.doc.project.media(m).and_then(|m| m.recipe.as_ref()) else {
                        continue;
                    };
                    let key = format!("{}|{}", s.id, s.name);
                    match groups.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, g)) => g.push(m),
                        None => groups.push((key, vec![m])),
                    }
                }
                let last = groups.len().saturating_sub(1);
                for (i, (_, group)) in groups.into_iter().enumerate() {
                    self.update_uses(group, (i == last).then(|| reply.clone()), cx);
                }
                None
            }
            "edit" => {
                let name = match text("name") {
                    Some(n) => n.to_string(),
                    None => match used(self) {
                        Ok((_, s)) => match self.library_of(&s) {
                            Some((entry, _, _)) => entry.name.clone(),
                            None => return Some(fail(format!("{} is not in your symbols on this Mac", s.name))),
                        },
                        Err(_) => return Some(fail("give --name N, or --media M of a use")),
                    },
                };
                match symbols::open_window(&name) {
                    Ok(()) => Some(json!({
                        "ok": true,
                        "opened": symbols::file(&name),
                        "project": name,
                        "note": "work in that window with --project; save there to publish",
                    })),
                    Err(e) => Some(fail(e.to_string())),
                }
            }
            "keep" => {
                let (media, symbol) = match used(self) {
                    Ok(m) => m,
                    Err(e) => return Some(fail(e)),
                };
                let name = text("name").map_or(symbol.name.clone(), str::to_string);
                self.keep_symbol(media.id, &name, Some(reply), cx);
                None
            }
            "break" => {
                let clips = match self.clips_of(request) {
                    Ok(c) if !c.is_empty() => c,
                    _ => return Some(fail("give the use with --clip C")),
                };
                let made = self.break_apart(clips[0], cx);
                if made.is_empty() {
                    return Some(fail("that clip is not a use of a symbol"));
                }
                Some(self.edited_with(json!({ "clips": made.iter().map(|c| c.0).collect::<Vec<_>>() })))
            }
            "rename" | "duplicate" | "delete" => {
                let Some(name) = text("name") else {
                    return Some(fail("give the symbol's --name"));
                };
                let op = match (text("sub"), text("new")) {
                    (Some("delete"), _) => LibraryOp::Delete(name.to_string()),
                    (Some("rename"), Some(new)) => LibraryOp::Rename(name.to_string(), new.to_string()),
                    (Some("duplicate"), Some(new)) => LibraryOp::Duplicate(name.to_string(), new.to_string()),
                    _ => return Some(fail("give the new name with --new NAME")),
                };
                self.library_op(op, Some(reply), cx);
                None
            }
            other => Some(fail(format!(
                "unknown `symbol {other}`: use list, make, add, read, fill, length, fit, update, edit, keep, break, rename, duplicate or delete"
            ))),
        }
    }
}

/// A change to your symbols.
#[derive(Clone, Debug)]
pub enum LibraryOp {
    Rename(String, String),
    Duplicate(String, String),
    Delete(String),
}

pub type Reply = std::sync::mpsc::Sender<String>;

fn answer(reply: &Option<Reply>, value: serde_json::Value) {
    if let Some(reply) = reply {
        let _ = reply.send(value.to_string());
    }
}

/// A fill in words: what it is.
pub(super) fn shown(fill: &Fill) -> String {
    match fill {
        Fill::Text { text } => format!("“{}”", text.lines().next().unwrap_or("").trim()),
        Fill::Color { color } => format!("#{:06x}", color >> 8),
        Fill::Media { media } => media.name.clone(),
    }
}

fn fill_json(fill: &Fill) -> serde_json::Value {
    use serde_json::json;
    match fill {
        Fill::Text { text } => json!({ "text": text }),
        Fill::Color { color } => json!({ "color": format!("#{color:08x}") }),
        Fill::Media { media } => json!({ "media": media.name, "kind": media.kind }),
    }
}
