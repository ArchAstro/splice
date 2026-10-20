//! Playback and scrubbing. The player owns the playhead and, for each video
//! clip under it, the frame on screen. Playing runs one ffmpeg stream per
//! visible clip (and opens streams just before clips begin); paused, it
//! fetches single accurate stills for wherever the playhead lands.

use crate::ffmpeg::{self, FrameStream};
use crate::library::Library;
use gpui::RenderImage;
use splice_core::model::{ClipId, Media, MediaId, MediaKind, Project, Source};
use splice_core::time::{SECOND, Time, floor_to_frame};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// How far ahead streams are opened for clips about to start.
const LOOKAHEAD: Time = SECOND * 7 / 10;
/// How long playback waits for first frames before starting the clock anyway.
const PRIME_TIMEOUT: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Paused,
    /// Streams are opening; the clock starts once they have frames.
    Priming(Instant),
    Playing {
        anchor: Instant,
        from: Time,
    },
}

struct Shown {
    image: Arc<RenderImage>,
    media: MediaId,
    /// The key and mask filters the frame went through.
    filters: Option<String>,
    source: Time,
    /// Decoded size, so a still is fetched again when the viewer grows.
    size: (u32, u32),
}

struct Stream {
    media: MediaId,
    filters: Option<String>,
    size: (u32, u32),
    decoder: FrameStream,
    /// Decoded, not yet due.
    pending: Option<(Time, Arc<RenderImage>)>,
}

struct StillResult {
    clip: ClipId,
    media: MediaId,
    filters: Option<String>,
    source: Time,
    size: (u32, u32),
    image: Option<Arc<RenderImage>>,
}

/// What the editor should do after a tick.
#[derive(Default)]
pub struct Tick {
    /// Playback just got its frames: start the sound now.
    pub started: bool,
    /// Playback reached the end.
    pub ended: bool,
}

pub struct Player {
    pub position: Time,
    pub state: State,
    /// The largest frame size worth decoding (the viewer's size in device pixels).
    pub preview: (u32, u32),
    streams: HashMap<ClipId, Stream>,
    shown: HashMap<ClipId, Shown>,
    stills: HashSet<ClipId>,
    /// Stills that could not be decoded (past the last frame, say); not retried.
    failed: HashMap<ClipId, Time>,
    still_tx: Sender<StillResult>,
    still_rx: Receiver<StillResult>,
    retired: Vec<Arc<RenderImage>>,
}

impl Player {
    pub fn new() -> Player {
        let (still_tx, still_rx) = channel();
        Player {
            position: 0,
            state: State::Paused,
            preview: (1280, 720),
            streams: HashMap::new(),
            shown: HashMap::new(),
            stills: HashSet::new(),
            failed: HashMap::new(),
            still_tx,
            still_rx,
            retired: Vec::new(),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state != State::Paused
    }

    /// Whether frames are still on their way, so the view should keep ticking.
    pub fn busy(&self) -> bool {
        self.is_playing() || !self.stills.is_empty()
    }

    pub fn play(&mut self, project: &Project) {
        if self.position >= project.duration() {
            self.position = 0;
        }
        self.state = State::Priming(Instant::now());
    }

    pub fn pause(&mut self) {
        self.state = State::Paused;
        self.streams.clear();
    }

    /// Moves the playhead. While playing, the clock restarts from here.
    pub fn seek(&mut self, t: Time) {
        self.position = t.max(0);
        if self.is_playing() {
            self.state = State::Priming(Instant::now());
        }
    }

    /// The image to draw for a media clip at the playhead. Pictures with
    /// no key or mask come straight from the library; the rest are decoded.
    pub fn frame<'a>(
        &'a self,
        clip: ClipId,
        media: &Media,
        filters: Option<&str>,
        library: &'a Library,
    ) -> Option<&'a Arc<RenderImage>> {
        if media.kind == MediaKind::Image && filters.is_none() {
            return library.get(&media.hash).and_then(|a| a.still.as_ref());
        }
        self.shown.get(&clip).map(|s| &s.image)
    }

    /// Images no longer on screen; the window should release them.
    pub fn take_retired(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.retired)
    }

    /// Hands images to the window for release on the next frame.
    pub fn retire(&mut self, images: Vec<Arc<RenderImage>>) {
        self.retired.extend(images);
    }

    /// Drops everything decoded, e.g. when another project opens.
    pub fn reset(&mut self) {
        self.pause();
        self.position = 0;
        for (_, shown) in self.shown.drain() {
            self.retired.push(shown.image);
        }
    }

    pub fn tick(&mut self, project: &Project, library: &Library) -> Tick {
        let mut tick = Tick::default();
        let now = Instant::now();
        let fps = project.settings.fps;
        let half_frame = project.frame() / 2;

        while let Ok(result) = self.still_rx.try_recv() {
            self.stills.remove(&result.clip);
            match result.image {
                Some(image) => self.show(
                    result.clip,
                    result.media,
                    result.filters,
                    result.source,
                    result.size,
                    image,
                ),
                None => {
                    self.failed.insert(result.clip, result.source);
                }
            }
        }

        if let State::Playing { anchor, from } = self.state {
            self.position = from + (now - anchor).as_micros() as Time;
            let end = project.duration();
            if self.position >= end {
                self.position = end;
                self.pause();
                tick.ended = true;
            }
        }
        let playing = self.is_playing();
        let position = self.position;

        // Media that needs decoding: video, and pictures with a key or mask.
        let stack: Vec<_> = project
            .visual_stack(position)
            .into_iter()
            .filter_map(|(c, source)| match source {
                Source::Media(m) => {
                    let filters = c.look.pixel_filters();
                    (m.kind == MediaKind::Video || filters.is_some()).then(|| (c.clone(), m.clone(), filters))
                }
                Source::Generated(_) => None,
            })
            .collect();
        let visible: HashSet<ClipId> = stack.iter().map(|(c, _, _)| c.id).collect();
        let mut keep_streams = HashSet::new();

        for (clip, media, filters) in &stack {
            let source = if media.kind == MediaKind::Image {
                0
            } else {
                floor_to_frame(clip.source_at(position), fps)
            };
            if playing && media.kind == MediaKind::Video {
                keep_streams.insert(clip.id);
                let Some(path) = library.path(&media.hash) else {
                    continue;
                };
                self.ensure_stream(
                    clip.id,
                    media.id,
                    filters,
                    path,
                    source,
                    fps,
                    (media.width, media.height),
                );
                if let Some((t, size, image)) = self.pull(clip.id, source + half_frame) {
                    self.show(clip.id, media.id, filters.clone(), t, size, image);
                }
            } else {
                let size = self.decode_size((media.width, media.height));
                let current = self
                    .shown
                    .get(&clip.id)
                    .filter(|s| s.media == media.id && s.filters == *filters)
                    .map(|s| (s.source, s.size));
                let failed = self.failed.get(&clip.id) == Some(&source);
                if current != Some((source, size)) && !failed && !self.stills.contains(&clip.id) {
                    let Some(path) = library.path(&media.hash) else {
                        continue;
                    };
                    self.request_still(
                        clip.id,
                        media.id,
                        filters.clone(),
                        path.to_path_buf(),
                        source,
                        (media.width, media.height),
                    );
                }
            }
        }

        if playing {
            // Open streams for clips about to begin so they are ready on time.
            for (_, clip) in project.clips() {
                if clip.start > position && clip.start <= position + LOOKAHEAD {
                    let Some(media) = project.media(clip.media).filter(|m| m.kind == MediaKind::Video) else {
                        continue;
                    };
                    if project.slot(clip.id).is_some_and(|s| project.tracks[s.track].hidden) {
                        continue;
                    }
                    let Some(path) = library.path(&media.hash) else {
                        continue;
                    };
                    keep_streams.insert(clip.id);
                    let filters = clip.look.pixel_filters();
                    self.ensure_stream(
                        clip.id,
                        media.id,
                        &filters,
                        path,
                        clip.source_in,
                        fps,
                        (media.width, media.height),
                    );
                }
            }
        }
        self.streams.retain(|id, _| keep_streams.contains(id));
        let gone: Vec<ClipId> = self.shown.keys().filter(|id| !visible.contains(id)).copied().collect();
        for id in gone {
            if let Some(shown) = self.shown.remove(&id) {
                self.retired.push(shown.image);
            }
        }

        if let State::Priming(since) = self.state {
            let ready = stack.iter().all(|(c, _, _)| self.shown.contains_key(&c.id));
            if ready || now - since > PRIME_TIMEOUT {
                self.state = State::Playing {
                    anchor: now,
                    from: position,
                };
                tick.started = true;
            }
        }
        tick
    }

    fn show(
        &mut self,
        clip: ClipId,
        media: MediaId,
        filters: Option<String>,
        source: Time,
        size: (u32, u32),
        image: Arc<RenderImage>,
    ) {
        if let Some(old) = self.shown.insert(
            clip,
            Shown {
                image,
                media,
                filters,
                source,
                size,
            },
        ) {
            self.retired.push(old.image);
        }
    }

    fn decode_size(&self, media: (u32, u32)) -> (u32, u32) {
        ffmpeg::fit(media.0, media.1, self.preview.0.max(64), self.preview.1.max(64))
    }

    /// Keeps a stream for `clip` that will reach `source` soon, opening one if not.
    #[allow(clippy::too_many_arguments)]
    fn ensure_stream(
        &mut self,
        clip: ClipId,
        media: MediaId,
        filters: &Option<String>,
        path: &std::path::Path,
        source: Time,
        fps: u32,
        size: (u32, u32),
    ) {
        if let Some(stream) = self.streams.get(&clip) {
            let next = stream.pending.as_ref().map_or(stream.decoder.next_time(), |(t, _)| *t);
            let usable = stream.media == media
                && stream.filters == *filters
                && next <= source + SECOND / 2
                && next >= source - SECOND;
            if usable {
                return;
            }
        }
        let (w, h) = self.decode_size(size);
        match FrameStream::open(path, source, fps, w, h, filters.as_deref()) {
            Ok(decoder) => {
                self.streams.insert(
                    clip,
                    Stream {
                        media,
                        filters: filters.clone(),
                        size: (w, h),
                        decoder,
                        pending: None,
                    },
                );
            }
            Err(e) => eprintln!("splice: cannot decode: {e}"),
        }
    }

    /// The newest decoded frame due by `due`, skipping late ones.
    fn pull(&mut self, clip: ClipId, due: Time) -> Option<(Time, (u32, u32), Arc<RenderImage>)> {
        let stream = self.streams.get_mut(&clip)?;
        let size = stream.size;
        let mut latest = None;
        loop {
            if stream.pending.is_none() {
                let t = stream.decoder.next_time();
                stream.pending = stream.decoder.try_next().map(|image| (t, image));
            }
            match stream.pending.take() {
                Some((t, image)) if t <= due => latest = Some((t, image)),
                Some(early) => {
                    stream.pending = Some(early);
                    break;
                }
                None => break,
            }
        }
        latest.map(|(t, image)| (t, size, image))
    }

    fn request_still(
        &mut self,
        clip: ClipId,
        media: MediaId,
        filters: Option<String>,
        path: std::path::PathBuf,
        source: Time,
        size: (u32, u32),
    ) {
        self.stills.insert(clip);
        let (w, h) = self.decode_size(size);
        let tx = self.still_tx.clone();
        std::thread::spawn(move || {
            let image = ffmpeg::still(&path, source, w, h, filters.as_deref()).ok();
            let _ = tx.send(StillResult {
                clip,
                media,
                filters,
                source,
                size: (w, h),
                image,
            });
        });
    }
}
