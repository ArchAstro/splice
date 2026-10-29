//! The Record tab: what the project records (a screen or a window, a
//! camera, a microphone, the sound the Mac plays), seen live and laid out as
//! a scene, and how the files are made. A take writes each source to its own
//! file and comes onto the timeline as layers in the scene's places, where
//! they can still be moved. Splice keeps itself out of the shot. Stop from
//! the title bar, ⇧⌘R, or `splice ctl record stop`.

use super::render::{GAP, caps, icon, panel, tip};
use super::*;
use crate::capture::{self, Devices, Feed, Happened, Live};
use crate::recorder;
use gpui::FontWeight;
use serde_json::{Value, json};
use splice_core::look::Look;
use splice_core::story::BeatId;
use splice_core::studio::{self, Bubble, Codec, Corner, Layout, Quality, Screen, Studio, Take};
use std::sync::mpsc::Sender;

/// Where a take goes when it is done.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Into {
    /// Just into the media panel.
    Media,
    /// Laid into a beat.
    Beat(BeatId),
    /// On the timeline at a time, from a layer up.
    At(Time, usize),
    /// After everything on the timeline.
    End,
}

/// A list opened in the sources panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Screen,
    Camera,
    Microphone,
}

/// Recording: the helper while the sources are live, and what waits on it.
#[derive(Default)]
pub struct Recorder {
    /// The setup the user made their default.
    default: Option<Studio>,
    pub live: Option<Live>,
    devices: Option<Devices>,
    listing: bool,
    /// Where the take being recorded goes.
    into: Option<Into>,
    /// Asked to record before the sources were live: since when.
    pending: Option<Instant>,
    /// `record start` callers waiting for the take to begin.
    starters: Vec<Sender<String>>,
    /// `record stop --wait` callers.
    waiters: Vec<Sender<String>>,
    pick: Option<Pick>,
    /// The last take's folder.
    pub last: Option<PathBuf>,
    polling: bool,
    problem: Option<String>,
    /// Where the stage is drawn, measured as it is painted.
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder {
            default: saved_default(),
            ..Recorder::default()
        }
    }

    pub fn recording(&self) -> bool {
        self.pending.is_some() || self.live.as_ref().is_some_and(|l| l.recording.is_some())
    }

    fn paused(&self) -> bool {
        self.live
            .as_ref()
            .and_then(|l| l.recording.as_ref())
            .is_some_and(|r| r.paused.is_some())
    }

    fn seconds(&self) -> f64 {
        self.live.as_ref().map_or(0.0, Live::seconds)
    }
}

fn defaults_file() -> PathBuf {
    dirs().data.join("studio.json")
}

/// The setup the user has made their default, for projects that have none.
fn saved_default() -> Option<Studio> {
    let text = std::fs::read_to_string(defaults_file()).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_default(studio: &Studio) -> std::io::Result<()> {
    std::fs::create_dir_all(&dirs().data)?;
    std::fs::write(
        defaults_file(),
        serde_json::to_string_pretty(studio).unwrap_or_default(),
    )
}

fn screen_name(screen: &Option<Screen>) -> String {
    match screen {
        None => "None".into(),
        Some(Screen::Display { number: 1 }) => "Main display".into(),
        Some(Screen::Display { number }) => format!("Display {number}"),
        Some(Screen::Window { app, title }) if title.is_empty() => format!("{app} window"),
        Some(Screen::Window { app, title }) => format!("{app} — {title}"),
        Some(Screen::Region { width, height, .. }) => format!("Part of the screen, {width:.0}×{height:.0}"),
    }
}

impl Editor {
    // The setup.

    /// What this project records: its own setup, else the user's default,
    /// else the main display and the Mac's microphone.
    pub(super) fn studio(&self) -> Studio {
        if let Some(s) = &self.doc.project.studio {
            return s.clone();
        }
        self.rec.default.clone().unwrap_or_else(|| Studio {
            screen: Some(Screen::Display { number: 1 }),
            microphone: self.rec.devices.as_ref().and_then(|d| {
                d.microphones.iter().find(|m| m.default).map(|m| studio::Device {
                    id: m.id.clone(),
                    name: m.name.clone(),
                })
            }),
            ..Studio::default()
        })
    }

    /// Changes the project's setup (one undo step), and the live sources to match.
    pub(super) fn set_studio(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Studio)) {
        let mut studio = self.studio();
        f(&mut studio);
        self.edit(cx, |p| p.studio = Some(studio));
        self.relive(cx);
    }

    fn list_devices(&mut self, cx: &mut Context<Self>) {
        if self.rec.listing {
            return;
        }
        self.rec.listing = true;
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { capture::devices() }).await;
            this.update(cx, |editor, cx| {
                editor.rec.listing = false;
                match found {
                    Ok(devices) => {
                        editor.rec.devices = Some(devices);
                        editor.relive(cx);
                    }
                    Err(e) => editor.rec.problem = Some(format!("Could not list cameras and microphones: {e}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keeps the sources live while the Record tab is open or a take is
    /// under way, matching the setup; lets them go otherwise.
    pub(super) fn relive(&mut self, cx: &mut Context<Self>) {
        let wanted = self.mode == story::Mode::Record || self.rec.recording();
        if self.rec.live.as_ref().is_some_and(|l| l.recording.is_some()) {
            return;
        }
        if !wanted {
            self.let_go();
            return;
        }
        let Some(devices) = &self.rec.devices else {
            return self.list_devices(cx);
        };
        let studio = self.studio();
        if !studio.has_sources() {
            self.let_go();
            self.rec.problem = None;
            return;
        }
        let config = match capture::config(&studio, devices, 480) {
            Ok(config) => config,
            Err(e) => {
                self.let_go();
                self.rec.problem = Some(e.to_string());
                return;
            }
        };
        if let Some(live) = &mut self.rec.live
            && live.config == config
            && live.running()
        {
            return;
        }
        self.let_go();
        let area = studio
            .screen
            .as_ref()
            .filter(|_| studio.output.clicks)
            .and_then(|s| capture::source(s).ok());
        match Live::start(config, area) {
            Ok(live) => {
                self.rec.live = Some(live);
                self.rec.problem = None;
                self.keep_polling(cx);
            }
            Err(e) => self.rec.problem = Some(e.to_string()),
        }
        cx.notify();
    }

    fn let_go(&mut self) {
        if let Some(live) = self.rec.live.take() {
            self.player.retire(live.frames.values().cloned().collect());
        }
    }

    fn keep_polling(&mut self, cx: &mut Context<Self>) {
        if self.rec.polling {
            return;
        }
        self.rec.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(40)).await;
                let going = this.update(cx, |editor, cx| editor.poll_recorder(cx)).unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes in what the helper has said. False when there is no helper to hear.
    fn poll_recorder(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(live) = &mut self.rec.live else {
            self.rec.polling = false;
            return false;
        };
        let mut retired = Vec::new();
        let (happened, heard) = live.poll(&mut retired);
        let ready = live.live;
        self.player.retire(retired);
        if let Some(asked) = self.rec.pending {
            if ready {
                self.rec.pending = None;
                self.begin_take(cx);
            } else if asked.elapsed() > Duration::from_secs(10) {
                self.rec.pending = None;
                self.take_failed("the sources did not start; check the Record tab".into());
            }
        }
        for h in happened {
            match h {
                Happened::Recording => self.say("Recording"),
                Happened::Paused => self.say("Paused"),
                Happened::Resumed => self.say("Recording"),
                Happened::Warning(w) => self.say(w),
                Happened::Stopped(Ok(take)) => {
                    let into = self.rec.into.take().unwrap_or(Into::End);
                    let reply = std::mem::take(&mut self.rec.waiters).into_iter().next();
                    self.import_take(take, into, reply, cx);
                    self.relive(cx);
                }
                Happened::Stopped(Err(e)) => {
                    self.rec.into = None;
                    self.take_failed(format!("Recording failed: {e}"));
                }
                Happened::Failed(e) => {
                    self.rec.problem = Some(e.clone());
                    self.let_go();
                    self.take_failed(e);
                }
            }
        }
        // A new picture or level to show, or the clock to tick.
        if heard && (self.mode == story::Mode::Record || self.rec.recording()) {
            cx.notify();
        }
        true
    }

    fn take_failed(&mut self, e: String) {
        self.say_error(e.clone());
        for w in std::mem::take(&mut self.rec.starters)
            .into_iter()
            .chain(std::mem::take(&mut self.rec.waiters))
        {
            let _ = w.send(crate::ctl::fail(e.clone()).to_string());
        }
    }

    // Takes.

    /// ⇧⌘R, the title bar's button, Story's Record: stop a take; record
    /// into the chosen beat in Story; record in the Record tab; else open it.
    pub(super) fn open_record(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.rec.recording() {
            return self.stop_take(cx);
        }
        let into = match (self.mode, self.beat) {
            (story::Mode::Story, Some(beat)) => Into::Beat(beat),
            (story::Mode::Record, _) => Into::End,
            _ => return self.set_mode(story::Mode::Record, cx),
        };
        if !self.studio().has_sources() {
            return self.set_mode(story::Mode::Record, cx);
        }
        if let Err(e) = self.start_take(into, None, cx) {
            self.say_error(e);
        }
    }

    /// Records a take with the project's setup. `reply` hears once it has begun.
    pub(super) fn start_take(
        &mut self,
        into: Into,
        reply: Option<Sender<String>>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.rec.recording() {
            return Err("already recording; stop it first".into());
        }
        if !self.studio().has_sources() {
            return Err("nothing to record: choose a screen, camera or microphone in the Record tab".into());
        }
        self.rec.into = Some(into);
        self.rec.pending = Some(Instant::now());
        self.rec.starters.extend(reply);
        self.say("Starting the sources…");
        self.relive(cx);
        if self.rec.live.is_none() && self.rec.devices.is_some() {
            self.rec.pending = None;
            let e = self
                .rec
                .problem
                .clone()
                .unwrap_or_else(|| "the sources could not start".into());
            self.take_failed(e.clone());
            return Err(e);
        }
        self.keep_polling(cx);
        cx.notify();
        Ok(())
    }

    fn begin_take(&mut self, cx: &mut Context<Self>) {
        let dir = capture::take_dir(self.doc.path());
        let Some(live) = &mut self.rec.live else { return };
        match live.record(dir.clone()) {
            Ok(()) => {
                for s in std::mem::take(&mut self.rec.starters) {
                    let _ = s.send(json!({ "ok": true, "recording": true, "dir": dir }).to_string());
                }
            }
            Err(e) => self.take_failed(e.to_string()),
        }
        cx.notify();
    }

    /// Stops the take; it comes in when its files are finished.
    pub(super) fn stop_take(&mut self, cx: &mut Context<Self>) {
        if self.rec.pending.take().is_some() {
            self.rec.into = None;
            self.take_failed("stopped before the sources were live".into());
            return;
        }
        if let Some(live) = &mut self.rec.live
            && live.recording.is_some()
        {
            live.stop();
            let seconds = live.seconds();
            self.say(format!("Finishing the take ({seconds:.0}s)…"));
        }
        cx.notify();
    }

    fn pause_take(&mut self, cx: &mut Context<Self>) {
        let paused = self.rec.paused();
        if let Some(live) = &mut self.rec.live
            && live.recording.is_some()
        {
            if paused { live.resume() } else { live.pause() }
        }
        cx.notify();
    }

    fn import_take(&mut self, take: Take, into: Into, reply: Option<Sender<String>>, cx: &mut Context<Self>) {
        self.working += 1;
        self.rec.last = take
            .files()
            .first()
            .and_then(|(_, p)| p.parent())
            .map(Path::to_path_buf);
        let files: Vec<(&'static str, PathBuf)> = take.files().into_iter().map(|(n, p)| (n, p.to_path_buf())).collect();
        cx.spawn(async move |this, cx| {
            let probed = cx
                .background_spawn(async move {
                    files
                        .into_iter()
                        .map(|(name, path)| {
                            let probe = ffmpeg::probe(&path)?;
                            let hash = hash_file(&path)?;
                            Ok((name, path, probe, hash))
                        })
                        .collect::<Vec<anyhow::Result<_>>>()
                })
                .await;
            this.update(cx, |editor, cx| editor.add_take(probed, take, into, reply, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    #[allow(clippy::type_complexity)]
    fn add_take(
        &mut self,
        probed: Vec<anyhow::Result<(&'static str, PathBuf, ffmpeg::Probe, String)>>,
        take: Take,
        into: Into,
        reply: Option<Sender<String>>,
        cx: &mut Context<Self>,
    ) {
        self.working -= 1;
        let studio = self.studio();
        let mut errors = Vec::new();
        let mut added: Vec<(Media, PathBuf)> = Vec::new();
        let mut placed: Vec<ClipId> = Vec::new();
        let first = self.doc.project.is_empty();
        self.doc.edit(|project| {
            let mut ids: HashMap<&str, MediaId> = HashMap::new();
            for result in probed {
                match result {
                    Ok((name, path, probe, hash)) => {
                        let id = project.add_media(Media {
                            id: MediaId(0),
                            name: format!("{} {}", name_of(name), take_name(&path)),
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
                            content: None,
                            clicks: recorder::read_clicks(&path),
                        });
                        ids.insert(name, id);
                        added.push((project.media(id).cloned().expect("just added"), path));
                    }
                    Err(e) => errors.push(e.to_string()),
                }
            }
            let size = |p: &Project, name: &str| {
                ids.get(name)
                    .and_then(|id| p.media(*id))
                    .filter(|m| m.width > 0)
                    .map(|m| (m.width, m.height))
            };
            match into {
                Into::Media => {}
                Into::Beat(beat) => {
                    let main = if studio.scene.layout == Layout::Camera {
                        ["camera", "screen"]
                    } else {
                        ["screen", "camera"]
                    };
                    if let Some(id) = main.iter().find_map(|n| ids.get(n))
                        && let Some(clip) = project.fill_beat(beat, *id)
                    {
                        placed.push(clip);
                    }
                }
                Into::At(..) | Into::End => {
                    let (t, track) = match into {
                        Into::At(t, track) => (t, track),
                        _ => (project.duration(), 0),
                    };
                    let canvas = (project.settings.width, project.settings.height);
                    let shown = studio.looks(canvas, size(project, "screen"), size(project, "camera"));
                    let mut clips: HashMap<&str, ClipId> = HashMap::new();
                    for (name, wanted) in [
                        ("screen", shown.screen.is_some()),
                        ("camera", shown.camera.is_some()),
                        ("microphone", true),
                        ("system", true),
                    ] {
                        if wanted
                            && let Some(id) = ids.get(name)
                            && let Some(clip) = project.insert_clip(*id, track, t)
                        {
                            clips.insert(name, clip);
                            placed.push(clip);
                        }
                    }
                    // The first pictures in a project set its shape: place them by it.
                    let canvas = (project.settings.width, project.settings.height);
                    let shown = studio.looks(canvas, size(project, "screen"), size(project, "camera"));
                    for (name, look) in [("screen", shown.screen), ("camera", shown.camera)] {
                        if let (Some(clip), Some(look)) = (clips.get(name), look)
                            && look != Look::default()
                        {
                            project.set_look(*clip, |l| *l = look);
                        }
                    }
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
                .map(|(m, _)| json!({ "id": m.id.0, "name": m.name }))
                .collect();
            let _ = reply.send(
                json!({
                    "ok": errors.is_empty(),
                    "take": self.rec.last,
                    "seconds": take.seconds,
                    "media": media,
                    "clips": placed.iter().map(|c| c.0).collect::<Vec<_>>(),
                    "errors": errors,
                    "revision": self.doc.revision,
                })
                .to_string(),
            );
        }
        if !placed.is_empty() {
            self.selection = placed;
            self.after_edit();
        }
        match errors.first() {
            Some(e) => self.say_error(e.clone()),
            None => self.say(format!(
                "Recorded {:.0}s: {}",
                take.seconds,
                take.files().iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
            )),
        }
        if first && !self.doc.project.is_empty() {
            self.zoom_fit();
        }
        cx.notify();
    }

    // Drawing.

    /// The title bar's record control: a button, or the running clock.
    pub(super) fn record_button(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.rec.recording() {
            return render::icon_button("record", "record", true)
                .tooltip(tip("Record", Some("⇧⌘R")))
                .on_click(cx.listener(|this, _, window, cx| this.open_record(window, cx)))
                .into_any_element();
        }
        let s = self.rec.seconds() as u64;
        let paused = self.rec.paused();
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
            .child(
                div()
                    .size(px(8.0))
                    .rounded_full()
                    .bg(if paused { Theme::overlay() } else { Theme::accent() }),
            )
            .child(
                div()
                    .font_family(MONO_FONT)
                    .text_size(px(11.5))
                    .text_color(Theme::text())
                    .child(format!("{}:{:02}", s / 60, s % 60)),
            )
            .child(div().text_size(px(11.5)).text_color(Theme::text()).child("Stop"))
            .tooltip(tip("Stop recording", Some("⇧⌘R")))
            .on_click(cx.listener(|this, _, _, cx| this.stop_take(cx)))
            .into_any_element()
    }

    /// The Record tab: the scene, live, and beside it the sources and how
    /// the files are made.
    pub(super) fn render_studio(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.rec.devices.is_none() {
            self.list_devices(cx);
        }
        let studio = self.studio();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .gap(px(GAP))
            .child(
                panel()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.render_scene(&studio, cx))
                    .child(self.scene_bar(&studio, cx)),
            )
            .child(
                panel().w(px(292.0)).flex_none().flex().flex_col().child(
                    div()
                        .id("studio-side")
                        .flex_1()
                        .overflow_y_scroll()
                        .p(px(14.0))
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(self.sources_panel(&studio, cx))
                        .child(self.output_panel(&studio, cx)),
                ),
            )
            .into_any_element()
    }

    /// The canvas, with each live picture where the scene puts it.
    fn render_scene(&mut self, studio: &Studio, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.doc.project.settings;
        let cell = self.rec.bounds.clone();
        let measure = canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
            .absolute()
            .size_full();
        let area = self.rec.bounds.get();
        let (bw, bh) = (f32::from(area.size.width) - 40.0, f32::from(area.size.height) - 32.0);
        let aspect = settings.width as f32 / settings.height.max(1) as f32;
        let (w, h) = if bw / bh.max(1.0) > aspect {
            (bh * aspect, bh)
        } else {
            (bw, bw / aspect)
        };
        let (w, h) = (w.max(1.0), h.max(1.0));
        let (left, top) = (
            (f32::from(area.size.width) - w) / 2.0,
            (f32::from(area.size.height) - h) / 2.0,
        );
        let frames = self.rec.live.as_ref().map(|l| l.frames.clone()).unwrap_or_default();
        let dims = |f: Feed| {
            frames.get(&f).map(|i| {
                let s = i.size(0);
                (s.width.0 as u32, s.height.0 as u32)
            })
        };
        let canvas_px = (settings.width, settings.height);
        let shown = studio.looks(canvas_px, dims(Feed::Screen), dims(Feed::Camera));
        let mut layers: Vec<AnyElement> = Vec::new();
        for (feed, look) in [(Feed::Screen, shown.screen), (Feed::Camera, shown.camera)] {
            let (Some(look), Some(image)) = (look, frames.get(&feed)) else {
                continue;
            };
            let Some(natural) = dims(feed) else { continue };
            layers.push(picture(image.clone(), natural, &look, (w, h)));
        }
        let failed = self.rec.problem.is_some();
        let status = if let Some(problem) = &self.rec.problem {
            Some(problem.clone())
        } else if !studio.has_sources() {
            Some("Choose what to record on the right.".into())
        } else if let Some(what) = self.rec.live.as_ref().and_then(|l| l.asking.clone()) {
            Some(format!("Waiting for you to allow the {what}: macOS is asking."))
        } else if self.rec.live.as_ref().is_none_or(|l| !l.live) {
            Some("Starting the sources…".into())
        } else {
            None
        };
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(measure)
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(w))
                    .h(px(h))
                    .overflow_hidden()
                    .bg(gpui::black())
                    .children(layers)
                    .when_some(status, |d, s| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(gpui::rgba(0x000000b3))
                                .p(px(20.0))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .w_full()
                                        .text_center()
                                        .text_size(px(12.5))
                                        .line_height(px(18.0))
                                        .text_color(Theme::subtext())
                                        .child(s),
                                )
                                .when(failed, |d| {
                                    d.child(chip("studio-retry", "Try again", false).on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.rec.problem = None;
                                            this.let_go();
                                            this.relive(cx);
                                        },
                                    )))
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Layouts, and the record and pause buttons.
    fn scene_bar(&mut self, studio: &Studio, cx: &mut Context<Self>) -> impl IntoElement {
        let scene = studio.scene;
        let recording = self.rec.recording();
        let layouts = Layout::ALL.map(|layout| {
            chip(("layout", layout as usize), layout.name(), scene.layout == layout)
                .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.scene.layout = layout)))
        });
        let corners = Corner::ALL.map(|corner| {
            let on = scene.corner == corner;
            let (right, bottom) = (
                matches!(corner, Corner::TopRight | Corner::BottomRight),
                matches!(corner, Corner::BottomLeft | Corner::BottomRight),
            );
            // A little screen, its camera in that corner.
            div()
                .id(("corner", corner as usize))
                .w(px(28.0))
                .h(px(18.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(if on { Theme::accent() } else { Theme::surface1() })
                .when(on, |d| d.bg(Theme::surface0()))
                .hover(|d| d.border_color(Theme::surface2()))
                .cursor_pointer()
                .relative()
                .child(
                    div()
                        .absolute()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(if on { Theme::accent() } else { Theme::overlay() })
                        .map(|d| if right { d.right(px(2.0)) } else { d.left(px(2.0)) })
                        .map(|d| if bottom { d.bottom(px(2.0)) } else { d.top(px(2.0)) }),
                )
                .tooltip(tip("Put the camera in this corner", None))
                .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.scene.corner = corner)))
        });
        let sizes = [("S", 0.22), ("M", 0.3), ("L", 0.4)].map(|(name, size)| {
            chip(
                ("bubble-size", (size * 100.0) as usize),
                name,
                (scene.size - size).abs() < 0.01,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.scene.size = size)))
        });
        let shapes = [(Bubble::Circle, "Round"), (Bubble::Square, "Square")].map(|(shape, name)| {
            chip(("bubble-shape", shape as usize), name, scene.shape == shape)
                .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.scene.shape = shape)))
        });
        let record = div()
            .id("studio-record")
            .h(px(34.0))
            .px(px(16.0))
            .rounded(px(9.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .when(!recording, |d| d.bg(Theme::accent()).hover(|s| s.opacity(0.9)))
            .when(recording, |d| {
                d.border_1().border_color(Theme::accent()).bg(gpui::rgba(0xef6a5533))
            })
            .child(
                div()
                    .size(px(10.0))
                    .when(recording, |d| d.rounded(px(2.0)).bg(Theme::accent()))
                    .when(!recording, |d| d.rounded_full().bg(Theme::ground())),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if recording { Theme::text() } else { Theme::ground() })
                    .child(if recording {
                        let s = self.rec.seconds() as u64;
                        format!("Stop  {}:{:02}", s / 60, s % 60)
                    } else {
                        "Record".to_string()
                    }),
            )
            .tooltip(tip(
                if recording {
                    "Stop, and lay the take on the timeline"
                } else {
                    "Record a take"
                },
                Some("⇧⌘R"),
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.rec.recording() {
                    this.stop_take(cx);
                } else if let Err(e) = this.start_take(Into::End, None, cx) {
                    this.say_error(e);
                }
            }));
        let paused = self.rec.paused();
        let label = |text: &'static str| div().text_size(px(11.0)).text_color(Theme::overlay()).child(text);
        div()
            .flex_none()
            .px(px(14.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(Theme::rule())
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .children(layouts)
                    .child(div().flex_1())
                    .when(recording && self.rec.pending.is_none(), |d| {
                        d.child(
                            chip("studio-pause", if paused { "Resume" } else { "Pause" }, paused)
                                .on_click(cx.listener(|this, _, _, cx| this.pause_take(cx))),
                        )
                    })
                    .child(record),
            )
            .when(scene.layout == Layout::Bubble, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .child(label("Corner"))
                        .children(corners)
                        .child(div().w(px(10.0)))
                        .child(label("Size"))
                        .children(sizes)
                        .child(div().w(px(10.0)))
                        .children(shapes),
                )
            })
    }

    fn sources_panel(&mut self, studio: &Studio, cx: &mut Context<Self>) -> impl IntoElement {
        let levels = self.rec.live.as_ref().map(|l| l.levels.clone()).unwrap_or_default();
        let devices = self.rec.devices.clone().unwrap_or_default();
        let open = self.rec.pick;
        let row = |id: &'static str,
                   glyph: &'static str,
                   label: &'static str,
                   value: String,
                   pick: Pick,
                   cx: &mut Context<Self>| {
            let on = open == Some(pick);
            div()
                .id(id)
                .h(px(34.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(px(9.0))
                .cursor_pointer()
                .when(on, |d| d.bg(Theme::surface0()))
                .hover(|d| d.bg(Theme::surface0()))
                .child(icon(glyph, Theme::subtext()).size(px(14.0)))
                .child(
                    div()
                        .w(px(64.0))
                        .text_size(px(11.5))
                        .text_color(Theme::overlay())
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.0))
                        .text_color(Theme::text())
                        .child(value),
                )
                .child(icon("chevron", Theme::overlay()).size(px(11.0)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rec.pick = if this.rec.pick == Some(pick) { None } else { Some(pick) };
                    // Something may have been plugged in since.
                    if this.rec.pick.is_some_and(|p| p != Pick::Screen) {
                        this.list_devices(cx);
                    }
                    cx.notify();
                }))
        };
        let mut list: Vec<AnyElement> = Vec::new();
        let screen_row = row(
            "pick-screen",
            "film",
            "Screen",
            screen_name(&studio.screen),
            Pick::Screen,
            cx,
        );
        let camera_row = row(
            "pick-camera",
            "camera",
            "Camera",
            studio.camera.as_ref().map_or("None".into(), |d| d.name.clone()),
            Pick::Camera,
            cx,
        );
        let mic_row = row(
            "pick-mic",
            "mic",
            "Mic",
            studio.microphone.as_ref().map_or("None".into(), |d| d.name.clone()),
            Pick::Microphone,
            cx,
        );
        let options = |pick: Pick, cx: &mut Context<Self>| -> Vec<AnyElement> {
            let option = |id: (&'static str, usize), label: String, chosen: bool| {
                div()
                    .id(id)
                    .h(px(28.0))
                    .pl(px(32.0))
                    .pr(px(8.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(Theme::surface0()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(if chosen { Theme::text() } else { Theme::subtext() })
                            .child(label),
                    )
                    .when(chosen, |d| d.child(icon("check", Theme::accent()).size(px(12.0))))
            };
            let mut out: Vec<AnyElement> = Vec::new();
            match pick {
                Pick::Screen => {
                    let mut choices: Vec<(String, Option<Screen>)> = vec![("None".into(), None)];
                    for d in recorder::displays() {
                        let name = if d.number == 1 {
                            "Main display".into()
                        } else {
                            format!("Display {}", d.number)
                        };
                        choices.push((
                            format!("{name} · {:.0}×{:.0}", d.width, d.height),
                            Some(Screen::Display { number: d.number }),
                        ));
                    }
                    for w in recorder::windows().into_iter().take(14) {
                        let label = if w.title.is_empty() {
                            w.app.clone()
                        } else {
                            format!("{} — {}", w.app, w.title)
                        };
                        choices.push((
                            label,
                            Some(Screen::Window {
                                app: w.app,
                                title: w.title,
                            }),
                        ));
                    }
                    for (i, (label, screen)) in choices.into_iter().enumerate() {
                        let chosen = studio.screen == screen;
                        out.push(
                            option(("screen-choice", i), label, chosen)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let screen = screen.clone();
                                    this.rec.pick = None;
                                    this.set_studio(cx, |s| s.screen = screen);
                                }))
                                .into_any_element(),
                        );
                    }
                }
                Pick::Camera | Pick::Microphone => {
                    let (there, current) = if pick == Pick::Camera {
                        (&devices.cameras, &studio.camera)
                    } else {
                        (&devices.microphones, &studio.microphone)
                    };
                    let mut choices: Vec<(String, Option<studio::Device>)> = vec![("None".into(), None)];
                    for d in there {
                        choices.push((
                            d.name.clone(),
                            Some(studio::Device {
                                id: d.id.clone(),
                                name: d.name.clone(),
                            }),
                        ));
                    }
                    for (i, (label, device)) in choices.into_iter().enumerate() {
                        let chosen = current.as_ref().map(|c| &c.id) == device.as_ref().map(|d| &d.id);
                        out.push(
                            option(("device-choice", i), label, chosen)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let device = device.clone();
                                    this.rec.pick = None;
                                    this.set_studio(cx, |s| {
                                        if pick == Pick::Camera {
                                            s.camera = device;
                                        } else {
                                            s.microphone = device;
                                        }
                                    });
                                }))
                                .into_any_element(),
                        );
                    }
                    if there.is_empty() {
                        out.push(
                            div()
                                .pl(px(32.0))
                                .text_size(px(11.0))
                                .text_color(Theme::overlay())
                                .child("None connected")
                                .into_any_element(),
                        );
                    }
                }
            }
            out
        };
        list.push(screen_row.into_any_element());
        if open == Some(Pick::Screen) {
            list.extend(options(Pick::Screen, cx));
        }
        list.push(camera_row.into_any_element());
        if open == Some(Pick::Camera) {
            list.extend(options(Pick::Camera, cx));
        }
        list.push(
            div()
                .flex()
                .flex_col()
                .child(mic_row)
                .when(studio.microphone.is_some(), |d| {
                    d.child(meter(levels.get("microphone").copied()))
                })
                .into_any_element(),
        );
        if open == Some(Pick::Microphone) {
            list.extend(options(Pick::Microphone, cx));
        }
        let system = studio.system_audio;
        list.push(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("system-audio")
                        .h(px(34.0))
                        .px(px(8.0))
                        .rounded(px(7.0))
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .cursor_pointer()
                        .hover(|d| d.bg(Theme::surface0()))
                        .child(icon("volume", Theme::subtext()).size(px(14.0)))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.0))
                                .text_color(Theme::text())
                                .child("Sound the Mac plays"),
                        )
                        .child(switch(system))
                        .on_click(
                            cx.listener(|this, _, _, cx| this.set_studio(cx, |s| s.system_audio = !s.system_audio)),
                        ),
                )
                .when(system, |d| d.child(meter(levels.get("system").copied())))
                .into_any_element(),
        );
        let access = self.rec.devices.as_ref().map(|d| {
            let mut missing = Vec::new();
            if studio.screen.is_some() && !d.screen_access {
                missing.push("Screen Recording");
            }
            if studio.camera.is_some() && d.camera_access == "denied" {
                missing.push("Camera");
            }
            if studio.microphone.is_some() && d.microphone_access == "denied" {
                missing.push("Microphone");
            }
            missing
        });
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(caps("Sources").mb(px(6.0)))
            .children(list)
            .when_some(access.filter(|m| !m.is_empty()), |d, missing| {
                d.child(
                    div()
                        .mt(px(8.0))
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(Theme::accent())
                        .child(format!(
                            "Turn on {} for Splice in System Settings → Privacy & Security.",
                            missing.join(" and ")
                        )),
                )
            })
    }

    fn output_panel(&mut self, studio: &Studio, cx: &mut Context<Self>) -> impl IntoElement {
        let out = studio.output;
        let line = |label: &'static str, chips: Vec<AnyElement>| {
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(div().text_size(px(11.0)).text_color(Theme::overlay()).child(label))
                .child(div().flex().flex_wrap().gap(px(4.0)).children(chips))
        };
        let codecs = [(Codec::H264, "H.264"), (Codec::Hevc, "HEVC")]
            .map(|(codec, name)| {
                chip(("codec", codec as usize), name, out.codec == codec)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.output.codec = codec)))
                    .into_any_element()
            })
            .into_iter()
            .collect();
        let qualities = Quality::ALL
            .map(|q| {
                chip(("quality", q as usize), q.name(), out.quality == q)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.output.quality = q)))
                    .into_any_element()
            })
            .into_iter()
            .collect();
        let rates = [30u32, 60]
            .map(|fps| {
                chip(("fps", fps as usize), &format!("{fps} fps"), out.fps == fps)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.output.fps = fps)))
                    .into_any_element()
            })
            .into_iter()
            .collect();
        let heights = studio::HEIGHTS
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let h = *h;
                chip(("height", i), &studio::height_name(h), out.height == h)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_studio(cx, |s| s.output.height = h)))
                    .into_any_element()
            })
            .collect();
        let toggles = vec![
            chip("cursor", "Pointer", out.cursor)
                .on_click(cx.listener(|this, _, _, cx| this.set_studio(cx, |s| s.output.cursor = !s.output.cursor)))
                .into_any_element(),
            chip("clicks", "Note clicks", out.clicks)
                .tooltip(tip("Note where you click, to zoom in on later", None))
                .on_click(cx.listener(|this, _, _, cx| this.set_studio(cx, |s| s.output.clicks = !s.output.clicks)))
                .into_any_element(),
        ];
        let is_default = self.rec.default.as_ref() == Some(studio);
        let last = self.rec.last.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(caps("Files"))
            .child(line("Codec", codecs))
            .child(line("Quality", qualities))
            .child(line("Frame rate", rates))
            .child(line("Screen size", heights))
            .child(line("Screen", toggles))
            .child(
                div()
                    .text_size(px(11.0))
                    .line_height(px(15.0))
                    .text_color(Theme::overlay())
                    .child("Each source is its own file: screen.mp4, camera.mp4, microphone.m4a, system.m4a."),
            )
            .child(
                chip(
                    "make-default",
                    if is_default {
                        "This is your default"
                    } else {
                        "Make this my default"
                    },
                    is_default,
                )
                .tooltip(tip("New projects record with this setup", None))
                .on_click(cx.listener(|this, _, _, cx| {
                    let studio = this.studio();
                    match save_default(&studio) {
                        Ok(()) => {
                            this.rec.default = Some(studio);
                            this.say("New projects will record with this setup");
                        }
                        Err(e) => this.say_error(format!("Could not save the default: {e}")),
                    }
                    cx.notify();
                })),
            )
            .when_some(last, |d, last| {
                d.child(
                    chip("show-take", "Show the last take's files", false)
                        .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&last))),
                )
            })
    }

    // `splice ctl record …`.

    pub(super) fn ctl_record(
        &mut self,
        request: &Value,
        reply: Sender<String>,
        cx: &mut Context<Self>,
    ) -> Option<Value> {
        use crate::ctl::fail;
        let sub = request.get("sub").and_then(Value::as_str).unwrap_or("status");
        match sub {
            "sources" => {
                let devices = match capture::devices() {
                    Ok(d) => d,
                    Err(e) => return Some(fail(e.to_string())),
                };
                Some(json!({
                    "ok": true,
                    "displays": recorder::displays(),
                    "windows": recorder::windows(),
                    "cameras": devices.cameras,
                    "microphones": devices.microphones,
                    "access": {
                        "screen": devices.screen_access,
                        "camera": devices.camera_access,
                        "microphone": devices.microphone_access,
                    },
                }))
            }
            "setup" => {
                if let Err(e) = self.apply_setup(request, cx) {
                    return Some(fail(e));
                }
                if request.get("default").and_then(Value::as_bool) == Some(true) {
                    let studio = self.studio();
                    if let Err(e) = save_default(&studio) {
                        return Some(fail(format!("could not save the default: {e}")));
                    }
                    self.rec.default = Some(studio);
                }
                Some(json!({
                    "ok": true,
                    "setup": self.studio(),
                    "default": self.rec.default.as_ref() == Some(&self.studio()),
                    "own": self.doc.project.studio.is_some(),
                }))
            }
            "status" => Some(json!({
                "ok": true,
                "recording": self.rec.recording(),
                "paused": self.rec.paused(),
                "seconds": self.rec.seconds(),
                "live": self.rec.live.as_ref().is_some_and(|l| l.live),
                "levels": self.rec.live.as_ref().map(|l| l.levels.clone()),
                "problem": self.rec.problem,
                "asking": self.rec.live.as_ref().and_then(|l| l.asking.clone()),
                "last": self.rec.last,
            })),
            "start" => {
                if let Err(e) = self.apply_setup(request, cx) {
                    return Some(fail(e));
                }
                let into = if let Some(beat) = request.get("beat").and_then(Value::as_u64) {
                    let id = BeatId(beat);
                    if self.doc.project.story.index(id).is_none() {
                        return Some(fail(format!("no beat {beat}; `splice ctl story read` lists them")));
                    }
                    Into::Beat(id)
                } else if request.get("into").and_then(Value::as_str) == Some("media") {
                    Into::Media
                } else {
                    match self.time(request, "at") {
                        Ok(Some(t)) => Into::At(t, self.track(request).unwrap_or(0)),
                        Ok(None) => Into::End,
                        Err(e) => return Some(fail(e)),
                    }
                };
                match self.start_take(into, Some(reply), cx) {
                    Ok(()) => {
                        if let Some(s) = request.get("for").and_then(Value::as_f64) {
                            cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(Duration::from_secs_f64(s.max(0.5) + 0.3))
                                    .await;
                                this.update(cx, |editor, cx| editor.stop_take(cx)).ok();
                            })
                            .detach();
                        }
                        None
                    }
                    Err(e) => Some(fail(e)),
                }
            }
            "pause" | "resume" => {
                if !self.rec.recording() {
                    return Some(fail("not recording"));
                }
                if (sub == "pause") != self.rec.paused() {
                    self.pause_take(cx);
                }
                Some(json!({ "ok": true }))
            }
            "stop" => {
                if !self.rec.recording() {
                    return Some(fail("not recording"));
                }
                let wait = request.get("wait").and_then(Value::as_bool) == Some(true);
                if wait {
                    self.rec.waiters.push(reply);
                }
                self.stop_take(cx);
                if wait {
                    None
                } else {
                    Some(json!({ "ok": true, "stopping": true }))
                }
            }
            other => Some(fail(format!(
                "unknown `record {other}`: use sources, setup, start, pause, resume, stop or status"
            ))),
        }
    }

    /// Changes the setup from `record setup` or `record start` flags.
    fn apply_setup(&mut self, request: &Value, cx: &mut Context<Self>) -> Result<(), String> {
        let text = |k: &str| request.get(k).and_then(Value::as_str).map(str::to_string);
        let flag = |k: &str| request.get(k).and_then(Value::as_bool);
        let mut studio = self.studio();
        let before = studio.clone();
        if let Some(screen) = text("screen") {
            studio.screen = parse_screen(&screen)?;
        } else if let Some(n) = request.get("display").and_then(Value::as_u64) {
            studio.screen = Some(Screen::Display { number: n as u32 });
        } else if let Some(w) = request.get("window") {
            let windows = recorder::windows();
            let found = match w {
                Value::Number(n) => windows.iter().find(|x| Some(x.id as u64) == n.as_u64()),
                Value::String(name) => {
                    let lower = name.to_lowercase();
                    windows
                        .iter()
                        .find(|x| x.app.to_lowercase().contains(&lower) || x.title.to_lowercase().contains(&lower))
                }
                _ => None,
            };
            let w = found.ok_or("no such window; `splice ctl record sources` lists them")?;
            studio.screen = Some(Screen::Window {
                app: w.app.clone(),
                title: w.title.clone(),
            });
        }
        if let Some(region) = text("region") {
            let n: Vec<f64> = region.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            let [x, y, width, height] = n[..] else {
                return Err("--region is X,Y,WIDTH,HEIGHT in points from the display's top left".into());
            };
            let display = match &studio.screen {
                Some(Screen::Display { number }) => *number,
                _ => 1,
            };
            studio.screen = Some(Screen::Region {
                display,
                x,
                y,
                width,
                height,
            });
        }
        let need_devices = text("camera").is_some() || text("microphone").is_some() || flag("mic") == Some(true);
        let devices = if need_devices {
            Some(capture::devices().map_err(|e| e.to_string())?)
        } else {
            None
        };
        let choose = |name: &str, there: &[capture::Device], what: &str| -> Result<Option<studio::Device>, String> {
            if matches!(name, "none" | "off" | "") {
                return Ok(None);
            }
            let lower = name.to_lowercase();
            let found = there
                .iter()
                .find(|d| d.id == name)
                .or_else(|| there.iter().find(|d| d.name.to_lowercase().contains(&lower)))
                .or_else(|| there.iter().find(|d| d.default).filter(|_| lower == "default"));
            let d = found.ok_or(format!(
                "no {what} like “{name}”; `splice ctl record sources` lists them"
            ))?;
            Ok(Some(studio::Device {
                id: d.id.clone(),
                name: d.name.clone(),
            }))
        };
        if let (Some(name), Some(d)) = (text("camera"), &devices) {
            studio.camera = choose(&name, &d.cameras, "camera")?;
        }
        if let Some(d) = &devices {
            if let Some(name) = text("microphone") {
                studio.microphone = choose(&name, &d.microphones, "microphone")?;
            } else if flag("mic") == Some(true) && studio.microphone.is_none() {
                studio.microphone = choose("default", &d.microphones, "microphone")?;
            }
        }
        if let Some(on) = flag("system") {
            studio.system_audio = on;
        }
        if let Some(layout) = text("layout") {
            studio.scene.layout = Layout::parse(&layout).ok_or("--layout is screen, bubble, side or camera")?;
        }
        if let Some(corner) = text("corner") {
            studio.scene.corner = Corner::parse(&corner).ok_or("--corner is tl, tr, bl or br")?;
        }
        if let Some(size) = request.get("size").and_then(Value::as_f64) {
            studio.scene.size = (size as f32).clamp(0.08, 0.9);
        }
        if let Some(split) = request.get("split").and_then(Value::as_f64) {
            studio.scene.split = (split as f32).clamp(0.15, 0.6);
        }
        if let Some(shape) = text("shape") {
            studio.scene.shape = match shape.as_str() {
                "circle" | "round" => Bubble::Circle,
                "square" => Bubble::Square,
                _ => return Err("--shape is circle or square".into()),
            };
        }
        if let Some(codec) = text("codec") {
            studio.output.codec = match codec.to_lowercase().as_str() {
                "h264" | "h.264" | "avc" => Codec::H264,
                "hevc" | "h265" | "h.265" => Codec::Hevc,
                _ => return Err("--codec is h264 or hevc".into()),
            };
        }
        if let Some(q) = text("quality") {
            studio.output.quality = Quality::ALL
                .into_iter()
                .find(|x| x.name().eq_ignore_ascii_case(&q))
                .ok_or("--quality is good, high or best")?;
        }
        if let Some(fps) = request.get("fps").and_then(Value::as_u64) {
            studio.output.fps = (fps as u32).clamp(1, 120);
        }
        if let Some(h) = text("height") {
            studio.output.height = match h.to_lowercase().trim_end_matches('p') {
                "full" | "native" | "none" => None,
                "4k" => Some(2160),
                n => Some(n.parse().map_err(|_| "--height is full, 4k, 1440, 1080 or a number")?),
            };
        }
        if let Some(on) = flag("cursor") {
            studio.output.cursor = on;
        }
        if let Some(on) = flag("clicks") {
            studio.output.clicks = on;
        }
        if studio != before || self.doc.project.studio.is_none() && request.get("default").is_some() {
            self.edit(cx, |p| p.studio = Some(studio));
        }
        // Also brings back sources that stopped.
        self.relive(cx);
        Ok(())
    }
}

/// `--screen`: display:N (or N), window:APP, none.
fn parse_screen(s: &str) -> Result<Option<Screen>, String> {
    let (kind, rest) = s.split_once(':').unwrap_or((s, ""));
    match kind.to_lowercase().as_str() {
        "none" | "off" => Ok(None),
        "main" => Ok(Some(Screen::Display { number: 1 })),
        "display" => Ok(Some(Screen::Display {
            number: rest.parse().unwrap_or(1).max(1),
        })),
        "window" => {
            let lower = rest.to_lowercase();
            let w = recorder::windows()
                .into_iter()
                .find(|w| w.app.to_lowercase().contains(&lower) || w.title.to_lowercase().contains(&lower))
                .ok_or(format!(
                    "no window like “{rest}”; `splice ctl record sources` lists them"
                ))?;
            Ok(Some(Screen::Window {
                app: w.app,
                title: w.title,
            }))
        }
        n => n
            .parse::<u32>()
            .map(|number| Some(Screen::Display { number: number.max(1) }))
            .map_err(|_| "--screen is display:N, window:APP or none".to_string()),
    }
}

fn name_of(feed: &str) -> &'static str {
    match feed {
        "screen" => "Screen",
        "camera" => "Camera",
        "microphone" => "Mic",
        _ => "Mac sound",
    }
}

/// "Take 2026-10-28 14.03.12" from a file in its folder.
fn take_name(path: &Path) -> String {
    path.parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().trim_start_matches("Take ").to_string())
        .unwrap_or_default()
}

/// A live picture where `look` puts it on a stage of `stage` points.
fn picture(image: Arc<RenderImage>, natural: (u32, u32), look: &Look, stage: (f32, f32)) -> AnyElement {
    let (sw, sh) = stage;
    let (nw, nh) = (natural.0.max(1) as f32, natural.1.max(1) as f32);
    let fit = (sw / nw).min(sh / nh) * look.scale;
    let (pw, ph) = (nw * fit, nh * fit);
    let (cx, cy) = (sw / 2.0 + look.x * sw, sh / 2.0 + look.y * sh);
    let (pl, pt) = (cx - pw / 2.0, cy - ph / 2.0);
    // What shows of it: all, a mask's shape, or what a crop leaves.
    let (mut l, mut t, mut r, mut b) = (0.0, 0.0, 1.0, 1.0);
    let mut round = false;
    if let Some(m) = look.mask {
        (l, t, r, b) = (
            m.x - m.width / 2.0,
            m.y - m.height / 2.0,
            m.x + m.width / 2.0,
            m.y + m.height / 2.0,
        );
        round = m.shape == splice_core::look::Shape::Ellipse;
    } else if let Some(c) = look.crop {
        (l, t, r, b) = (c.left, c.top, 1.0 - c.right, 1.0 - c.bottom);
    }
    let (vl, vt, vw, vh) = (pl + l * pw, pt + t * ph, (r - l) * pw, (b - t) * ph);
    div()
        .absolute()
        .left(px(vl))
        .top(px(vt))
        .w(px(vw))
        .h(px(vh))
        .overflow_hidden()
        .when(round, |d| d.rounded_full())
        .child(
            img(image)
                .absolute()
                .left(px(pl - vl))
                .top(px(pt - vt))
                .w(px(pw))
                .h(px(ph))
                .object_fit(ObjectFit::Fill),
        )
        .into_any_element()
}

/// A small choice that is on or off.
fn chip(id: impl std::convert::Into<gpui::ElementId>, label: &str, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(24.0))
        .px(px(9.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .cursor_pointer()
        .border_1()
        .border_color(if on { Theme::surface2() } else { Theme::edge() })
        .when(on, |d| d.bg(Theme::surface0()))
        .hover(|d| d.bg(Theme::surface0()))
        .text_size(px(11.5))
        .text_color(if on { Theme::text() } else { Theme::subtext() })
        .child(label.to_string())
}

fn switch(on: bool) -> impl IntoElement {
    div()
        .w(px(26.0))
        .h(px(15.0))
        .rounded_full()
        .p(px(2.0))
        .flex()
        .when(on, |d| d.justify_end().bg(Theme::accent()))
        .when(!on, |d| d.bg(Theme::surface1()))
        .child(div().size(px(11.0)).rounded_full().bg(Theme::ground()))
}

/// How loud a sound is: a bar, green to the accent near the top.
fn meter(level: Option<f32>) -> impl IntoElement {
    let level = level.unwrap_or(0.0).clamp(0.0, 1.0);
    // Shown in decibels, so speech sits around the middle.
    let shown = if level <= 0.0 {
        0.0
    } else {
        ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    };
    div()
        .mx(px(8.0))
        .mb(px(4.0))
        .h(px(4.0))
        .rounded_full()
        .bg(Theme::surface0())
        .overflow_hidden()
        .child(div().h_full().w(relative(shown)).rounded_full().bg(if level > 0.89 {
            Theme::accent()
        } else {
            gpui::rgb(0x7fb069)
        }))
}
