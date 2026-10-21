//! The editor window: media bin, viewer, timeline and status line, and every
//! command that acts on them.

use crate::actions::*;
use crate::audio::{AudioOut, Voice};
use crate::ffmpeg::{self, file_name};
use crate::library::{self, Library};
use crate::player::Player;
use crate::session::{Document, Layout, Recent, dirs};
use crate::theme::{MONO_FONT, Theme, UI_FONT};
use crate::timeline::{self, ClipArt, Ghost, GutterButton, Hit, View};
use crate::titles;
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Corners, DragMoveEvent, Entity, ExternalPaths, FocusHandle,
    HitboxBehavior, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    PathPromptOptions, Pixels, Point, RenderImage, ScrollWheelEvent, SharedString, Task, Window, canvas, div, img,
    point, prelude::*, px, quad, relative, size, transparent_black,
};
use splice_core::export::{self, Quality};
use splice_core::model::{ASPECTS, ClipId, Generator, Media, MediaId, MediaKind, Project};
use splice_core::store::{EXTENSION, Store, VersionKind, hash_file};
use splice_core::time::{SECOND, Time, secs, timecode};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

mod agent;
mod input;
mod inspector;
mod made;
mod record;
mod render;
mod shelf;
mod shots;
mod stage;
mod story;
mod syntax;
mod workbench;

pub(crate) use shelf::generated_name;

const AUTOSAVE_EVERY: Duration = Duration::from_millis(1500);

/// A divider drag: which one, where the pointer started and the size then.
#[derive(Clone, Copy)]
pub enum Resize {
    Bin { from: f32, width: f32 },
    Timeline { from: f32, height: f32 },
}

/// A media item being dragged out of the bin.
#[derive(Clone)]
pub struct MediaDrag {
    media: MediaId,
    name: SharedString,
    duration: Time,
    /// A title or colour from the shelf, rather than media.
    generator: Option<Generator>,
}

struct DragChip(SharedString);

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py(px(5.0))
            .rounded(px(7.0))
            .bg(Theme::surface0())
            .border_1()
            .border_color(Theme::accent())
            .shadow_lg()
            .text_size(px(11.5))
            .text_color(Theme::text())
            .font_family(UI_FONT)
            .child(self.0.clone())
    }
}

enum Drag {
    None,
    Scrub,
    Move {
        ids: Vec<ClipId>,
        anchor: Point<f32>,
        before: Project,
        moved: bool,
    },
    Trim {
        id: ClipId,
        left: bool,
        before: Project,
    },
    /// The end of a story beat, making it longer or shorter.
    BeatEnd {
        id: splice_core::story::BeatId,
        before: Project,
    },
}

enum Overlay {
    None,
    Palette {
        query: String,
        index: usize,
    },
    Versions {
        list: Vec<splice_core::store::Version>,
        index: usize,
    },
    /// A message for the connected assistant.
    Ask {
        text: String,
    },
    /// Choosing a title's font, each shown in itself.
    Fonts {
        clip: ClipId,
        query: String,
        index: usize,
        list: Vec<String>,
    },
    /// What to record: a screen or a window.
    Record {
        choices: Vec<record::Choice>,
        index: usize,
        microphone: bool,
        clicks: bool,
    },
    /// Choosing the instrument a score's part plays.
    Instruments {
        media: MediaId,
        part: usize,
        query: String,
        index: usize,
    },
}

#[derive(Default)]
struct ExportJob {
    output: PathBuf,
    progress: f32,
    done: Option<Result<(), String>>,
    child: Option<Child>,
}

struct Status {
    text: String,
    error: bool,
    at: Instant,
}

pub struct Editor {
    focus: FocusHandle,
    palette_focus: FocusHandle,
    doc: Document,
    library: Library,
    player: Player,
    audio: AudioOut,
    selection: Vec<ClipId>,
    bin_selection: Option<MediaId>,
    view: View,
    drag: Drag,
    ghost: Option<Ghost>,
    snap_line: Option<Time>,
    timeline_bounds: Rc<Cell<Bounds<Pixels>>>,
    stage_bounds: Rc<Cell<Bounds<Pixels>>>,
    status: Option<Status>,
    export: Option<Arc<Mutex<ExportJob>>>,
    /// Imports, unpacks and embeds still running.
    working: usize,
    overlay: Overlay,
    /// The command menu's or versions' list, kept scrolled to the chosen row.
    overlay_scroll: gpui::ScrollHandle,
    title: String,
    /// Keep the whole movie in view until the person zooms or scrolls.
    auto_fit: bool,
    layout: Layout,
    /// A panel divider being dragged.
    resizing: Option<Resize>,
    /// The largest media width and timeline height the window allows.
    limits: (f32, f32),
    /// The assistant connected through `splice ctl wait`, if any.
    agent: agent::Agent,
    /// `splice ctl export --wait` callers, answered when the export ends.
    export_replies: Vec<std::sync::mpsc::Sender<String>>,
    /// Titles and colours drawn for the viewer.
    drawings: titles::Drawings,
    /// Where each picture sat on the stage at the last paint, bottom first.
    stage_layers: Rc<std::cell::RefCell<Vec<stage::Placed>>>,
    /// Something held with the mouse on the stage or in the inspector.
    grab: Option<stage::Grab>,
    /// Centre lines shown while a picture snaps to them.
    guides: (bool, bool),
    slider_bounds: Rc<std::cell::RefCell<HashMap<inspector::Field, Bounds<Pixels>>>>,
    /// Inspector sections opened or closed by hand, per clip.
    folds: HashMap<(ClipId, &'static str), bool>,
    /// A title's words being typed in the inspector.
    title_edit: Option<inspector::TitleEdit>,
    text_focus: FocusHandle,
    /// Which shelf the left panel shows.
    shelf: Shelf,
    /// How wide each card in the left panel is, so a row fills it.
    card_width: f32,
    /// The timeline's layers were scrolled by hand, so they stay where put.
    scrolled_layers: bool,
    /// Scores and scenes: their renders, errors, studios.
    made: made::Made,
    /// Edit view or Story view.
    mode: story::Mode,
    /// The story beat chosen on the board or the lane.
    beat: Option<splice_core::story::BeatId>,
    /// Words being typed into a beat or a cue.
    typing: Option<story::Typing>,
    story_focus: FocusHandle,
    /// A screen recording under way.
    recording: Option<record::Active>,
    /// The reference shot each beat's card shows, when one was clicked.
    shown_shots: HashMap<splice_core::story::BeatId, splice_core::story::ShotId>,
    /// Takes to lay into a beat once they are imported.
    fill_after_import: HashMap<PathBuf, splice_core::story::BeatId>,
    /// The score or scene editor, when one is open.
    workbench: Option<workbench::Bench>,
    _ctl: Task<()>,
    _autosave: Task<()>,
    _watch: Task<()>,
}

/// The left panel's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shelf {
    Media,
    Text,
    Color,
}

impl Editor {
    pub fn new(open: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Editor {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut status = None;
        let doc = open
            .or_else(Recent::last)
            .and_then(|path| match Document::open(&path) {
                Ok(doc) => Some(doc),
                Err(e) => {
                    status = Some(Status {
                        text: e.to_string(),
                        error: true,
                        at: Instant::now(),
                    });
                    None
                }
            })
            .unwrap_or_else(|| Document::new_untitled().expect("cannot create a project in ~/Movies/Splice"));
        Recent::push(doc.path());

        let autosave = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSAVE_EVERY).await;
                if this.update(cx, |editor, cx| editor.autosave(cx)).is_err() {
                    break;
                }
            }
        });

        // Saves to scenes' code made outside Splice come back in.
        let watch = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(700)).await;
                if this.update(cx, |editor, cx| editor.poll_scenes(cx)).is_err() {
                    break;
                }
            }
        });

        if !ffmpeg::available() {
            status = Some(Status {
                text: "ffmpeg is not installed — run `brew install ffmpeg` to import, play and export".into(),
                error: true,
                at: Instant::now(),
            });
        }

        // Answer `splice ctl` while this window is open.
        let ctl = match crate::ctl::listen() {
            Some(mut incoming) => cx.spawn_in(window, async move |this, cx| {
                use futures::StreamExt;
                while let Some(request) = incoming.next().await {
                    if this
                        .update_in(cx, |editor, window, cx| editor.ctl_incoming(request, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }),
            None => Task::ready(()),
        };

        let mut editor = Editor {
            focus,
            palette_focus: cx.focus_handle(),
            doc,
            library: Library::default(),
            player: Player::new(),
            audio: AudioOut::open(),
            selection: Vec::new(),
            bin_selection: None,
            view: View::default(),
            drag: Drag::None,
            ghost: None,
            snap_line: None,
            timeline_bounds: Rc::new(Cell::new(Bounds::default())),
            stage_bounds: Rc::new(Cell::new(Bounds::default())),
            status,
            export: None,
            working: 0,
            overlay: Overlay::None,
            overlay_scroll: gpui::ScrollHandle::new(),
            title: String::new(),
            auto_fit: true,
            layout: Layout::load(),
            resizing: None,
            limits: (f32::MAX, f32::MAX),
            agent: agent::Agent::default(),
            export_replies: Vec::new(),
            drawings: titles::Drawings::default(),
            stage_layers: Rc::default(),
            grab: None,
            guides: (false, false),
            slider_bounds: Rc::default(),
            folds: HashMap::new(),
            title_edit: None,
            text_focus: cx.focus_handle(),
            shelf: Shelf::Media,
            card_width: render::CARD_MIN,
            scrolled_layers: false,
            made: made::Made::default(),
            workbench: None,
            mode: story::Mode::Edit,
            beat: None,
            typing: None,
            story_focus: cx.focus_handle(),
            recording: None,
            fill_after_import: HashMap::new(),
            shown_shots: HashMap::new(),
            _ctl: ctl,
            _autosave: autosave,
            _watch: watch,
        };
        editor.prepare_media(cx);
        // Coming back to Splice while it hides for a recording stops it.
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_activated(window.is_window_active(), cx);
        })
        .detach();
        editor
    }

    // Document lifecycle.

    fn autosave(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.doc.autosave() {
            self.say_error(format!("Autosave failed: {e}"));
        }
        if self
            .status
            .as_ref()
            .is_some_and(|s| s.at.elapsed() > Duration::from_secs(6))
        {
            self.status = None;
        }
        cx.notify();
    }

    /// Last write before quitting or closing.
    pub fn flush(&mut self) {
        self.doc.finish();
    }

    fn replace_document(&mut self, doc: Document, cx: &mut Context<Self>) {
        self.stop(cx);
        let mut old = std::mem::replace(&mut self.doc, doc);
        old.finish();
        self.player.reset();
        self.player.retire(self.library.images());
        self.library.clear();
        self.selection.clear();
        self.bin_selection = None;
        self.view = View::default();
        Recent::push(self.doc.path());
        self.prepare_media(cx);
        self.auto_fit = true;
        cx.notify();
    }

    /// Makes every piece of media readable: the extracted copy in the cache,
    /// else unpacked from the project file, else the original file.
    fn prepare_media(&mut self, cx: &mut Context<Self>) {
        let cache = &dirs().cache;
        for media in self.doc.project.media.clone() {
            if media.recipe.is_some() {
                continue;
            }
            if self
                .library
                .get(&media.hash)
                .is_some_and(|a| a.path.is_some() || a.loading)
            {
                continue;
            }
            let cached = library::cache_path(cache, &media);
            if cached.exists() {
                self.library.entry(&media.hash).path = Some(cached.clone());
                self.build_assets(media, cached, cx);
            } else if self.doc.store.has_media(&media.hash).unwrap_or(false) {
                self.unpack(media, cached, cx);
            } else if media.source.exists() {
                self.library.entry(&media.hash).path = Some(media.source.clone());
                self.embed(&media, media.source.clone(), cx);
                self.build_assets(media.clone(), media.source.clone(), cx);
            } else {
                self.library.entry(&media.hash).error = Some("offline".into());
            }
        }
        self.reconcile_made(cx);
    }

    fn unpack(&mut self, media: Media, dest: PathBuf, cx: &mut Context<Self>) {
        let store_path = self.doc.path().to_path_buf();
        self.library.entry(&media.hash).loading = true;
        self.working += 1;
        cx.spawn(async move |this, cx| {
            let target = dest.clone();
            let hash = media.hash.clone();
            let result = cx
                .background_spawn(async move { Store::open(&store_path)?.extract_media(&hash, &target) })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                match result {
                    Ok(()) => {
                        editor.library.entry(&media.hash).path = Some(dest.clone());
                        editor.build_assets(media, dest, cx);
                    }
                    Err(e) => {
                        editor.library.entry(&media.hash).loading = false;
                        editor.say_error(format!("Could not unpack {}: {e}", media.name));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn build_assets(&mut self, media: Media, path: PathBuf, cx: &mut Context<Self>) {
        let rate = self.audio.rate;
        self.library.entry(&media.hash).loading = true;
        cx.spawn(async move |this, cx| {
            let hash = media.hash.clone();
            let built = cx
                .background_spawn(async move { library::build(&media, &path, rate) })
                .await;
            this.update(cx, |editor, cx| {
                editor.library.apply(&hash, built);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Copies media into the project file so the project travels as one file.
    fn embed(&mut self, media: &Media, path: PathBuf, cx: &mut Context<Self>) {
        let store_path = self.doc.path().to_path_buf();
        let (hash, ext, name) = (media.hash.clone(), media.ext.clone(), media.name.clone());
        self.working += 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { Store::open(&store_path)?.put_media(&hash, &ext, &path) })
                .await;
            this.update(cx, |editor, cx| {
                editor.working -= 1;
                if let Err(e) = result {
                    editor.say_error(format!("Could not store {name} in the project: {e}"));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Where export finds this project's pictures.
    fn resolver(&self) -> titles::Resolver<impl Fn(&Media) -> Option<PathBuf> + '_> {
        let library = &self.library;
        titles::Resolver {
            media: move |m: &Media| library.path(&m.hash).map(Path::to_path_buf),
            dir: titles::drawn_dir(),
            canvas: (self.doc.project.settings.width, self.doc.project.settings.height),
        }
    }

    // Import.

    fn import_paths(
        &mut self,
        paths: Vec<PathBuf>,
        place: Option<(Time, usize)>,
        reply: Option<std::sync::mpsc::Sender<String>>,
        cx: &mut Context<Self>,
    ) {
        let (projects, media): (Vec<PathBuf>, Vec<PathBuf>) = paths
            .into_iter()
            .partition(|p| p.extension().is_some_and(|e| e == EXTENSION));
        if let Some(project) = projects.into_iter().next() {
            self.open_path(project, cx);
            if let Some(reply) = reply {
                let _ = reply.send(serde_json::json!({ "ok": true, "opened": self.doc.path() }).to_string());
            }
            return;
        }
        if media.is_empty() {
            return;
        }
        self.working += 1;
        self.say(format!(
            "Importing {} file{}…",
            media.len(),
            if media.len() == 1 { "" } else { "s" }
        ));
        cx.spawn(async move |this, cx| {
            let probed = cx
                .background_spawn(async move {
                    media
                        .into_iter()
                        .map(|path| {
                            let probe = ffmpeg::probe(&path)?;
                            let hash = hash_file(&path)?;
                            Ok((path, probe, hash))
                        })
                        .collect::<Vec<anyhow::Result<_>>>()
                })
                .await;
            this.update(cx, |editor, cx| editor.add_imported(probed, place, reply, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn add_imported(
        &mut self,
        probed: Vec<anyhow::Result<(PathBuf, ffmpeg::Probe, String)>>,
        place: Option<(Time, usize)>,
        reply: Option<std::sync::mpsc::Sender<String>>,
        cx: &mut Context<Self>,
    ) {
        self.working -= 1;
        let mut errors = Vec::new();
        let mut added: Vec<(Media, PathBuf)> = Vec::new();
        let mut placed = Vec::new();
        self.doc.edit(|project| {
            let mut at = place;
            for result in probed {
                match result {
                    Ok((path, probe, hash)) => {
                        let id = project.add_media(Media {
                            id: MediaId(0),
                            name: file_name(&path),
                            hash,
                            ext: path
                                .extension()
                                .map(|e| e.to_string_lossy().to_lowercase())
                                .unwrap_or_default(),
                            kind: probe.kind,
                            duration: probe.duration,
                            width: probe.width,
                            height: probe.height,
                            has_audio: probe.has_audio,
                            source: path.clone(),
                            recipe: None,
                        });
                        let media = project.media(id).cloned().expect("just added");
                        if let Some(beat) = self.fill_after_import.remove(&path) {
                            if let Some(clip) = project.fill_beat(beat, id) {
                                placed.push(clip);
                            }
                        } else if let Some((t, track)) = at
                            && let Some(clip) = project.insert_clip(id, track, t)
                        {
                            placed.push(clip);
                            let end = project.clip(clip).map_or(t, |c| c.end());
                            at = Some((end, track));
                        }
                        added.push((media, path));
                    }
                    Err(e) => errors.push(e.to_string()),
                }
            }
        });
        for (media, path) in added.iter().cloned() {
            if self.library.get(&media.hash).is_some_and(|a| a.path.is_some()) {
                continue;
            }
            self.library.entry(&media.hash).path = Some(path.clone());
            self.embed(&media, path.clone(), cx);
            self.build_assets(media, path, cx);
        }
        if let Some(reply) = reply {
            let media: Vec<_> = added
                .iter()
                .map(|(m, _)| serde_json::json!({ "id": m.id.0, "name": m.name }))
                .collect();
            let clips: Vec<_> = placed.iter().map(|c| c.0).collect();
            let answer = serde_json::json!({
                "ok": errors.is_empty(),
                "media": media,
                "clips": clips,
                "errors": errors,
                "revision": self.doc.revision,
            });
            let _ = reply.send(answer.to_string());
        }
        if !placed.is_empty() {
            self.selection = placed;
            self.after_edit();
        }
        match errors.first() {
            Some(e) => self.say_error(e.clone()),
            None => self.say(format!(
                "Imported {}",
                added
                    .iter()
                    .map(|(m, _)| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
        if self.doc.project.duration() > 0 && place.is_some() && added.len() == self.doc.project.media.len() {
            self.zoom_fit();
        }
        cx.notify();
    }

    /// Files from Finder: a project opens, media comes in.
    pub fn open_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.import_paths(paths, None, None, cx);
    }

    fn open_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path == self.doc.path() {
            return;
        }
        match Document::open(&path) {
            Ok(doc) => {
                self.replace_document(doc, cx);
            }
            Err(e) => self.say_error(format!("Could not open {}: {e}", file_name(&path))),
        }
    }

    // Edits.

    /// Keeps playback, selection and sound in step after any change.
    fn after_edit(&mut self) {
        let project = &self.doc.project;
        self.selection.retain(|id| project.slot(*id).is_some());
        if self.player.is_playing() {
            self.audio.stop();
            self.player.seek(self.player.position);
        }
    }

    fn edit<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Project) -> R) -> R {
        let result = self.doc.edit(f);
        self.after_edit();
        self.reconcile_made(cx);
        cx.notify();
        result
    }

    fn seek(&mut self, t: Time, cx: &mut Context<Self>) {
        let end = self.doc.project.duration();
        if self.player.is_playing() {
            self.audio.stop();
        }
        self.player.seek(t.clamp(0, end.max(0)));
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.player.pause();
        self.audio.stop();
        cx.notify();
    }

    fn start_sound(&mut self) {
        let audio = &self.audio;
        let voices = self
            .doc
            .project
            .audible_clips()
            .into_iter()
            .filter_map(|(clip, media)| {
                let pcm = self.library.get(&media.hash)?.pcm.clone()?;
                Some(Voice {
                    start: audio.frames(clip.start),
                    length: audio.frames(clip.duration),
                    source_in: audio.frames(clip.source_in),
                    volume: clip.volume,
                    pcm,
                })
            })
            .collect();
        self.audio.play(self.player.position, voices);
    }

    fn say(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    fn say_error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    fn targets(&self) -> Vec<ClipId> {
        self.selection.clone()
    }

    fn tracks_of_selection(&self) -> Vec<usize> {
        let mut tracks: Vec<usize> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.project.slot(*id))
            .map(|s| s.track)
            .collect();
        tracks.sort_unstable();
        tracks.dedup();
        if tracks.is_empty() {
            tracks.push(0);
        }
        tracks
    }

    // Zoom and scroll.

    fn timeline_width(&self) -> f32 {
        f32::from(self.timeline_bounds.get().size.width).max(400.0)
    }

    fn zoom(&mut self, factor: f32, anchor_x: Option<f32>) {
        self.auto_fit = false;
        let anchor = anchor_x.unwrap_or_else(|| {
            self.view
                .x(self.player.position)
                .clamp(timeline::GUTTER, self.timeline_width())
        });
        let t = self.view.t(anchor);
        self.view.pps = (self.view.pps * factor).clamp(2.0, 2400.0);
        self.view.scroll_x = (timeline::GUTTER + secs(t) as f32 * self.view.pps - anchor).max(0.0);
    }

    fn zoom_fit(&mut self) {
        self.auto_fit = true;
        let project = &self.doc.project;
        let duration = secs(project.duration().max(project.story.end())).max(10.0) as f32;
        self.view.pps = ((self.timeline_width() - timeline::GUTTER - 48.0) / duration).clamp(2.0, 2400.0);
        self.view.scroll_x = 0.0;
    }

    fn follow_playhead(&mut self) {
        let width = self.timeline_width();
        let x = self.view.x(self.player.position);
        if x > width - 24.0 || x < timeline::GUTTER {
            self.view.scroll_x = (secs(self.player.position) as f32 * self.view.pps - 40.0).max(0.0);
        }
    }

    // Timeline mouse.

    fn local(&self, p: Point<Pixels>) -> Point<f32> {
        let origin = self.timeline_bounds.get().origin;
        point(f32::from(p.x - origin.x), f32::from(p.y - origin.y))
    }

    fn timeline_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let p = self.local(event.position);
        let shift = event.modifiers.shift || event.modifiers.platform;
        match timeline::hit(&self.doc.project, &self.view, p) {
            Hit::Ruler => {
                self.drag = Drag::Scrub;
                self.seek(self.view.t(p.x), cx);
            }
            Hit::Gutter(track, Some(button)) => {
                self.edit(cx, |project| {
                    let t = &mut project.tracks[track];
                    match button {
                        GutterButton::Hide => t.hidden = !t.hidden,
                        GutterButton::Mute => t.muted = !t.muted,
                    }
                });
            }
            Hit::Gutter(_, None) => {}
            Hit::Beat(id) => {
                self.select_beat(Some(id), cx);
                if event.click_count >= 2 {
                    self.set_mode(story::Mode::Story, cx);
                }
            }
            Hit::BeatEnd(id) => {
                self.beat = Some(id);
                self.drag = Drag::BeatEnd {
                    id,
                    before: self.doc.project.clone(),
                };
            }
            Hit::StoryEmpty => {
                self.beat = None;
                if event.click_count >= 2 {
                    self.new_beat(window, cx);
                }
            }
            Hit::Body(id) => {
                let made = self
                    .doc
                    .project
                    .clip(id)
                    .filter(|c| c.generator.is_none())
                    .and_then(|c| self.doc.project.media(c.media))
                    .filter(|m| m.recipe.is_some())
                    .map(|m| m.id);
                if event.click_count >= 2
                    && let Some(media) = made
                {
                    self.selection = vec![id];
                    return self.open_workbench(media, window, cx);
                }
                // A title, as on the stage: double-click to type.
                if event.click_count >= 2
                    && self
                        .doc
                        .project
                        .clip(id)
                        .is_some_and(|c| matches!(c.generator, Some(Generator::Text(_))))
                {
                    self.begin_title_edit(id, window, cx);
                    window.prevent_default();
                    return cx.notify();
                }
                if shift {
                    if let Some(i) = self.selection.iter().position(|s| *s == id) {
                        self.selection.remove(i);
                    } else {
                        self.selection.push(id);
                    }
                } else if !self.selection.contains(&id) {
                    self.selection = vec![id];
                }
                if self.selection.contains(&id) {
                    self.drag = Drag::Move {
                        ids: self.selection.clone(),
                        anchor: p,
                        before: self.doc.project.clone(),
                        moved: false,
                    };
                }
            }
            Hit::Start(id) | Hit::End(id) => {
                let left = matches!(timeline::hit(&self.doc.project, &self.view, p), Hit::Start(_));
                self.selection = vec![id];
                self.drag = Drag::Trim {
                    id,
                    left,
                    before: self.doc.project.clone(),
                };
            }
            Hit::Empty => {
                if !shift {
                    self.selection.clear();
                }
                if p.x > timeline::GUTTER {
                    self.drag = Drag::Scrub;
                    self.seek(self.view.t(p.x), cx);
                }
            }
        }
        cx.notify();
    }

    fn timeline_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let p = self.local(event.position);
        let view = self.view;
        let position = self.player.position;
        match &mut self.drag {
            Drag::None => return,
            Drag::Scrub => {
                let t = view.t(p.x);
                self.seek(t, cx);
                return;
            }
            Drag::Move {
                ids,
                anchor,
                before,
                moved,
            } => {
                if !*moved && (p.x - anchor.x).abs() < 3.0 && (p.y - anchor.y).abs() < 3.0 {
                    return;
                }
                *moved = true;
                let n = before.tracks.len();
                let dtrack = view.track_at(p.y, n) as isize - view.track_at(anchor.y, n) as isize;
                let mut dt = view.px_to_time(p.x - anchor.x);
                let mut points = before.snap_points(ids);
                points.push(position);
                let edges: Vec<Time> = ids
                    .iter()
                    .filter_map(|id| before.clip(*id))
                    .flat_map(|c| [c.start + dt, c.end() + dt])
                    .collect();
                self.snap_line = None;
                if !event.modifiers.alt
                    && let Some((shift, at)) = timeline::snap(&view, &points, &edges)
                {
                    dt += shift;
                    self.snap_line = Some(at);
                }
                let mut project = before.clone();
                project.move_clips(ids, dt, dtrack);
                self.doc.project = project;
            }
            Drag::BeatEnd { id, before } => {
                let Some((start, _)) = before.story.span(*id) else {
                    return;
                };
                let mut t = view.t(p.x);
                let mut points: Vec<Time> = before.snap_points(&[]);
                points.push(position);
                points.extend(before.story.cues.iter().map(|c| c.at));
                self.snap_line = None;
                if !event.modifiers.alt
                    && let Some((shift, at)) = timeline::snap(&view, &points, &[t])
                {
                    t += shift;
                    self.snap_line = Some(at);
                }
                let mut project = before.clone();
                project.set_beat_duration(*id, t - start);
                self.doc.project = project;
            }
            Drag::Trim { id, left, before } => {
                let mut t = view.t(p.x);
                let mut points = before.snap_points(&[*id]);
                points.push(position);
                self.snap_line = None;
                if !event.modifiers.alt
                    && let Some((shift, at)) = timeline::snap(&view, &points, &[t])
                {
                    t += shift;
                    self.snap_line = Some(at);
                }
                let mut project = before.clone();
                if *left {
                    project.trim_start(*id, t);
                } else {
                    project.trim_end(*id, t);
                }
                self.doc.project = project;
            }
        }
        cx.notify();
    }

    fn timeline_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Move {
                before, moved: true, ..
            }
            | Drag::Trim { before, .. }
            | Drag::BeatEnd { before, .. } => {
                self.doc.commit(before);
                self.after_edit();
            }
            _ => {}
        }
        self.snap_line = None;
        cx.notify();
    }

    fn timeline_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(16.0));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.platform || event.modifiers.control {
            let anchor = self.local(event.position).x;
            self.zoom((1.0 + dy / 200.0).clamp(0.5, 2.0), Some(anchor));
        } else {
            let (dx, dy) = if event.modifiers.shift { (dy, 0.0) } else { (dx, dy) };
            if dx != 0.0 {
                self.auto_fit = false;
            }
            let content = secs(self.doc.project.duration()) as f32 * self.view.pps;
            self.view.scroll_x = (self.view.scroll_x - dx).clamp(0.0, (content - 100.0).max(0.0));
            let height = f32::from(self.timeline_bounds.get().size.height);
            let max_y = (self.view.content_height(self.doc.project.tracks.len()) - height).max(0.0);
            self.view.scroll_y = (self.view.scroll_y.max(0.0) - dy).clamp(0.0, max_y);
            if dy != 0.0 {
                self.scrolled_layers = true;
            }
        }
        cx.notify();
    }

    /// Where a drop at window position `p` would land: (start, track).
    fn drop_target(&self, p: Point<Pixels>) -> Option<(Time, usize)> {
        let b = self.timeline_bounds.get();
        if !b.contains(&p) {
            return None;
        }
        let local = self.local(p);
        if local.x < timeline::GUTTER || local.y < self.view.top() {
            return None;
        }
        let track = self.view.track_at(local.y, self.doc.project.tracks.len());
        Some((self.view.t(local.x).max(0), track))
    }

    fn hover_drop(&mut self, p: Point<Pixels>, duration: Time, cx: &mut Context<Self>) {
        let ghost = self
            .drop_target(p)
            .map(|(start, track)| Ghost { start, track, duration });
        let changed = ghost.map(|g| (g.start, g.track)) != self.ghost.map(|g| (g.start, g.track));
        self.ghost = ghost;
        if changed {
            cx.notify();
        }
    }

    // Commands.

    fn play_pause(&mut self, _: &PlayPause, _: &mut Window, cx: &mut Context<Self>) {
        if self.player.is_playing() {
            self.stop(cx);
        } else if self.doc.project.duration() > 0 {
            self.player.play(&self.doc.project);
            cx.notify();
        }
    }

    fn step(&mut self, by: Time, cx: &mut Context<Self>) {
        self.stop(cx);
        let frame = self.doc.project.frame();
        let t = splice_core::time::floor_to_frame(self.player.position + by + frame / 2, self.doc.project.settings.fps);
        self.seek(t, cx);
        self.follow_playhead();
    }

    fn split(&mut self, _: &Split, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.player.position;
        let targets = self.targets();
        let made = self.edit(cx, |p| p.split_at(at, &targets));
        if made.is_empty() {
            self.say("Nothing to split under the playhead");
        }
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.delete_notes(cx) {
            return;
        }
        if self.selection.is_empty() {
            self.say("Select a clip to delete");
            return cx.notify();
        }
        let targets = self.targets();
        self.edit(cx, |p| p.delete(&targets));
        self.selection.clear();
    }

    fn ripple_delete(&mut self, _: &RippleDelete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            self.say("Select a clip to delete");
            return cx.notify();
        }
        let targets = self.targets();
        self.edit(cx, |p| p.ripple_delete(&targets));
        self.selection.clear();
    }

    fn duplicate(&mut self, _: &Duplicate, _: &mut Window, cx: &mut Context<Self>) {
        let targets = self.targets();
        self.selection = self.edit(cx, |p| p.duplicate(&targets));
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = self.doc.project.clips().map(|(_, c)| c.id).collect();
        cx.notify();
    }

    fn deselect(&mut self, _: &Deselect, _: &mut Window, cx: &mut Context<Self>) {
        if self.bench_escape(cx) {
            return;
        }
        self.selection.clear();
        self.bin_selection = None;
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(bench) = &mut self.workbench {
            bench.notes.clear();
        }
        if self.doc.undo() {
            self.after_edit();
            self.prepare_media(cx);
        }
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(bench) = &mut self.workbench {
            bench.notes.clear();
        }
        if self.doc.redo() {
            self.after_edit();
            self.prepare_media(cx);
        }
        cx.notify();
    }

    fn toggle_track(&mut self, mute: bool, cx: &mut Context<Self>) {
        let tracks = self.tracks_of_selection();
        self.edit(cx, |p| {
            for i in tracks {
                if let Some(t) = p.tracks.get_mut(i) {
                    if mute {
                        t.muted = !t.muted;
                    } else {
                        t.hidden = !t.hidden;
                    }
                }
            }
        });
    }

    fn cycle_aspect(&mut self, _: &CycleAspect, _: &mut Window, cx: &mut Context<Self>) {
        let s = self.doc.project.settings;
        let current = ASPECTS.iter().position(|(_, w, h)| *w == s.width && *h == s.height);
        let (name, w, h) = ASPECTS[current.map_or(0, |i| (i + 1) % ASPECTS.len())];
        self.edit(cx, |p| {
            p.settings.width = w;
            p.settings.height = h;
        });
        self.say(format!("Canvas {name} · {w}×{h}"));
    }

    fn import(&mut self, _: &Import, _: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |editor, cx| editor.import_paths(paths, None, None, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |editor, cx| editor.open_path(path, cx)).ok();
            }
        })
        .detach();
    }

    fn new_project(&mut self, _: &NewProject, _: &mut Window, cx: &mut Context<Self>) {
        match Document::new_untitled() {
            Ok(doc) => self.replace_document(doc, cx),
            Err(e) => self.say_error(format!("Could not create a project: {e}")),
        }
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.is_untitled() {
            return self.save_as(&SaveAs, window, cx);
        }
        match self.doc.save(None) {
            Ok(()) => self.say("Saved a version"),
            Err(e) => self.say_error(format!("Save failed: {e}")),
        }
        cx.notify();
    }

    fn save_as(&mut self, _: &SaveAs, _: &mut Window, cx: &mut Context<Self>) {
        let name = format!("{}.{EXTENSION}", self.doc.name());
        let dir = self
            .doc
            .path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dirs().projects.clone());
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = path.await {
                if path.extension().is_none_or(|e| e != EXTENSION) {
                    path.set_extension(EXTENSION);
                }
                this.update(cx, |editor, cx| editor.save_to(path, cx)).ok();
            }
        })
        .detach();
    }

    fn save_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match self.doc.save_as(&path) {
            Ok(()) => {
                Recent::push(&path);
                self.say(format!("Saved as {}", file_name(&path)));
                // Media still being copied in went to the old file; finish it here.
                for media in self.doc.project.media.clone() {
                    if !self.doc.store.has_media(&media.hash).unwrap_or(true)
                        && let Some(source) = self.library.path(&media.hash).map(Path::to_path_buf)
                    {
                        self.embed(&media, source, cx);
                    }
                }
            }
            Err(e) => self.say_error(format!("Save failed: {e}")),
        }
        cx.notify();
    }

    fn versions(&mut self, _: &Versions, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Versions { .. }) {
            return self.close_overlay(window, cx);
        }
        let _ = self.doc.autosave();
        self.overlay = Overlay::Versions {
            list: self.doc.versions(),
            index: 0,
        };
        self.overlay_scroll = gpui::ScrollHandle::new();
        // Its keys: arrows, return, escape.
        window.focus(&self.palette_focus);
        cx.notify();
    }

    fn restore(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.stop(cx);
        match self.doc.restore(id) {
            Ok(()) => {
                self.after_edit();
                self.prepare_media(cx);
                self.say("Restored that version — undo brings back what you had");
            }
            Err(e) => self.say_error(format!("Could not restore: {e}")),
        }
        self.close_overlay(window, cx);
    }

    fn export(&mut self, quality: Quality, cx: &mut Context<Self>) {
        if self.export.is_some() {
            self.say("An export is already running");
            return cx.notify();
        }
        if self.doc.project.duration() == 0 {
            self.say("Put something on the timeline to export");
            return cx.notify();
        }
        let dir = directories::UserDirs::new()
            .and_then(|u| u.video_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| dirs().projects.clone());
        let path = cx.prompt_for_new_path(&dir, Some(&format!("{}.mp4", self.doc.name())));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = path.await {
                if path.extension().is_none() {
                    path.set_extension("mp4");
                }
                this.update(cx, |editor, cx| {
                    if let Err(e) = editor.start_export(path, quality, true, cx) {
                        editor.say_error(e);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Starts encoding the movie to `output`; `reveal` shows it in Finder when done.
    fn start_export(
        &mut self,
        output: PathBuf,
        quality: Quality,
        reveal: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(waiting) = self.unrendered_in_use() {
            return Err(format!("Cannot export yet: {waiting}"));
        }
        let plan = match export::plan(&self.doc.project, &self.resolver(), &output, quality) {
            Ok(plan) => plan,
            Err(e) => return Err(format!("Cannot export: {e}")),
        };
        let mut command = ffmpeg::export_command().map_err(|e| e.to_string())?;
        let child = command
            .args(&plan.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => return Err(format!("Could not start ffmpeg: {e}")),
        };
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let job = Arc::new(Mutex::new(ExportJob {
            output: output.clone(),
            child: Some(child),
            ..Default::default()
        }));
        let shared = job.clone();
        let duration = plan.duration;
        std::thread::spawn(move || {
            let errors = std::thread::spawn(move || {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            });
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(t) = export::parse_progress(&line) {
                    shared.lock().unwrap().progress = (t as f32 / duration as f32).clamp(0.0, 1.0);
                }
            }
            let child = shared.lock().unwrap().child.take();
            let status = child.map(|mut c| c.wait());
            let errors = errors.join().unwrap_or_default();
            let result = match status {
                Some(Ok(s)) if s.success() => Ok(()),
                None => Err("cancelled".to_string()),
                _ => Err(errors.lines().last().unwrap_or("ffmpeg failed").to_string()),
            };
            shared.lock().unwrap().done = Some(result);
        });
        self.export = Some(job.clone());
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let done = job.lock().unwrap().done.clone();
                let keep_going = this
                    .update(cx, |editor, cx| {
                        cx.notify();
                        let Some(result) = done else { return true };
                        editor.export = None;
                        let answer = match &result {
                            Ok(()) => serde_json::json!({ "ok": true, "path": output }),
                            Err(e) => serde_json::json!({ "ok": false, "error": e }),
                        };
                        for reply in editor.export_replies.drain(..) {
                            let _ = reply.send(answer.to_string());
                        }
                        match result {
                            Ok(()) => {
                                editor.say(format!("Exported {}", file_name(&output)));
                                if reveal {
                                    cx.reveal_path(&output);
                                }
                            }
                            Err(e) if e == "cancelled" => editor.say("Export cancelled"),
                            Err(e) => editor.say_error(format!("Export failed: {e}")),
                        }
                        false
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
        Ok(())
    }

    fn cancel_export(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.export {
            let mut job = job.lock().unwrap();
            if let Some(child) = job.child.as_mut() {
                let _ = child.kill();
            }
            let _ = std::fs::remove_file(&job.output);
            job.child = None;
        }
        cx.notify();
    }

    fn open_palette(&mut self, _: &CommandMenu, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Palette { .. }) {
            return self.close_overlay(window, cx);
        }
        self.overlay = Overlay::Palette {
            query: String::new(),
            index: 0,
        };
        self.overlay_scroll = gpui::ScrollHandle::new();
        window.focus(&self.palette_focus);
        cx.notify();
    }

    fn ask_agent(&mut self, _: &AskAgent, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Ask { .. }) {
            return self.close_overlay(window, cx);
        }
        self.overlay = Overlay::Ask { text: String::new() };
        window.focus(&self.palette_focus);
        cx.notify();
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Overlay::None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn palette_matches(query: &str) -> Vec<(&'static str, &'static str, Box<dyn gpui::Action>)> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let typed = words.join(" ");
        let mut found: Vec<_> = commands()
            .into_iter()
            .filter(|(label, _, _)| words.iter().all(|w| label.to_lowercase().contains(w)))
            .collect();
        // What was typed exactly first, then what starts with it, so
        // "show media" is Show media rather than Show / hide media.
        found.sort_by_key(|(label, _, _)| {
            let label = label.to_lowercase();
            if label == typed {
                0
            } else if label.starts_with(&typed) {
                1
            } else {
                2
            }
        });
        found
    }

    fn overlay_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let typed = event.keystroke.key_char.clone();
        let command = event.keystroke.modifiers.platform || event.keystroke.modifiers.control;
        match &mut self.overlay {
            Overlay::None => return,
            Overlay::Palette { query, index } => match key {
                "escape" => return self.close_overlay(window, cx),
                "up" => *index = index.saturating_sub(1),
                "down" => *index = (*index + 1).min(Self::palette_matches(query).len().saturating_sub(1)),
                "backspace" => {
                    query.pop();
                    *index = 0;
                }
                "enter" => {
                    let matches = Self::palette_matches(query);
                    let chosen = matches.into_iter().nth(*index).map(|(_, _, a)| a);
                    self.close_overlay(window, cx);
                    if let Some(action) = chosen {
                        window.dispatch_action(action, cx);
                    }
                    return;
                }
                _ if !command => {
                    if let Some(c) = typed.filter(|c| !c.chars().any(char::is_control)) {
                        query.push_str(&c);
                        *index = 0;
                    }
                }
                _ => {}
            },
            Overlay::Ask { text } => match key {
                "escape" => return self.close_overlay(window, cx),
                "backspace" if command => text.clear(),
                "backspace" => {
                    text.pop();
                }
                "enter" if event.keystroke.modifiers.shift => text.push('\n'),
                "enter" => {
                    let message = text.trim().to_string();
                    if message.is_empty() {
                        return;
                    }
                    self.close_overlay(window, cx);
                    return self.send_to_agent(message, cx);
                }
                "v" if command => {
                    if let Some(pasted) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        text.push_str(&pasted);
                    }
                }
                _ if !command => {
                    if let Some(c) = typed.filter(|c| !c.chars().any(char::is_control)) {
                        text.push_str(&c);
                    }
                }
                _ => {}
            },
            Overlay::Fonts {
                clip,
                query,
                index,
                list,
            } => match key {
                "escape" => return self.close_overlay(window, cx),
                "up" => *index = index.saturating_sub(1),
                "down" => *index += 1,
                "backspace" => {
                    query.pop();
                    *index = 0;
                }
                "enter" => {
                    let chosen = Self::font_matches(list, query).into_iter().nth(*index);
                    let clip = *clip;
                    match chosen {
                        Some(font) => return self.choose_font(clip, font, window, cx),
                        None => return self.close_overlay(window, cx),
                    }
                }
                _ if !command => {
                    if let Some(c) = typed.filter(|c| !c.chars().any(char::is_control)) {
                        query.push_str(&c);
                        *index = 0;
                    }
                }
                _ => {}
            },
            Overlay::Record { choices, index, .. } => match key {
                "escape" => return self.close_overlay(window, cx),
                "up" => *index = index.saturating_sub(1),
                "down" => *index = (*index + 1).min(choices.len().saturating_sub(1)),
                "enter" => return self.start_from_overlay(window, cx),
                _ => {}
            },
            Overlay::Instruments { part, query, index, .. } => match key {
                "escape" => return self.close_overlay(window, cx),
                "up" => *index = index.saturating_sub(1),
                "down" => *index += 1,
                "backspace" => {
                    query.pop();
                    *index = 0;
                }
                "enter" => {
                    let part = *part;
                    match Self::instrument_matches(query).into_iter().nth(*index) {
                        Some((choice, _)) => return self.choose_instrument(part, choice, window, cx),
                        None => return self.close_overlay(window, cx),
                    }
                }
                _ if !command => {
                    if let Some(c) = typed.filter(|c| !c.chars().any(char::is_control)) {
                        query.push_str(&c);
                        *index = 0;
                    }
                }
                _ => {}
            },
            Overlay::Versions { list, index } => match key {
                "escape" => return self.close_overlay(window, cx),
                "up" => *index = index.saturating_sub(1),
                "down" => *index = (*index + 1).min(list.len().saturating_sub(1)),
                "enter" => {
                    if let Some(v) = list.get(*index) {
                        let id = v.id;
                        return self.restore(id, window, cx);
                    }
                }
                _ => {}
            },
        }
        if let ("up" | "down", Overlay::Palette { index, .. } | Overlay::Versions { index, .. }) = (key, &self.overlay)
        {
            self.overlay_scroll.scroll_to_item(*index);
        }
        cx.stop_propagation();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_commands_come_first() {
        let first = |q: &str| Editor::palette_matches(q).first().map(|(label, _, _)| *label);
        assert_eq!(first("show media"), Some("Show media"));
        assert_eq!(first("versions"), Some("Versions"));
        assert_eq!(first("new score"), Some("New score (music to compose)"));
    }
}
