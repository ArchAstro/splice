//! Playback and scrubbing. The player owns the playhead and, for each video
//! clip under it, the frame on screen. Playing runs one ffmpeg stream per
//! visible clip (and opens streams just before clips begin); paused, it
//! fetches single accurate stills for wherever the playhead lands.

use crate::ffmpeg::{self, FrameStream};
use crate::library::Library;
use gpui::RenderImage;
use splice_core::compose;
use splice_core::model::{Clip, ClipId, Media, MediaId, MediaKind, Project, Source};
use splice_core::time::{self, SECOND, Time, floor_to_frame};
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
    /// Source seconds a second, and which way.
    speed: f64,
    reverse: bool,
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

/// A clip under the playhead whose frames need decoding.
struct Wanted {
    clip: Clip,
    media: Media,
    filters: Option<String>,
    /// The source time it shows.
    source: Time,
    /// The size its frames are decoded at.
    size: (u32, u32),
}

/// How big frames decoded at `size` are against the media's own.
fn scale_of(size: (u32, u32), media: &Media) -> f32 {
    size.0 as f32 / media.width.max(1) as f32
}

/// A backdrop's filters: what the clip hides stays hidden, then the blur.
fn backdrop_filters(clip: &Clip, size: (u32, u32)) -> String {
    let (mut filters, after) = clip.look.hiding_filters(size, "b");
    filters.push(clip.look.backdrop.unwrap_or_default().ffmpeg(after));
    filters.push("format=rgba".into());
    filters.join(",")
}

/// A backdrop's decoded size: small, since it is blurred and scaled up anyway.
pub fn backdrop_size(full: (u32, u32)) -> (u32, u32) {
    ((full.0 / 4).max(16) & !1, (full.1 / 4).max(16) & !1)
}

/// How playback has kept up since it last started: for `ctl status`, and for
/// telling a heavy stack of effects from a smooth one.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Keeping {
    /// Seconds played.
    pub seconds: f64,
    /// New frames drawn, all clips together.
    pub frames: u64,
    /// Decoded frames passed over because a later one was already due.
    pub skipped: u64,
    /// Looks at the clock that found a clip's picture over two frames behind.
    pub behind: u64,
    pub looks: u64,
    /// The furthest any clip's picture fell behind, in seconds.
    pub worst: f64,
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
    /// Clips whose stream has given a frame since playback was asked for:
    /// the clock waits for them, not for the stills shown while paused.
    flowing: HashSet<ClipId>,
    pub keeping: Keeping,
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
            flowing: HashSet::new(),
            keeping: Keeping::default(),
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
        self.flowing.clear();
        self.keeping = Keeping::default();
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
            self.flowing.clear();
        }
    }

    /// The image to draw for a media clip at the playhead. Pictures whose
    /// pixels nothing changes (`filtered`) come straight from the library;
    /// the rest are decoded.
    pub fn frame<'a>(
        &'a self,
        clip: ClipId,
        media: &Media,
        filtered: bool,
        library: &'a Library,
    ) -> Option<&'a Arc<RenderImage>> {
        if media.kind == MediaKind::Image && !filtered {
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

    /// Drops what was decoded from `media`, whose file changed.
    pub fn forget(&mut self, media: MediaId) {
        self.streams.retain(|_, s| s.media != media);
        let gone: Vec<ClipId> = self
            .shown
            .iter()
            .filter(|(_, s)| s.media == media)
            .map(|(c, _)| *c)
            .collect();
        for clip in gone {
            if let Some(shown) = self.shown.remove(&clip) {
                self.retired.push(shown.image);
            }
            self.failed.remove(&clip);
        }
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

        // Media that needs decoding: video, and pictures with a key, mask or
        // colour; each with the source time it shows now (held inside the
        // media where a transition draws it past its ends).
        let frame = project.frame();
        let shown_at = |clip: &Clip, m: &Media| {
            if m.kind == MediaKind::Video {
                floor_to_frame(clip.source_at(position).clamp(0, (m.duration - frame).max(0)), fps)
            } else {
                0
            }
        };
        let stack: Vec<Wanted> = project
            .picture(position)
            .into_iter()
            .filter_map(|shown| match shown {
                compose::Shown::Clip { drawn, .. } => match drawn.source {
                    Source::Media(m) => {
                        let clip = drawn.clip;
                        let size = self.decode_size((m.width, m.height));
                        let filters = clip.look.pixel_filters(size, scale_of(size, m), "p");
                        let video = m.kind == MediaKind::Video;
                        (video || filters.is_some()).then(|| Wanted {
                            clip: clip.clone(),
                            media: m.clone(),
                            filters,
                            source: shown_at(clip, m),
                            size,
                        })
                    }
                    Source::Generated(_) => None,
                },
                // A backdrop: the same media, small (it is blurred anyway),
                // decoded as a clip of its own.
                compose::Shown::Backdrop {
                    clip,
                    source: Source::Media(m),
                    ..
                } => {
                    let size = backdrop_size(self.decode_size((m.width, m.height)));
                    Some(Wanted {
                        clip: Clip {
                            id: compose::backdrop_id(clip.id),
                            ..clip.clone()
                        },
                        media: m.clone(),
                        filters: Some(backdrop_filters(clip, size)),
                        source: shown_at(clip, m),
                        size,
                    })
                }
                compose::Shown::Backdrop { .. } | compose::Shown::Fill { .. } => None,
            })
            .collect();
        let visible: HashSet<ClipId> = stack.iter().map(|w| w.clip.id).collect();
        let mut keep_streams = HashSet::new();

        for Wanted {
            clip,
            media,
            filters,
            source,
            size,
        } in &stack
        {
            let size = *size;
            let source = *source;
            // A held frame is a still, playing or not.
            if playing && media.kind == MediaKind::Video && !clip.frozen {
                keep_streams.insert(clip.id);
                let Some(path) = library.path(&media.hash) else {
                    continue;
                };
                self.ensure_stream(clip, media, filters, path, source, fps, size);
                // Half a frame of playback, in source time.
                let half = (half_frame as f64 * clip.speed) as Time;
                let due = if clip.reverse { source - half } else { source + half };
                if let Some((t, size, image)) = self.pull(clip.id, due) {
                    self.show(clip.id, media.id, filters.clone(), t, size, image);
                    self.flowing.insert(clip.id);
                    self.keeping.frames += 1;
                }
                if let State::Playing { from, .. } = self.state {
                    self.keeping.seconds = time::secs(position - from);
                    self.keeping.looks += 1;
                    if let Some(shown) = self.shown.get(&clip.id) {
                        let gap = if clip.reverse {
                            shown.source - due
                        } else {
                            due - shown.source
                        };
                        let late = time::secs(gap) / clip.speed.max(0.01);
                        if late > 2.0 * time::secs(frame) {
                            self.keeping.behind += 1;
                        }
                        self.keeping.worst = self.keeping.worst.max(late);
                    }
                }
            } else {
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
                    self.request_still(clip.id, media.id, filters.clone(), path.to_path_buf(), source, size);
                }
            }
        }

        if playing {
            // Open streams for clips about to be drawn so they are ready on time.
            for (track, lane) in project.tracks.iter().enumerate().filter(|(_, l)| !l.hidden) {
                for clip in &lane.clips {
                    let Some(drawn) = project.drawn(clip, track) else {
                        continue;
                    };
                    if drawn.from <= position || drawn.from > position + LOOKAHEAD || clip.frozen {
                        continue;
                    }
                    let Source::Media(media) = drawn.source else { continue };
                    if media.kind != MediaKind::Video {
                        continue;
                    }
                    let Some(path) = library.path(&media.hash) else {
                        continue;
                    };
                    keep_streams.insert(clip.id);
                    let size = self.decode_size((media.width, media.height));
                    let filters = clip.look.pixel_filters(size, scale_of(size, media), "p");
                    let source = clip.source_at(drawn.from).clamp(0, (media.duration - frame).max(0));
                    self.ensure_stream(clip, media, &filters, path, source, fps, size);
                    // Its backdrop too, so a cut into it is not black at the sides.
                    if clip.look.backdrop.is_some() {
                        let small = backdrop_size(size);
                        let backdrop = Clip {
                            id: compose::backdrop_id(clip.id),
                            ..clip.clone()
                        };
                        keep_streams.insert(backdrop.id);
                        let filters = Some(backdrop_filters(clip, small));
                        self.ensure_stream(&backdrop, media, &filters, path, source, fps, small);
                    }
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
            let ready = stack.iter().all(|w| {
                let streamed = w.media.kind == MediaKind::Video && !w.clip.frozen;
                if streamed {
                    self.flowing.contains(&w.clip.id)
                } else {
                    self.shown.contains_key(&w.clip.id)
                }
            });
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

    /// Keeps a stream for `clip` that will reach `source` soon, opening one
    /// if not, decoding frames of `size`.
    #[allow(clippy::too_many_arguments)]
    fn ensure_stream(
        &mut self,
        clip: &Clip,
        media: &Media,
        filters: &Option<String>,
        path: &std::path::Path,
        source: Time,
        fps: u32,
        size: (u32, u32),
    ) {
        let (speed, reverse) = (clip.speed, clip.reverse);
        if let Some(stream) = self.streams.get(&clip.id) {
            let next = stream.pending.as_ref().map_or(stream.decoder.next_time(), |(t, _)| *t);
            // How far along the stream is towards `source`, in its direction.
            let ahead = if reverse { source - next } else { next - source };
            let usable = stream.media == media.id
                && stream.filters == *filters
                && stream.speed == speed
                && stream.reverse == reverse
                && ahead <= (SECOND as f64 / 2.0 * speed) as Time
                && ahead >= -(SECOND as f64 * speed) as Time;
            if usable {
                return;
            }
        }
        let (w, h) = size;
        match FrameStream::open(path, source, fps, w, h, filters.as_deref(), speed, reverse) {
            Ok(decoder) => {
                self.streams.insert(
                    clip.id,
                    Stream {
                        media: media.id,
                        filters: filters.clone(),
                        speed,
                        reverse,
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
        let reverse = stream.reverse;
        let mut latest = None;
        let mut skipped = 0;
        loop {
            if stream.pending.is_none() {
                let t = stream.decoder.next_time();
                stream.pending = stream.decoder.try_next().map(|image| (t, image));
            }
            match stream.pending.take() {
                Some((t, image)) if (if reverse { t >= due } else { t <= due }) => {
                    if latest.replace((t, image)).is_some() {
                        skipped += 1;
                    }
                }
                Some(early) => {
                    stream.pending = Some(early);
                    break;
                }
                None => break,
            }
        }
        self.keeping.skipped += skipped;
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
        let (w, h) = size;
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
