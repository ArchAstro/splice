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
pub const SCHEMA: u32 = 2;

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
}

fn is_default(look: &Look) -> bool {
    *look == Look::default()
}

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
    pub fn end(&self) -> Time {
        self.start + self.duration
    }

    pub fn contains(&self, t: Time) -> bool {
        self.start <= t && t < self.end()
    }

    /// The source time shown at timeline time `t`.
    pub fn source_at(&self, t: Time) -> Time {
        self.source_in + (t - self.start)
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

    fn alloc(&mut self) -> u64 {
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
    fn place(&mut self, clip: Clip, track: usize) -> usize {
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
        let clip = Clip {
            id,
            media,
            start: start.max(0),
            source_in: 0,
            duration,
            volume: 1.0,
            look: Look::default(),
            generator: None,
        };
        self.place(clip, track);
        Some(id)
    }

    /// Places a title or colour. It goes on `track` or the first free one above.
    pub fn insert_generated(&mut self, generator: Generator, track: usize, start: Time, duration: Time) -> ClipId {
        let id = ClipId(self.alloc());
        let clip = Clip {
            id,
            media: MediaId(0),
            start: start.max(0),
            source_in: 0,
            duration: duration.max(self.frame()),
            volume: 1.0,
            look: Look::default(),
            generator: Some(generator),
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
        let timed = self.media(clip.media).is_some_and(Media::is_timed);
        let previous_end = slot.index.checked_sub(1).map_or(0, |i| track.clips[i].end());
        let mut lower = previous_end.max(0);
        if timed {
            lower = lower.max(clip.start - clip.source_in);
        }
        let t = t.clamp(lower, clip.end() - min_len);
        let clip = &mut self.tracks[slot.track].clips[slot.index];
        let delta = t - clip.start;
        clip.start = t;
        clip.duration -= delta;
        if timed {
            clip.source_in += delta;
        }
    }

    /// Moves a clip's right edge to `t`.
    pub fn trim_end(&mut self, id: ClipId, t: Time) {
        let Some(slot) = self.slot(id) else { return };
        let min_len = self.frame();
        let track = &self.tracks[slot.track];
        let clip = &track.clips[slot.index];
        let next_start = track.clips.get(slot.index + 1).map_or(Time::MAX, |c| c.start);
        let mut upper = next_start;
        if let Some(media) = self.media(clip.media).filter(|m| m.is_timed()) {
            upper = upper.min(clip.start + media.duration - clip.source_in);
        }
        let t = t.clamp(clip.start + min_len, upper.max(clip.start + min_len));
        let clip = &mut self.tracks[slot.track].clips[slot.index];
        clip.duration = t - clip.start;
    }

    /// Cuts a clip in two at `t`; returns the right half.
    pub fn split(&mut self, id: ClipId, t: Time) -> Option<ClipId> {
        let min_len = self.frame();
        let slot = self.slot(id)?;
        let clip = &self.tracks[slot.track].clips[slot.index];
        if t < clip.start + min_len || t > clip.end() - min_len {
            return None;
        }
        let timed = self.media(clip.media).is_some_and(Media::is_timed);
        let new_id = ClipId(self.alloc());
        let clip = &mut self.tracks[slot.track].clips[slot.index];
        let left = t - clip.start;
        let mut right = Clip {
            id: new_id,
            media: clip.media,
            start: t,
            source_in: if timed { clip.source_in + left } else { 0 },
            duration: clip.duration - left,
            volume: clip.volume,
            look: clip.look.clone(),
            generator: clip.generator.clone(),
        };
        // The animation in stays with the left half, the one out with the right.
        right.look.enter.kind = MotionKind::None;
        clip.look.exit.kind = MotionKind::None;
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
            copies.push(clip.id);
            self.place(clip, slot.track);
        }
        copies
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
    // Schema 2 added `look` and `generator` to clips; both default, so
    // older projects need nothing. Future steps: `if schema < 3 { ... }`.
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
