//! The project: media, tracks of clips, and every edit the timeline offers.
//!
//! Tracks are layers. Track 0 is the bottom of the picture; each track above
//! it draws over it. Any clip can sit on any track, and clips on one track
//! never overlap: a clip dropped where there is no room rises to the first
//! track above with space, which is how overlays come about.

use crate::look::{Look, MotionKind};
use crate::time::{SECOND, Time, frame};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version of the project JSON. Bump it and add a step to `migrate` when the
/// shape changes.
pub const SCHEMA: u32 = 7;

/// How long a still image runs when it is first placed.
pub const IMAGE_DURATION: Time = 5 * SECOND;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrackId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Image,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub id: MediaId,
    pub name: String,
    /// Content hash; the key of the bytes inside the project file.
    pub hash: String,
    /// File extension, kept so decoders can sniff the container.
    pub ext: String,
    pub kind: MediaKind,
    /// Zero for images, which last as long as their clip.
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    /// Where the file was imported from, for relinking if the embedded copy is lost.
    #[serde(default)]
    pub source: PathBuf,
    /// Media Splice makes rather than imports: music from a score, a scene
    /// described as layers that move. `hash` then names the render of this recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<Recipe>,
    /// Where a see-through render draws anything, as fractions of its frame
    /// (x, y, width, height), so it is picked and outlined by what it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<[f32; 4]>,
    /// For a screen recording: where and when the mouse was clicked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clicks: Vec<Click>,
}

/// A mouse click in a screen recording: when (source time), and where, as
/// fractions of the frame from its top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Click {
    pub at: Time,
    pub x: f32,
    pub y: f32,
}

/// What made media is made from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Recipe {
    /// Music: instruments and notes, played by a General MIDI synth.
    Score(crate::score::Score),
    /// Pictures that move: layers described as JSON (`motion::Spec`), painted frame by frame.
    Scene(Scene),
    /// A piece of another timeline kept for reuse, with its placeholders filled in.
    Symbol(crate::symbol::Symbol),
}

impl Recipe {
    pub fn kind(&self) -> &'static str {
        match self {
            Recipe::Score(_) => "score",
            Recipe::Scene(_) => "scene",
            Recipe::Symbol(_) => "symbol",
        }
    }

    /// Names one render of this recipe: the same recipe, canvas and renderer
    /// (`engine`) always render the same bytes, so this is their hash too.
    pub fn key(&self, settings: &Settings, engine: &str) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(engine.as_bytes());
        hasher.update(&[0]);
        match self {
            Recipe::Score(score) => {
                hasher.update(serde_json::to_string(score).unwrap_or_default().as_bytes());
            }
            Recipe::Scene(scene) => {
                let shape = format!(
                    "{}x{}@{}|{}|{}|",
                    settings.width, settings.height, settings.fps, scene.duration, scene.transparent
                );
                hasher.update(shape.as_bytes());
                hasher.update(scene.source.as_bytes());
            }
            Recipe::Symbol(symbol) => {
                // Everything it draws is in its filled timeline, media by hash,
                // at the canvas it renders at in this movie.
                let at = symbol.render_settings(*settings);
                hasher.update(format!("{}x{}@{}|", at.width, at.height, at.fps).as_bytes());
                hasher.update(symbol.rendered(*settings).to_json().as_bytes());
            }
        }
        hasher.finalize().to_hex().to_string()
    }

    /// How long its render runs.
    pub fn duration(&self) -> anyhow::Result<Time> {
        match self {
            Recipe::Score(score) => Ok(crate::time::from_secs(score.compile()?.seconds)),
            Recipe::Scene(scene) => Ok(scene.duration),
            Recipe::Symbol(symbol) => match symbol.project.duration() {
                0 => anyhow::bail!("the symbol {} is empty", symbol.name),
                _ => Ok(symbol.length()),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// The scene's JSON, as written (a `motion::Spec`).
    pub source: String,
    pub duration: Time,
    /// See-through where nothing is drawn, so it overlays what is under it.
    #[serde(default = "yes")]
    pub transparent: bool,
    /// Names the folder its file lives in, for editing outside Splice.
    pub workspace: String,
}

fn yes() -> bool {
    true
}

impl Media {
    pub fn is_visual(&self) -> bool {
        self.kind != MediaKind::Audio
    }

    /// Whether clips of this media are bounded by the source length.
    pub fn is_timed(&self) -> bool {
        self.kind != MediaKind::Image
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub media: MediaId,
    /// Where the clip begins on the timeline.
    pub start: Time,
    /// Where in the source the clip begins.
    pub source_in: Time,
    pub duration: Time,
    #[serde(default = "one")]
    pub volume: f32,
    /// Position, size, opacity, key, mask and motion on the canvas.
    #[serde(default, skip_serializing_if = "is_default")]
    pub look: Look,
    /// Made by Splice (a title, a colour) rather than cut from media; `media`
    /// is then unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<Generator>,
    /// In a symbol, marks what each use of it can change: the words of a
    /// title, a colour, or the media shown. Clips with the same label change
    /// together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<crate::symbol::Tag>,
    /// How fast the source plays: 2 is twice as fast (and the clip half as long).
    #[serde(default = "one64", skip_serializing_if = "is_one")]
    pub speed: f64,
    /// Plays its stretch of the source backwards.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
    /// Holds one frame (at `source_in`) for as long as the clip lasts.
    #[serde(default, skip_serializing_if = "is_false")]
    pub frozen: bool,
    /// The way in from the clip before it (or from what is beneath).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<crate::transition::Transition>,
}

fn is_default(look: &Look) -> bool {
    *look == Look::default()
}

fn one64() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// The slowest and fastest a clip plays.
pub const SLOWEST: f64 = 0.1;
pub const FASTEST: f64 = 16.0;

/// Content Splice draws itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Generator {
    Text(Title),
    /// A flat colour filling the canvas: a background, a title card.
    Color {
        color: u32,
    },
}

impl Generator {
    pub fn name(&self) -> String {
        match self {
            Generator::Text(title) => {
                let line = title.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Text");
                line.trim().to_string()
            }
            Generator::Color { color } => match color & 0xff {
                0xff => format!("#{:06x}", color >> 8),
                alpha => format!("#{:06x} at {}%", color >> 8, (alpha * 100 + 127) / 255),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

/// Text on the canvas, in a system font.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Title {
    pub text: String,
    /// A font family installed on the system.
    pub font: String,
    /// Letter size as a fraction of the canvas height.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    /// 0xRRGGBBAA.
    pub color: u32,
    pub align: Align,
    /// A box behind the text, 0xRRGGBBAA.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<u32>,
    /// A soft shadow under the letters.
    pub shadow: bool,
    /// Coming in a piece at a time: word by word, letter by letter, line by line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animate: Option<crate::textmotion::TextMotion>,
    /// When each word is said, from the clip's start (captions); words
    /// come in (or light up) then.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub timing: Vec<Time>,
}

impl Default for Title {
    fn default() -> Title {
        Title {
            text: "Title".into(),
            font: "Helvetica Neue".into(),
            size: 0.08,
            bold: true,
            italic: false,
            color: 0xffffffff,
            align: Align::Center,
            background: None,
            shadow: true,
            animate: None,
            timing: Vec::new(),
        }
    }
}

/// What a clip shows: cut from media, or drawn by Splice.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    Media(&'a Media),
    Generated(&'a Generator),
}

impl Source<'_> {
    pub fn is_visual(&self) -> bool {
        match self {
            Source::Media(m) => m.is_visual(),
            Source::Generated(_) => true,
        }
    }
}

fn one() -> f32 {
    1.0
}

impl Clip {
    /// A plain clip of `media`: as it is, at full volume, playing forwards.
    pub fn new(id: ClipId, media: MediaId, start: Time, source_in: Time, duration: Time) -> Clip {
        Clip {
            id,
            media,
            start,
            source_in,
            duration,
            volume: 1.0,
            look: Look::default(),
            generator: None,
            placeholder: None,
            speed: 1.0,
            reverse: false,
            frozen: false,
            transition: None,
        }
    }

    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    pub fn contains(&self, t: Time) -> bool {
        self.start <= t && t < self.end()
    }

    /// How much source passes per second of timeline: its speed, or 0 when frozen.
    pub fn rate(&self) -> f64 {
        if self.frozen { 0.0 } else { self.speed }
    }

    /// How much of the source the clip plays.
    pub fn span(&self) -> Time {
        Self::scaled(self.duration, self.rate())
    }

    /// Where the stretch of source it plays ends.
    pub fn source_out(&self) -> Time {
        self.source_in + self.span()
    }

    fn scaled(t: Time, k: f64) -> Time {
        (t as f64 * k).round() as Time
    }

    /// The source time shown at timeline time `t`. Outside the clip (a
    /// transition drawing it early or late) it carries on the same way.
    pub fn source_at(&self, t: Time) -> Time {
        let local = Self::scaled(t - self.start, self.rate());
        if self.frozen {
            self.source_in
        } else if self.reverse {
            // The last frame first: just before where its stretch ends.
            self.source_out() - 1 - local
        } else {
            self.source_in + local
        }
    }

    /// Whether this clip changes how its source plays.
    pub fn is_retimed(&self) -> bool {
        self.speed != 1.0 || self.reverse || self.frozen
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    /// Sorted by `start`, never overlapping.
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub hidden: bool,
}

impl Track {
    pub fn end(&self) -> Time {
        self.clips.last().map_or(0, Clip::end)
    }

    fn is_free(&self, start: Time, end: Time, ignore: &[ClipId]) -> bool {
        self.clips
            .iter()
            .filter(|c| !ignore.contains(&c.id))
            .all(|c| c.end() <= start || c.start >= end)
    }

    fn insert_sorted(&mut self, clip: Clip) {
        let at = self.clips.partition_point(|c| c.start <= clip.start);
        self.clips.insert(at, clip);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            width: 1920,
            height: 1080,
            fps: 30,
        }
    }
}

/// Canvas shapes offered for a new project.
pub const ASPECTS: &[(&str, u32, u32)] = &[
    ("16:9", 1920, 1080),
    ("9:16", 1080, 1920),
    ("1:1", 1080, 1080),
    ("4:5", 1080, 1350),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema: u32,
    pub settings: Settings,
    pub media: Vec<Media>,
    /// Bottom layer first.
    pub tracks: Vec<Track>,
    /// The plan: story beats along the timeline, and cues.
    #[serde(default, skip_serializing_if = "crate::story::Story::is_empty")]
    pub story: crate::story::Story,
    /// Who and what the movie keeps coming back to, and its look.
    #[serde(default, skip_serializing_if = "crate::bible::Bible::is_empty")]
    pub bible: crate::bible::Bible,
    /// In a symbol: the stretch of it (start, end) that a longer or shorter
    /// use stretches, so what comes before and after (an entrance, an exit)
    /// keeps its timing. None stretches all of it evenly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<(Time, Time)>,
    /// Black bars that frame the whole movie to a wider (or narrower)
    /// shape: 2.39 for a cinema look. Drawn over everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letterbox: Option<f32>,
    /// How this project records; None uses the default the user has set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub studio: Option<crate::studio::Studio>,
    next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            schema: SCHEMA,
            settings: Settings::default(),
            media: Vec::new(),
            tracks: vec![Track {
                id: TrackId(1),
                clips: Vec::new(),
                muted: false,
                hidden: false,
            }],
            story: crate::story::Story::default(),
            bible: crate::bible::Bible::default(),
            hold: None,
            letterbox: None,
            studio: None,
            next_id: 2,
        }
    }
}

/// Clip placement: the track it is on and its index there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub track: usize,
    pub index: usize,
}

impl Project {
    pub fn from_json(json: &str) -> anyhow::Result<Project> {
        let mut value: serde_json::Value = serde_json::from_str(json)?;
        migrate(&mut value)?;
        let mut project: Project = serde_json::from_value(value)?;
        project.normalize();
        Ok(project)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("project serializes")
    }

    pub(crate) fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Repairs anything a hand-edited or older file could get wrong.
    fn normalize(&mut self) {
        if self.tracks.is_empty() {
            let id = TrackId(self.alloc());
            self.tracks.push(Track {
                id,
                clips: Vec::new(),
                muted: false,
                hidden: false,
            });
        }
        let max_id = self
            .media
            .iter()
            .map(|m| m.id.0)
            .chain(self.tracks.iter().map(|t| t.id.0))
            .chain(self.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.id.0)))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_id + 1);
        for track in &mut self.tracks {
            track.clips.sort_by_key(|c| c.start);
        }
    }

    pub fn frame(&self) -> Time {
        frame(self.settings.fps)
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(|t| t.clips.is_empty())
    }

    pub fn duration(&self) -> Time {
        self.tracks.iter().map(Track::end).max().unwrap_or(0)
    }

    pub fn media(&self, id: MediaId) -> Option<&Media> {
        self.media.iter().find(|m| m.id == id)
    }

    /// What a clip shows.
    pub fn source<'a>(&'a self, clip: &'a Clip) -> Option<Source<'a>> {
        match &clip.generator {
            Some(generator) => Some(Source::Generated(generator)),
            None => self.media(clip.media).map(Source::Media),
        }
    }

    /// The clip's name as the timeline shows it.
    pub fn clip_name(&self, clip: &Clip) -> String {
        match self.source(clip) {
            Some(Source::Media(m)) => m.name.clone(),
            Some(Source::Generated(g)) => g.name(),
            None => "missing".into(),
        }
    }

    pub fn slot(&self, id: ClipId) -> Option<Slot> {
        self.tracks.iter().enumerate().find_map(|(track, t)| {
            t.clips
                .iter()
                .position(|c| c.id == id)
                .map(|index| Slot { track, index })
        })
    }

    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.slot(id).map(|s| &self.tracks[s.track].clips[s.index])
    }

    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.slot(id).map(|s| &mut self.tracks[s.track].clips[s.index])
    }

    pub fn clips(&self) -> impl Iterator<Item = (usize, &Clip)> {
        self.tracks
            .iter()
            .enumerate()
            .flat_map(|(i, t)| t.clips.iter().map(move |c| (i, c)))
    }

    /// Adds media to the library, or returns the existing entry with the same bytes.
    pub fn add_media(&mut self, mut media: Media) -> MediaId {
        if let Some(existing) = self.media.iter().find(|m| m.hash == media.hash) {
            return existing.id;
        }
        media.id = MediaId(self.alloc());
        let id = media.id;
        self.media.push(media);
        id
    }

    /// Points made media at a finished render of its recipe, keeping its
    /// clips inside the new length.
    pub fn set_render(&mut self, id: MediaId, hash: &str, duration: Time) {
        let Some(media) = self.media.iter_mut().find(|m| m.id == id) else {
            return;
        };
        let old = media.duration;
        media.hash = hash.to_string();
        media.duration = duration;
        let frame = self.frame();
        for track in &mut self.tracks {
            let starts: Vec<Time> = track.clips.iter().map(|c| c.start).collect();
            for clip in track
                .clips
                .iter_mut()
                .filter(|c| c.media == id && c.generator.is_none())
            {
                // A clip of the whole thing stays the whole thing, as far as
                // the next clip allows.
                let whole = clip.source_in == 0 && (clip.span() - old).abs() <= frame;
                clip.source_in = clip.source_in.min((duration - frame).max(0));
                if clip.frozen {
                    continue;
                }
                let room = starts
                    .iter()
                    .copied()
                    .filter(|s| *s > clip.start)
                    .min()
                    .map_or(Time::MAX, |s| s - clip.start);
                let timeline = |source: Time| (source as f64 / clip.speed).floor() as Time;
                clip.duration = if whole {
                    timeline(duration).min(room)
                } else {
                    clip.duration.min(timeline(duration - clip.source_in))
                }
                .max(frame);
            }
        }
    }

    /// Changes what made media is made from.
    pub fn set_recipe(&mut self, id: MediaId, recipe: Recipe) {
        if let Some(media) = self.media.iter_mut().find(|m| m.id == id) {
            media.recipe = Some(recipe);
        }
    }

    /// Removes media from the library along with every clip that uses it.
    pub fn remove_media(&mut self, id: MediaId) {
        self.media.retain(|m| m.id != id);
        for track in &mut self.tracks {
            track.clips.retain(|c| c.media != id);
        }
        self.prune_tracks();
    }

    fn new_track(&mut self) -> Track {
        Track {
            id: TrackId(self.alloc()),
            clips: Vec::new(),
            muted: false,
            hidden: false,
        }
    }

    /// Puts `clip` on `track` or the first track above it with room, adding a
    /// track on top when none has. Returns the track it landed on.
    pub(crate) fn place(&mut self, clip: Clip, track: usize) -> usize {
        let start = track.min(self.tracks.len());
        let found = (start..self.tracks.len()).find(|&i| self.tracks[i].is_free(clip.start, clip.end(), &[]));
        let index = match found {
            Some(i) => i,
            None => {
                let new = self.new_track();
                self.tracks.push(new);
                self.tracks.len() - 1
            }
        };
        self.tracks[index].insert_sorted(clip);
        index
    }

    /// Drops trailing and inner empty tracks, keeping at least one.
    pub fn prune_tracks(&mut self) {
        self.tracks.retain(|t| !t.clips.is_empty());
        if self.tracks.is_empty() {
            let track = self.new_track();
            self.tracks.push(track);
        }
    }

    /// Places media on the timeline. The first clip in an empty project sets
    /// the canvas to the media's shape.
    pub fn insert_clip(&mut self, media: MediaId, track: usize, start: Time) -> Option<ClipId> {
        // Another clip of a use of a symbol is another use, with its own fills.
        let media = if self.is_used_symbol(media) {
            self.fork_media(media)?
        } else {
            media
        };
        let m = self.media(media)?.clone();
        if self.is_empty() && m.is_visual() && m.width > 0 && m.height > 0 {
            let (w, h) = fit_canvas(m.width, m.height);
            self.settings.width = w;
            self.settings.height = h;
        }
        let duration = if m.is_timed() { m.duration } else { IMAGE_DURATION };
        if duration <= 0 {
            return None;
        }
        let id = ClipId(self.alloc());
        let clip = Clip::new(id, media, start.max(0), 0, duration);
        self.place(clip, track);
        Some(id)
    }

    /// Places a title or colour. It goes on `track` or the first free one above.
    pub fn insert_generated(&mut self, generator: Generator, track: usize, start: Time, duration: Time) -> ClipId {
        let id = ClipId(self.alloc());
        let clip = Clip {
            generator: Some(generator),
            ..Clip::new(id, MediaId(0), start.max(0), 0, duration.max(self.frame()))
        };
        self.place(clip, track);
        id
    }

    /// Changes a clip's look.
    pub fn set_look(&mut self, id: ClipId, f: impl FnOnce(&mut Look)) {
        if let Some(clip) = self.clip_mut(id) {
            f(&mut clip.look);
        }
    }

    /// Changes a title or colour clip's content.
    pub fn set_generator(&mut self, id: ClipId, f: impl FnOnce(&mut Generator)) {
        if let Some(generator) = self.clip_mut(id).and_then(|c| c.generator.as_mut()) {
            f(generator);
        }
    }

    /// Places media at the end of the bottom track.
    pub fn append_clip(&mut self, media: MediaId) -> Option<ClipId> {
        let end = self.tracks[0].end();
        self.insert_clip(media, 0, end)
    }

    /// Moves clips by `dt` and `dtrack` together. Clips that would collide on
    /// their destination rise to free tracks, so a move never destroys anything.
    pub fn move_clips(&mut self, ids: &[ClipId], dt: Time, dtrack: isize) {
        let mut moving: Vec<(usize, Clip)> = Vec::new();
        for &id in ids {
            if let Some(slot) = self.slot(id) {
                moving.push((slot.track, self.tracks[slot.track].clips.remove(slot.index)));
            }
        }
        if moving.is_empty() {
            return;
        }
        // Keep the group's relative timing when the leftmost clip would pass zero.
        let earliest = moving.iter().map(|(_, c)| c.start).min().unwrap_or(0);
        let dt = dt.max(-earliest);
        let lowest = moving.iter().map(|(t, _)| *t).min().unwrap_or(0) as isize;
        let dtrack = dtrack.max(-lowest);
        moving.sort_by_key(|(t, c)| (*t, c.start));
        for (track, mut clip) in moving {
            clip.start += dt;
            let target = (track as isize + dtrack) as usize;
            if target >= self.tracks.len() {
                // Dragging above the top layer makes a new one.
                while self.tracks.len() <= target {
                    let new = self.new_track();
                    self.tracks.push(new);
                }
            }
            self.place(clip, target);
        }
        self.prune_tracks();
    }

    /// Moves a clip's left edge to `t`, keeping its right edge.
    pub fn trim_start(&mut self, id: ClipId, t: Time) {
        let Some(slot) = self.slot(id) else { return };
        let min_len = self.frame();
        let track = &self.tracks[slot.track];
        let clip = &track.clips[slot.index];
        let media_length = self.media(clip.media).filter(|m| m.is_timed()).map(|m| m.duration);
        let timed = media_length.is_some() && !clip.frozen;
        let previous_end = slot.index.checked_sub(1).map_or(0, |i| track.clips[i].end());
        let mut lower = previous_end.max(0);
        if let Some(length) = media_length.filter(|_| timed) {
            let timeline = |source: Time| (source as f64 / clip.speed).floor() as Time;
            lower = lower.max(if clip.reverse {
                // The left edge shows where its stretch ends, which the media bounds.
                clip.end() - timeline(length - clip.source_in)
            } else {
                clip.start - timeline(clip.source_in)
            });
        }
        let t = t.clamp(lower, clip.end() - min_len);
        let clip = &mut self.tracks[slot.track].clips[slot.index];
        let delta = t - clip.start;
        let out = clip.source_out();
        clip.start = t;
        clip.duration -= delta;
        if timed && !clip.reverse {
            clip.source_in = (out - clip.span()).max(0);
        }
        // Keyframes stay with the moments they were set on.
        clip.look.keys.shift(-delta);
    }

    /// Moves a clip's right edge to `t`.
    pub fn trim_end(&mut self, id: ClipId, t: Time) {
        let Some(slot) = self.slot(id) else { return };
        let min_len = self.frame();
        let track = &self.tracks[slot.track];
        let clip = &track.clips[slot.index];
        let next_start = track.clips.get(slot.index + 1).map_or(Time::MAX, |c| c.start);
        // A whole use of a symbol stretches (or shrinks) its hold rather than
        // being cut, so its exit still plays at the end.
        if let Some(media) = self.media(clip.media)
            && let Some(Recipe::Symbol(symbol)) = &media.recipe
            && clip.generator.is_none()
            && clip.source_in == 0
            && (clip.duration - media.duration).abs() <= min_len
        {
            let mut symbol = symbol.clone();
            let (start, media_id) = (clip.start, clip.media);
            let want = (t.min(next_start) - start).max(min_len);
            symbol.set_length(want);
            let length = symbol.length();
            if let Some(m) = self.media.iter_mut().find(|m| m.id == media_id) {
                m.recipe = Some(Recipe::Symbol(symbol));
                m.duration = length;
            }
            self.tracks[slot.track].clips[slot.index].duration = length;
            return;
        }
        let mut upper = next_start;
        let timed = self.media(clip.media).filter(|m| m.is_timed() && !clip.frozen);
        if let Some(media) = timed {
            let timeline = |source: Time| (source as f64 / clip.speed).floor() as Time;
            upper = upper.min(
                clip.start
                    + if clip.reverse {
                        // The right edge shows where its stretch starts, which can reach 0.
                        timeline(clip.source_out())
                    } else {
                        timeline(media.duration - clip.source_in)
                    },
            );
        }
        let timed = timed.is_some();
        let t = t.clamp(clip.start + min_len, upper.max(clip.start + min_len));
        let clip = &mut self.tracks[slot.track].clips[slot.index];
        let out = clip.source_out();
        clip.duration = t - clip.start;
        if timed && clip.reverse {
            clip.source_in = (out - clip.span()).max(0);
        }
    }

    /// Cuts a clip in two at `t`; returns the right half.
    pub fn split(&mut self, id: ClipId, t: Time) -> Option<ClipId> {
        let min_len = self.frame();
        let slot = self.slot(id)?;
        let clip = &self.tracks[slot.track].clips[slot.index];
        if t < clip.start + min_len || t > clip.end() - min_len {
            return None;
        }
        let timed = self.media(clip.media).is_some_and(Media::is_timed) && !clip.frozen;
        let new_id = ClipId(self.alloc());
        let clip = &mut self.tracks[slot.track].clips[slot.index];
        let left = t - clip.start;
        let whole = clip.duration;
        let mut right = Clip {
            id: new_id,
            start: t,
            duration: whole - left,
            // A cut in the middle of a clip has no transition.
            transition: None,
            ..clip.clone()
        };
        if timed {
            let source = |t: Time| (t as f64 * clip.speed).round() as Time;
            if clip.reverse {
                // Backwards, the left half plays the later part of the source.
                clip.source_in += source(whole - left);
            } else {
                right.source_in = clip.source_in + source(left);
            }
        }
        // The animation in stays with the left half, the one out with the right;
        // keyframes go with the half they are in, each half keeping its ends.
        right.look.enter.kind = MotionKind::None;
        clip.look.exit.kind = MotionKind::None;
        right.look.keys = clip.look.keys.part(left, whole);
        clip.look.keys = clip.look.keys.part(0, left);
        clip.duration = left;
        self.tracks[slot.track].clips.insert(slot.index + 1, right);
        Some(new_id)
    }

    /// Splits `ids` at `t`, or every visible clip under `t` when `ids` is empty.
    /// Returns the new right halves.
    pub fn split_at(&mut self, t: Time, ids: &[ClipId]) -> Vec<ClipId> {
        let targets: Vec<ClipId> = if ids.is_empty() {
            self.clips().filter(|(_, c)| c.contains(t)).map(|(_, c)| c.id).collect()
        } else {
            ids.to_vec()
        };
        targets.into_iter().filter_map(|id| self.split(id, t)).collect()
    }

    pub fn delete(&mut self, ids: &[ClipId]) {
        for track in &mut self.tracks {
            track.clips.retain(|c| !ids.contains(&c.id));
        }
        self.prune_tracks();
    }

    /// Deletes clips and closes each gap by pulling later clips on the same
    /// track to the left.
    pub fn ripple_delete(&mut self, ids: &[ClipId]) {
        for track in &mut self.tracks {
            let mut removed: Vec<(Time, Time)> = track
                .clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| (c.start, c.duration))
                .collect();
            if removed.is_empty() {
                continue;
            }
            removed.sort();
            track.clips.retain(|c| !ids.contains(&c.id));
            for clip in &mut track.clips {
                let shift: Time = removed
                    .iter()
                    .filter(|(start, _)| *start < clip.start)
                    .map(|(_, d)| d)
                    .sum();
                clip.start -= shift;
            }
        }
        self.prune_tracks();
    }

    /// Copies clips to just after themselves; returns the copies.
    pub fn duplicate(&mut self, ids: &[ClipId]) -> Vec<ClipId> {
        let mut copies = Vec::new();
        for &id in ids {
            let Some(slot) = self.slot(id) else { continue };
            let mut clip = self.tracks[slot.track].clips[slot.index].clone();
            clip.id = ClipId(self.alloc());
            clip.start = clip.end();
            if clip.generator.is_none()
                && self.is_used_symbol(clip.media)
                && let Some(own) = self.fork_media(clip.media)
            {
                clip.media = own;
            }
            copies.push(clip.id);
            self.place(clip, slot.track);
        }
        copies
    }

    /// Moves every clip on `track` that starts at or after `from` by `dt`.
    fn ripple(&mut self, track: usize, from: Time, dt: Time, except: ClipId) {
        if dt == 0 {
            return;
        }
        for clip in &mut self.tracks[track].clips {
            if clip.id != except && clip.start >= from {
                clip.start += dt;
            }
        }
        self.tracks[track].clips.sort_by_key(|c| c.start);
    }

    /// Plays clips at `speed` (2 is twice as fast), keeping what of the
    /// source each plays: a clip sped up gets shorter and one slowed down
    /// longer, and what follows on its track moves up or along to match.
    pub fn set_speed(&mut self, ids: &[ClipId], speed: f64) {
        let speed = if speed.is_finite() {
            speed.clamp(SLOWEST, FASTEST)
        } else {
            1.0
        };
        let frame = self.frame();
        for &id in ids {
            let Some(slot) = self.slot(id) else { continue };
            let clip = &self.tracks[slot.track].clips[slot.index];
            let timed = self.media(clip.media).is_some_and(|m| m.is_timed());
            if clip.generator.is_some() || !timed || clip.frozen || clip.speed == speed {
                continue;
            }
            let (end, old) = (clip.end(), clip.speed);
            let span = clip.span();
            // On whole frames, so what follows stays on the frame grid.
            let fps = self.settings.fps;
            let length = crate::time::floor_to_frame((span as f64 / speed).round() as Time + frame / 2, fps);
            if length < frame {
                // Too short to play that fast.
                continue;
            }
            let clip = &mut self.tracks[slot.track].clips[slot.index];
            let out = clip.source_out();
            clip.speed = speed;
            clip.duration = length;
            if clip.reverse {
                clip.source_in = (out - clip.span()).max(0);
            }
            // Keyframes and the way in keep their place in what is shown.
            clip.look.keys.scale_time(old / speed);
            if let Some(t) = &mut clip.transition {
                t.duration = t.duration.min(clip.duration);
            }
            let dt = clip.end() - end;
            self.ripple(slot.track, end, dt, id);
        }
    }

    /// Plays clips backwards (or forwards again), over the same stretch of source.
    pub fn set_reverse(&mut self, ids: &[ClipId], reverse: bool) {
        for &id in ids {
            let timed = self
                .clip(id)
                .filter(|c| c.generator.is_none() && !c.frozen)
                .and_then(|c| self.media(c.media))
                .is_some_and(|m| m.kind == MediaKind::Video || m.kind == MediaKind::Audio);
            if let Some(clip) = self.clip_mut(id).filter(|_| timed) {
                clip.reverse = reverse;
            }
        }
    }

    /// Holds the frame of clip `id` at timeline time `t` for `hold`: the clip
    /// is cut there, the held frame goes between the halves, and what follows
    /// on the track moves along. Returns the held clip.
    pub fn freeze_frame(&mut self, id: ClipId, t: Time, hold: Time) -> Option<ClipId> {
        let frame = self.frame();
        let clip = self.clip(id)?.clone();
        let media = self.media(clip.media)?;
        if clip.generator.is_some() || media.kind != MediaKind::Video || clip.frozen || !clip.contains(t) {
            return None;
        }
        let hold = hold.max(frame);
        // The frame shown there, kept inside the media.
        let at =
            crate::time::floor_to_frame(clip.source_at(t), self.settings.fps).clamp(0, (media.duration - frame).max(0));
        let slot = self.slot(id)?;
        let mut t = crate::time::floor_to_frame(t, self.settings.fps).max(clip.start);
        // Cut there; too near an edge to cut, the frame goes before or after it.
        if t > clip.start && self.split(id, t).is_none() {
            t = if t - clip.start < clip.end() - t {
                clip.start
            } else {
                clip.end()
            };
        }
        // What follows on the track (the right half too) moves along.
        self.ripple(slot.track, t, hold, ClipId(u64::MAX));
        let held = ClipId(self.alloc());
        let mut still = Clip {
            frozen: true,
            volume: 0.0,
            ..Clip::new(held, clip.media, t, at, hold)
        };
        still.look = clip.look.clone();
        still.look.enter.kind = MotionKind::None;
        still.look.exit.kind = MotionKind::None;
        // Animated values hold where they were at the frame.
        let local = (t - clip.start).min(clip.duration);
        still.look.keys = clip.look.keys.part(local, local);
        for prop in crate::look::Prop::ALL {
            still.look.keys.of_mut(prop).truncate(1);
        }
        self.tracks[slot.track].insert_sorted(still);
        Some(held)
    }

    /// The timeline with only what plays between `from` and `to`: clips
    /// wholly outside it go, the rest stay as they are (so they move as they
    /// do in the whole movie). Exporting a stretch cuts it from this.
    pub fn window(&self, from: Time, to: Time) -> Project {
        let mut out = self.clone();
        out.story = crate::story::Story::default();
        // A clip drawn on past its edges for a transition counts by where it is drawn.
        let drawn: Vec<ClipId> = self
            .clips()
            .filter(|(_, c)| {
                let (head, tail) = self.handles(c.id);
                c.start - head < to && c.end() + tail > from
            })
            .map(|(_, c)| c.id)
            .collect();
        // So is a clip a kept one's transition comes from, so it plays the same.
        let sources: Vec<ClipId> = drawn
            .iter()
            .filter_map(|id| self.cut_into(*id).and_then(|cut| cut.from))
            .collect();
        for track in &mut out.tracks {
            track.clips.retain(|c| drawn.contains(&c.id) || sources.contains(&c.id));
        }
        out
    }

    pub fn set_volume(&mut self, ids: &[ClipId], volume: f32) {
        for &id in ids {
            if let Some(clip) = self.clip_mut(id) {
                clip.volume = volume.clamp(0.0, 4.0);
            }
        }
    }

    /// Every edge a dragged edge or clip can snap to.
    pub fn snap_points(&self, exclude: &[ClipId]) -> Vec<Time> {
        let mut points = vec![0];
        for (_, clip) in self.clips().filter(|(_, c)| !exclude.contains(&c.id)) {
            points.push(clip.start);
            points.push(clip.end());
        }
        points.sort_unstable();
        points.dedup();
        points
    }

    /// The bars a letterbox draws, in canvas pixels: (top and bottom bar
    /// height, left and right bar width); one of them is 0.
    pub fn letterbox_bars(&self) -> Option<(u32, u32)> {
        let aspect = self.letterbox.filter(|a| a.is_finite() && *a > 0.1)? as f64;
        let (w, h) = (self.settings.width as f64, self.settings.height as f64);
        let shape = w / h.max(1.0);
        if (aspect - shape).abs() < 0.01 {
            return None;
        }
        Some(if aspect > shape {
            (
                (((h - w / aspect) / 2.0).round() as u32).min(self.settings.height / 2),
                0,
            )
        } else {
            (
                0,
                (((w - h * aspect) / 2.0).round() as u32).min(self.settings.width / 2),
            )
        })
    }

    /// Visible picture layers at `t`, bottom first.
    pub fn visual_stack(&self, t: Time) -> Vec<(&Clip, Source<'_>)> {
        self.tracks
            .iter()
            .filter(|track| !track.hidden)
            .flat_map(|track| track.clips.iter())
            .filter(|c| c.contains(t))
            .filter_map(|c| self.source(c).filter(Source::is_visual).map(|s| (c, s)))
            .collect()
    }

    /// Clips whose sound plays, bottom track first.
    pub fn audible_clips(&self) -> Vec<(&Clip, &Media)> {
        self.tracks
            .iter()
            .filter(|track| !track.muted)
            .flat_map(|track| track.clips.iter())
            .filter(|c| c.volume > 0.0 && c.generator.is_none())
            .filter_map(|c| self.media(c.media).filter(|m| m.has_audio).map(|m| (c, m)))
            .collect()
    }
}

/// Canvas size for media of the given shape: even dimensions, the long side at
/// most 1920 (or 1080 on the short side for portrait).
pub fn fit_canvas(width: u32, height: u32) -> (u32, u32) {
    let scale = (1920.0 / width.max(height) as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32).max(2) / 2) * 2;
    (even(width as f64 * scale), even(height as f64 * scale))
}

/// Upgrades older project JSON in place to `SCHEMA`.
fn migrate(value: &mut serde_json::Value) -> anyhow::Result<()> {
    let schema = value.get("schema").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    if schema > SCHEMA {
        anyhow::bail!("this project was saved by a newer Splice (schema {schema}); this build reads up to {SCHEMA}");
    }
    // Schema 2 added `look` and `generator` to clips, schema 3 `recipe` to
    // media, schema 4 `story` to the project, schema 5 symbols (a recipe) and
    // `placeholder` on clips, schema 6 speed, reverse, freeze, transitions,
    // colour and keyframes on clips, schema 7 effects, backdrops, hidden boxes,
    // letterboxes, titles that come in a piece at a time (and captions'
    // word timings) and recordings' clicks; all default, so older projects
    // need nothing.
    value["schema"] = SCHEMA.into();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(kind: MediaKind, duration: Time) -> Media {
        Media {
            id: MediaId(0),
            name: "m".into(),
            hash: format!("{kind:?}{duration}"),
            ext: "mp4".into(),
            kind,
            duration,
            width: 1280,
            height: 720,
            has_audio: kind != MediaKind::Image,
            source: PathBuf::new(),
            recipe: None,
            content: None,
            clicks: Vec::new(),
        }
    }

    fn project_with(duration: Time) -> (Project, MediaId) {
        let mut p = Project::default();
        let m = p.add_media(media(MediaKind::Video, duration));
        (p, m)
    }

    #[test]
    fn first_clip_sets_canvas() {
        let (mut p, m) = project_with(10 * SECOND);
        p.append_clip(m).unwrap();
        assert_eq!((p.settings.width, p.settings.height), (1280, 720));
    }

    #[test]
    fn append_runs_end_to_end() {
        let (mut p, m) = project_with(4 * SECOND);
        let a = p.append_clip(m).unwrap();
        let b = p.append_clip(m).unwrap();
        assert_eq!(p.clip(b).unwrap().start, p.clip(a).unwrap().end());
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.duration(), 8 * SECOND);
    }

    #[test]
    fn collisions_rise_to_new_track() {
        let (mut p, m) = project_with(4 * SECOND);
        p.insert_clip(m, 0, 0).unwrap();
        let b = p.insert_clip(m, 0, 2 * SECOND).unwrap();
        assert_eq!(p.slot(b).unwrap().track, 1);
    }

    #[test]
    fn split_and_ripple() {
        let (mut p, m) = project_with(10 * SECOND);
        let a = p.append_clip(m).unwrap();
        let b = p.split(a, 4 * SECOND).unwrap();
        let right = p.clip(b).unwrap();
        assert_eq!(
            (right.start, right.source_in, right.duration),
            (4 * SECOND, 4 * SECOND, 6 * SECOND)
        );
        let c = p.split(b, 7 * SECOND).unwrap();
        p.ripple_delete(&[b]);
        let last = p.clip(c).unwrap();
        assert_eq!((last.start, last.source_in), (4 * SECOND, 7 * SECOND));
        assert_eq!(p.duration(), 7 * SECOND);
    }

    #[test]
    fn split_refuses_edges() {
        let (mut p, m) = project_with(10 * SECOND);
        let a = p.append_clip(m).unwrap();
        assert!(p.split(a, 0).is_none());
        assert!(p.split(a, 10 * SECOND).is_none());
    }

    #[test]
    fn trims_respect_source_and_neighbours() {
        let (mut p, m) = project_with(10 * SECOND);
        let a = p.append_clip(m).unwrap();
        p.trim_end(a, 20 * SECOND);
        assert_eq!(p.clip(a).unwrap().duration, 10 * SECOND);
        p.trim_start(a, 3 * SECOND);
        let clip = p.clip(a).unwrap();
        assert_eq!(
            (clip.start, clip.source_in, clip.duration),
            (3 * SECOND, 3 * SECOND, 7 * SECOND)
        );
        p.trim_start(a, -5 * SECOND);
        let clip = p.clip(a).unwrap();
        assert_eq!((clip.start, clip.source_in), (0, 0));

        let b = p.append_clip(m).unwrap();
        p.trim_start(b, 0);
        assert_eq!(
            p.clip(b).unwrap().start,
            10 * SECOND,
            "cannot trim over the clip before"
        );
    }

    #[test]
    fn images_stretch_freely() {
        let mut p = Project::default();
        let m = p.add_media(media(MediaKind::Image, 0));
        let a = p.append_clip(m).unwrap();
        assert_eq!(p.clip(a).unwrap().duration, IMAGE_DURATION);
        p.trim_end(a, 60 * SECOND);
        assert_eq!(p.clip(a).unwrap().duration, 60 * SECOND);
    }

    #[test]
    fn moving_up_creates_layers_and_prunes() {
        let (mut p, m) = project_with(4 * SECOND);
        let a = p.append_clip(m).unwrap();
        let b = p.append_clip(m).unwrap();
        p.move_clips(&[b], -2 * SECOND, 1);
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.clip(b).unwrap().start, 2 * SECOND);
        p.move_clips(&[b], 2 * SECOND, -1);
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.clip(a).unwrap().start, 0);
    }

    #[test]
    fn move_clamps_at_zero() {
        let (mut p, m) = project_with(4 * SECOND);
        let a = p.insert_clip(m, 0, SECOND).unwrap();
        p.move_clips(&[a], -10 * SECOND, -3);
        assert_eq!(p.clip(a).unwrap().start, 0);
        assert_eq!(p.slot(a).unwrap().track, 0);
    }

    #[test]
    fn stack_is_bottom_first() {
        let (mut p, m) = project_with(4 * SECOND);
        let a = p.insert_clip(m, 0, 0).unwrap();
        let b = p.insert_clip(m, 0, 0).unwrap();
        let ids: Vec<_> = p.visual_stack(SECOND).iter().map(|(c, _)| c.id).collect();
        assert_eq!(ids, vec![a, b]);
        p.tracks[1].hidden = true;
        assert_eq!(p.visual_stack(SECOND).len(), 1);
    }

    #[test]
    fn json_round_trip_and_newer_schema() {
        let (mut p, m) = project_with(4 * SECOND);
        p.append_clip(m);
        let back = Project::from_json(&p.to_json()).unwrap();
        assert_eq!(back, p);
        let newer = p.to_json().replace(&format!("\"schema\":{SCHEMA}"), "\"schema\":99");
        assert!(Project::from_json(&newer).is_err());
    }

    #[test]
    fn a_window_keeps_what_plays_in_it_unchanged() {
        let (mut p, m) = project_with(4 * SECOND);
        p.append_clip(m);
        p.append_clip(m);
        let part = p.window(SECOND, 3 * SECOND);
        assert_eq!(part.clips().count(), 1);
        let c = &part.tracks[0].clips[0];
        assert_eq!((c.start, c.source_in, c.duration), (0, 0, 4 * SECOND));
    }

    #[test]
    fn duplicate_lands_after() {
        let (mut p, m) = project_with(4 * SECOND);
        let a = p.append_clip(m).unwrap();
        let copy = p.duplicate(&[a])[0];
        assert_eq!(p.clip(copy).unwrap().start, 4 * SECOND);
        assert_eq!(p.slot(copy).unwrap().track, 0);
    }

    #[test]
    fn canvas_fit() {
        assert_eq!(fit_canvas(3840, 2160), (1920, 1080));
        assert_eq!(fit_canvas(1080, 1920), (1080, 1920));
        assert_eq!(fit_canvas(641, 361), (640, 360));
    }
}
