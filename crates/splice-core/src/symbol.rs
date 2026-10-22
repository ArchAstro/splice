//! Symbols: a piece of a movie kept for reuse in any project, such as an intro,
//! a lower third, a logo sting. A symbol is a small timeline of its own; each
//! use of it is one clip that plays a render of that timeline, with its
//! placeholders (clips marked with a tag) filled in for that use: other
//! words for a title, another colour, another picture or sound.

use crate::model::{Clip, ClipId, Generator, Media, MediaId, MediaKind, Project, Settings, Track, TrackId};
use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Marks a clip in a symbol as a placeholder. The id is what fills are kept
/// by, so renaming a placeholder never loses what uses put in it; the label
/// is what people see. Clips with the same id change together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "TagForm")]
pub struct Tag {
    pub id: String,
    pub label: String,
}

/// A tag as written: in full, or (in early files) just its label.
#[derive(Deserialize)]
#[serde(untagged)]
enum TagForm {
    Full { id: String, label: String },
    Label(String),
}

impl From<TagForm> for Tag {
    fn from(form: TagForm) -> Tag {
        match form {
            TagForm::Full { id, label } => Tag { id, label },
            TagForm::Label(label) => Tag {
                id: label.clone(),
                label,
            },
        }
    }
}

/// A copy of a symbol, as one use of it in a project holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    /// Which library symbol this is, whatever it is called now.
    #[serde(default)]
    pub id: String,
    /// Its name when this copy was taken.
    pub name: String,
    /// Which library version this copy is (`version_of` its timeline).
    pub version: String,
    /// Fit to the canvas of the movie using it (laid out afresh at that
    /// shape) rather than keeping its own shape inside it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reflow: bool,
    /// Its own timeline, canvas and media. The media's bytes are kept in the
    /// project file that uses it, like any other media.
    pub project: Box<Project>,
    /// How long this use runs, when stretched or shrunk from the symbol's own
    /// length (through its hold).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<Time>,
    /// What this use puts in each placeholder, by placeholder id. A
    /// placeholder not listed keeps what the symbol has.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fills: BTreeMap<String, Fill>,
}

/// What goes in a placeholder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Fill {
    Text {
        text: String,
    },
    Color {
        color: u32,
    },
    /// A picture, video or sound from the project using the symbol.
    Media {
        media: Box<Media>,
    },
}

/// What kind of thing a placeholder takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Takes {
    Text,
    Color,
    /// A still or moving picture.
    Picture,
    Sound,
}

impl Takes {
    pub fn name(self) -> &'static str {
        match self {
            Takes::Text => "text",
            Takes::Color => "color",
            Takes::Picture => "picture",
            Takes::Sound => "sound",
        }
    }

    /// Whether media of `kind` can go in.
    pub fn accepts(self, kind: MediaKind) -> bool {
        match self {
            Takes::Picture => kind != MediaKind::Audio,
            Takes::Sound => kind == MediaKind::Audio,
            Takes::Text | Takes::Color => false,
        }
    }
}

/// A placeholder as the symbol defines it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Placeholder {
    pub id: String,
    pub label: String,
    pub takes: Takes,
    /// What the symbol itself has there.
    pub default: Fill,
}

impl Fill {
    pub fn takes(&self) -> Takes {
        match self {
            Fill::Text { .. } => Takes::Text,
            Fill::Color { .. } => Takes::Color,
            Fill::Media { media } if media.kind == MediaKind::Audio => Takes::Sound,
            Fill::Media { .. } => Takes::Picture,
        }
    }
}

/// A fill an update could not keep, because its placeholder is gone or now
/// takes something else.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dropped {
    pub label: String,
    pub fill: Fill,
}

/// Names one version of a symbol's timeline.
pub fn version_of(project: &Project) -> String {
    blake3::hash(project.to_json().as_bytes()).to_hex()[..16].to_string()
}

/// What a placeholder clip in `project` takes, and what it has now.
fn current(project: &Project, clip: &Clip) -> Option<Fill> {
    match &clip.generator {
        Some(Generator::Text(title)) => Some(Fill::Text {
            text: title.text.clone(),
        }),
        Some(Generator::Color { color }) => Some(Fill::Color { color: *color }),
        None => project.media(clip.media).map(|m| Fill::Media {
            media: Box::new(m.clone()),
        }),
    }
}

impl Symbol {
    pub fn new(id: &str, name: &str, project: Project) -> Symbol {
        Symbol {
            id: id.to_string(),
            name: name.to_string(),
            version: version_of(&project),
            reflow: false,
            length: None,
            project: Box::new(project),
            fills: BTreeMap::new(),
        }
    }

    /// What a use of it is called: its name, and the first words filled in,
    /// so uses tell themselves apart ("Lower third · Grace Hopper").
    pub fn title(&self) -> String {
        let words = self
            .placeholders()
            .into_iter()
            .find_map(|p| match self.fills.get(&p.id) {
                Some(Fill::Text { text }) => text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim().to_string()),
                _ => None,
            });
        match words {
            Some(words) => format!("{} · {words}", self.name),
            None => self.name.clone(),
        }
    }

    /// Its placeholders, bottom track first, each once.
    pub fn placeholders(&self) -> Vec<Placeholder> {
        let mut out: Vec<Placeholder> = Vec::new();
        for (_, clip) in self.project.clips() {
            let Some(tag) = &clip.placeholder else { continue };
            if out.iter().any(|p| p.id == tag.id) {
                continue;
            }
            if let Some(default) = current(&self.project, clip) {
                out.push(Placeholder {
                    id: tag.id.clone(),
                    label: tag.label.clone(),
                    takes: default.takes(),
                    default,
                });
            }
        }
        out
    }

    /// The placeholder `key` names: its id, or its label in any case.
    pub fn placeholder(&self, key: &str) -> Option<Placeholder> {
        let all = self.placeholders();
        let lower = key.to_lowercase();
        all.iter()
            .find(|p| p.id == key)
            .or_else(|| all.iter().find(|p| p.label.to_lowercase() == lower))
            .cloned()
    }

    /// What placeholder `id` shows in this use.
    pub fn shown(&self, placeholder: &Placeholder) -> Fill {
        self.fills
            .get(&placeholder.id)
            .cloned()
            .unwrap_or_else(|| placeholder.default.clone())
    }

    /// Puts `fill` in the placeholder `key` (its id or label), if it takes
    /// that kind of thing.
    pub fn fill(&mut self, key: &str, fill: Fill) -> anyhow::Result<()> {
        let Some(placeholder) = self.placeholder(key) else {
            let labels: Vec<String> = self.placeholders().into_iter().map(|p| p.label).collect();
            anyhow::bail!(
                "{} has no placeholder called {key:?} (it has: {})",
                self.name,
                if labels.is_empty() {
                    "none".into()
                } else {
                    labels.join(", ")
                }
            );
        };
        if placeholder.takes != fill.takes() {
            anyhow::bail!(
                "{:?} takes a {}, not a {}",
                placeholder.label,
                placeholder.takes.name(),
                fill.takes().name()
            );
        }
        if fill == placeholder.default {
            self.fills.remove(&placeholder.id);
        } else {
            self.fills.insert(placeholder.id, fill);
        }
        Ok(())
    }

    /// Puts back what the symbol has in placeholder `key`.
    pub fn unfill(&mut self, key: &str) -> anyhow::Result<()> {
        let placeholder = self
            .placeholder(key)
            .ok_or_else(|| anyhow::anyhow!("{} has no placeholder called {key:?}", self.name))?;
        self.fills.remove(&placeholder.id);
        Ok(())
    }

    /// The stretch a longer or shorter use stretches: the symbol's hold,
    /// else all of it.
    pub fn hold(&self) -> (Time, Time) {
        let whole = self.project.duration();
        match self.project.hold {
            Some((a, b)) if 0 <= a && a < b && b <= whole => (a, b),
            _ => (0, whole),
        }
    }

    /// How long this use runs.
    pub fn length(&self) -> Time {
        self.length.unwrap_or_else(|| self.project.duration())
    }

    /// Makes this use run `length`, as near as its hold allows: the hold can
    /// shrink to a frame, and grow without end.
    pub fn set_length(&mut self, length: Time) {
        let whole = self.project.duration();
        let (a, b) = self.hold();
        let shortest = whole - (b - a) + self.project.frame();
        let length = length.max(shortest);
        self.length = (length != whole).then_some(length);
    }

    /// Where symbol time `t` falls in this use's (stretched) time.
    fn retime(&self, t: Time) -> Time {
        let whole = self.project.duration();
        let (a, b) = self.hold();
        let delta = self.length() - whole;
        if delta == 0 || t <= a {
            t
        } else if t >= b {
            t + delta
        } else {
            let span = (b - a) as i128;
            a + ((t - a) as i128 * (span + delta as i128) / span) as Time
        }
    }

    /// The symbol's timeline with this use's fills in place and its length
    /// applied: what renders.
    pub fn filled(&self) -> Project {
        let mut project = (*self.project).clone();
        // Media fills join the symbol's media, once each.
        let mut added: BTreeMap<String, MediaId> = BTreeMap::new();
        for fill in self.fills.values() {
            if let Fill::Media { media } = fill
                && !added.contains_key(&media.hash)
            {
                let id = project.add_media((**media).clone());
                added.insert(media.hash.clone(), id);
            }
        }
        for track in &mut project.tracks {
            for clip in &mut track.clips {
                let Some(fill) = clip.placeholder.as_ref().and_then(|t| self.fills.get(&t.id)) else {
                    continue;
                };
                match (fill, &mut clip.generator) {
                    (Fill::Text { text }, Some(Generator::Text(title))) => title.text = text.clone(),
                    (Fill::Color { color }, Some(Generator::Color { color: c })) => *c = *color,
                    (Fill::Media { media }, None) => {
                        clip.media = added[&media.hash];
                        clip.source_in = 0;
                    }
                    _ => {}
                }
            }
        }
        if self.length.is_some() {
            let frame = project.frame();
            for track in &mut project.tracks {
                for clip in &mut track.clips {
                    let (start, end) = (self.retime(clip.start), self.retime(clip.end()));
                    clip.start = start;
                    clip.duration = (end - start).max(frame);
                }
            }
            project.hold = None;
        }
        project
    }

    /// The canvas a use renders at in a movie of `host`'s canvas: the host's
    /// own when it reflows, else its own shape fitted inside the host's at the
    /// host's resolution, so it stays sharp. Always at the host's frame rate.
    pub fn render_settings(&self, host: Settings) -> Settings {
        let own = self.project.settings;
        if self.reflow || own.width == 0 || own.height == 0 {
            return Settings { fps: host.fps, ..host };
        }
        let fit = (host.width as f64 / own.width as f64).min(host.height as f64 / own.height as f64);
        let even = |v: f64| ((v.round() as u32).max(2) / 2) * 2;
        Settings {
            width: even(own.width as f64 * fit),
            height: even(own.height as f64 * fit),
            fps: host.fps,
        }
    }

    /// What a use renders in a movie of canvas `host`: its filled timeline at
    /// `render_settings`. Laid out afresh for a narrower movie, its titles
    /// shrink with the width, so words that fit across it still do (a title's
    /// size follows the canvas height, which would otherwise blow them up).
    pub fn rendered(&self, host: Settings) -> Project {
        let mut project = self.filled();
        let own = self.project.settings;
        let at = self.render_settings(host);
        if self.reflow && own.width > 0 && own.height > 0 && at.height > 0 {
            let narrower = (at.width as f32 / at.height as f32) / (own.width as f32 / own.height as f32);
            if narrower < 1.0 {
                for track in &mut project.tracks {
                    for clip in &mut track.clips {
                        if let Some(Generator::Text(title)) = &mut clip.generator {
                            title.size *= narrower;
                        }
                    }
                }
            }
        }
        project.settings = at;
        project
    }

    /// Whether its shape differs from `host`'s, so how it fits matters.
    pub fn shape_differs(&self, host: Settings) -> bool {
        let own = self.project.settings;
        (own.width as u64 * host.height as u64) != (own.height as u64 * host.width as u64)
    }

    /// The hashes of the bytes it needs: its own media, recursively, and what fills it.
    pub fn hashes(&self) -> Vec<String> {
        let mut hashes = self.project.hashes();
        for fill in self.fills.values() {
            if let Fill::Media { media } = fill {
                hashes.push(media.hash.clone());
            }
        }
        hashes
    }

    /// Whether anything in it makes sound.
    pub fn has_audio(&self) -> bool {
        !self.filled().audible_clips().is_empty()
    }

    /// Whether it uses the symbol `id`, at any depth: putting that symbol's
    /// use inside it would make a loop.
    pub fn uses(&self, id: &str) -> bool {
        self.id == id || self.project.uses_symbol(id)
    }

    /// Takes the library's newer version, keeping the fills that still fit,
    /// and returns the ones that do not.
    pub fn update(&mut self, project: Project, name: &str) -> Vec<Dropped> {
        let old: BTreeMap<String, String> = self.placeholders().into_iter().map(|p| (p.id, p.label)).collect();
        let fills = std::mem::take(&mut self.fills);
        *self.project = project;
        self.name = name.to_string();
        self.version = version_of(&self.project);
        // The use keeps its length, as far as the new hold allows.
        if let Some(length) = self.length {
            self.set_length(length);
        }
        let mut dropped = Vec::new();
        for (id, fill) in fills {
            let fits = self
                .placeholders()
                .iter()
                .any(|p| p.id == id && p.takes == fill.takes());
            if fits {
                self.fills.insert(id, fill);
            } else {
                dropped.push(Dropped {
                    label: old.get(&id).cloned().unwrap_or(id),
                    fill,
                });
            }
        }
        dropped
    }

    /// The fills an update to `project` would drop, without taking it.
    pub fn would_drop(&self, project: &Project) -> Vec<Dropped> {
        let mut copy = self.clone();
        copy.update(project.clone(), &self.name)
    }
}

/// Labels a new placeholder by what it is: "Title", "Title 2", "Colour",
/// "Picture", "Sound", unique among `taken`.
fn role_label(project: &Project, clip: &Clip, taken: &[String]) -> String {
    let base = match &clip.generator {
        Some(Generator::Text(_)) => "Title",
        Some(Generator::Color { .. }) => "Colour",
        None => match project.media(clip.media).map(|m| m.kind) {
            Some(MediaKind::Audio) => "Sound",
            _ => "Picture",
        },
    };
    let mut label = base.to_string();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&label)) {
        label = format!("{base} {n}");
        n += 1;
    }
    label
}

impl Project {
    /// Whether any media here is a use of the symbol `id`, at any depth.
    pub fn uses_symbol(&self, id: &str) -> bool {
        self.media.iter().any(|m| match &m.recipe {
            Some(crate::model::Recipe::Symbol(s)) => s.uses(id),
            _ => false,
        })
    }

    /// The labels of the placeholders marked here.
    fn labels(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, clip) in self.clips() {
            if let Some(tag) = &clip.placeholder
                && !out.contains(&tag.label)
            {
                out.push(tag.label.clone());
            }
        }
        out
    }

    /// A new timeline of just `clips`, moved to start at zero, on the same
    /// canvas and keeping their layering. Titles become placeholders ("Title",
    /// "Title 2"), so a symbol made from them can be retitled at once.
    pub fn extract(&self, clips: &[ClipId]) -> Option<Project> {
        self.extract_with(clips, &[])
    }

    /// `extract`, leaving the titles in `plain` as they are rather than
    /// making them placeholders.
    pub fn extract_with(&self, clips: &[ClipId], plain: &[ClipId]) -> Option<Project> {
        let start = self
            .clips()
            .filter(|(_, c)| clips.contains(&c.id))
            .map(|(_, c)| c.start)
            .min()?;
        let mut out = Project::default();
        out.settings = self.settings;
        out.tracks.clear();
        let mut ids: BTreeMap<MediaId, MediaId> = BTreeMap::new();
        let mut kept: Vec<ClipId> = Vec::new();
        for track in &self.tracks {
            let picked: Vec<&Clip> = track.clips.iter().filter(|c| clips.contains(&c.id)).collect();
            if picked.is_empty() {
                continue;
            }
            let mut copy = Track {
                id: TrackId(out.alloc()),
                clips: Vec::new(),
                muted: track.muted,
                hidden: track.hidden,
            };
            for clip in picked {
                let original = clip.id;
                let mut clip = clip.clone();
                clip.id = ClipId(out.alloc());
                if plain.contains(&original) {
                    kept.push(clip.id);
                }
                clip.start -= start;
                if clip.generator.is_none() {
                    let media = self.media(clip.media)?.clone();
                    clip.media = *ids.entry(media.id).or_insert_with(|| out.add_media(media));
                }
                copy.clips.push(clip);
            }
            out.tracks.push(copy);
        }
        // Titles become placeholders, in layering order.
        let titles: Vec<ClipId> = out
            .clips()
            .filter(|(_, c)| {
                matches!(c.generator, Some(Generator::Text(_))) && c.placeholder.is_none() && !kept.contains(&c.id)
            })
            .map(|(_, c)| c.id)
            .collect();
        for id in titles {
            out.set_placeholder(id, Some(""));
        }
        Some(out)
    }

    /// Takes `clips` out and puts one clip of `media` where they began: on
    /// the lowest of their tracks with room, else on a new track just above
    /// the highest of them, so nothing that was above them ends up below.
    pub fn replace_with(&mut self, clips: &[ClipId], media: MediaId, duration: Time) -> Option<ClipId> {
        let (low, high, start) = self
            .clips()
            .filter(|(_, c)| clips.contains(&c.id))
            .map(|(t, c)| (t, t, c.start))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2)))?;
        for t in &mut self.tracks {
            t.clips.retain(|c| !clips.contains(&c.id));
        }
        let id = ClipId(self.alloc());
        let clip = Clip::new(id, media, start, 0, duration);
        let end = start + duration;
        let free = (low..=high).find(|&i| self.tracks[i].clips.iter().all(|c| c.end() <= start || c.start >= end));
        let index = match free {
            Some(i) => i,
            None => {
                let track = Track {
                    id: TrackId(self.alloc()),
                    clips: Vec::new(),
                    muted: false,
                    hidden: false,
                };
                self.tracks.insert(high + 1, track);
                high + 1
            }
        };
        let at = self.tracks[index].clips.partition_point(|c| c.start <= start);
        self.tracks[index].clips.insert(at, clip);
        self.prune_tracks();
        Some(id)
    }

    /// Marks a clip as a placeholder labelled `label` (an empty label picks
    /// one by what the clip is), or unmarks it with `None`. A label another
    /// placeholder has joins it, so they change together; relabelling a
    /// placeholder renames it everywhere and keeps what uses put in it.
    pub fn set_placeholder(&mut self, id: ClipId, label: Option<&str>) {
        let Some(clip) = self.clip(id).cloned() else { return };
        let Some(label) = label.map(str::trim) else {
            if let Some(c) = self.clip_mut(id) {
                c.placeholder = None;
            }
            return;
        };
        let mut taken = self.labels();
        if let Some(own) = &clip.placeholder {
            taken.retain(|l| l != &own.label);
        }
        let label = if label.is_empty() {
            match &clip.placeholder {
                Some(own) => own.label.clone(),
                None => role_label(self, &clip, &taken),
            }
        } else {
            label.to_string()
        };
        // Joining another placeholder of the same name and kind.
        let kind = current(self, &clip).map(|f| f.takes());
        let join = self.clips().find_map(|(_, c)| {
            let tag = c.placeholder.as_ref()?;
            (c.id != id && tag.label.eq_ignore_ascii_case(&label) && current(self, c).map(|f| f.takes()) == kind)
                .then(|| tag.clone())
        });
        let tag = match (join, &clip.placeholder) {
            (Some(other), _) => other,
            (None, Some(own)) => Tag {
                id: own.id.clone(),
                label: label.clone(),
            },
            (None, None) => Tag {
                id: format!("p{}", self.alloc()),
                label: label.clone(),
            },
        };
        // A rename reaches every clip of the placeholder.
        let old = clip.placeholder.as_ref().map(|t| t.id.clone());
        for track in &mut self.tracks {
            for c in &mut track.clips {
                let same = c.id == id || (old.is_some() && c.placeholder.as_ref().map(|t| &t.id) == old.as_ref());
                if same {
                    c.placeholder = Some(tag.clone());
                }
            }
        }
    }

    /// Clones media under a new id, for a use of a symbol that should get
    /// its own fills.
    pub fn fork_media(&mut self, id: MediaId) -> Option<MediaId> {
        let mut media = self.media(id)?.clone();
        media.id = MediaId(self.alloc());
        let new = media.id;
        self.media.push(media);
        Some(new)
    }

    /// Whether `media` is a use of a symbol that some clip already shows.
    pub(crate) fn is_used_symbol(&self, media: MediaId) -> bool {
        self.media(media)
            .is_some_and(|m| matches!(m.recipe, Some(crate::model::Recipe::Symbol(_))))
            && self.clips().any(|(_, c)| c.media == media && c.generator.is_none())
    }

    /// Puts the clips of a use of a symbol back on the timeline as they are
    /// in it (its fills in place, its trim kept), replacing the use. Each of
    /// its tracks gets a new track just above the use's. Returns the clips.
    pub fn break_apart(&mut self, use_clip: ClipId) -> Vec<ClipId> {
        let Some(slot) = self.slot(use_clip) else {
            return Vec::new();
        };
        let clip = self.tracks[slot.track].clips[slot.index].clone();
        let Some(crate::model::Recipe::Symbol(symbol)) = self.media(clip.media).and_then(|m| m.recipe.clone()) else {
            return Vec::new();
        };
        let inner = symbol.filled();
        let (from, to) = (clip.source_in, clip.source_in + clip.duration);
        let shift = clip.start - clip.source_in;
        self.tracks[slot.track].clips.remove(slot.index);
        let mut ids: BTreeMap<MediaId, MediaId> = BTreeMap::new();
        let mut made = Vec::new();
        let mut at = slot.track + 1;
        for track in &inner.tracks {
            let mut copy = Track {
                id: TrackId(self.alloc()),
                clips: Vec::new(),
                muted: track.muted,
                hidden: track.hidden,
            };
            for c in &track.clips {
                let (start, end) = (c.start.max(from), c.end().min(to));
                if end <= start {
                    continue;
                }
                let mut c = c.clone();
                let timed = c.generator.is_none() && inner.media(c.media).is_some_and(Media::is_timed);
                if timed {
                    c.source_in += start - c.start;
                }
                c.start = start + shift;
                c.duration = end - start;
                c.id = ClipId(self.alloc());
                c.placeholder = None;
                if c.generator.is_none() {
                    let Some(media) = inner.media(c.media).cloned() else {
                        continue;
                    };
                    c.media = *ids.entry(media.id).or_insert_with(|| self.add_media(media));
                }
                made.push(c.id);
                copy.clips.push(c);
            }
            if !copy.clips.is_empty() {
                self.tracks.insert(at, copy);
                at += 1;
            }
        }
        // The use's media goes when nothing shows it any more.
        if !self
            .clips()
            .any(|(_, c)| c.media == clip.media && c.generator.is_none())
        {
            self.media.retain(|m| m.id != clip.media);
        }
        self.prune_tracks();
        made
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Title;
    use crate::time::SECOND;

    fn logo() -> Media {
        Media {
            id: MediaId(0),
            name: "logo.png".into(),
            hash: "logo".into(),
            ext: "png".into(),
            kind: MediaKind::Image,
            duration: 0,
            width: 400,
            height: 400,
            has_audio: false,
            source: Default::default(),
            recipe: None,
            content: None,
        }
    }

    fn tone() -> Media {
        Media {
            hash: "tone".into(),
            kind: MediaKind::Audio,
            duration: 2 * SECOND,
            has_audio: true,
            ..logo()
        }
    }

    /// A lower third: a bar, a name over it, a logo.
    fn host() -> (Project, Vec<ClipId>) {
        let mut p = Project::default();
        let bar = p.insert_generated(Generator::Color { color: 0x202020ff }, 0, 2 * SECOND, 4 * SECOND);
        let name = p.insert_generated(
            Generator::Text(Title {
                text: "Ada Lovelace\nDirector".into(),
                ..Title::default()
            }),
            1,
            2 * SECOND,
            4 * SECOND,
        );
        let logo = p.add_media(logo());
        let mark = p.insert_clip(logo, 2, 3 * SECOND).unwrap();
        (p, vec![bar, name, mark])
    }

    fn lower_third() -> Symbol {
        let (host, clips) = host();
        let mut piece = host.extract(&clips).unwrap();
        let mark = piece.tracks[2].clips[0].id;
        piece.set_placeholder(mark, Some(""));
        Symbol::new("sym1", "Lower third", piece)
    }

    #[test]
    fn a_selection_becomes_a_symbol_with_its_titles_as_placeholders() {
        let (host, clips) = host();
        let piece = host.extract(&clips).unwrap();
        assert_eq!(piece.tracks.len(), 3, "layering kept");
        assert_eq!(piece.tracks[0].clips[0].start, 0, "moved to start at zero");
        assert_eq!(piece.tracks[2].clips[0].start, SECOND);
        assert_eq!(piece.media.len(), 1);
        let symbol = Symbol::new("sym1", "Lower third", piece);
        let holes = symbol.placeholders();
        assert_eq!(holes.len(), 1);
        assert_eq!(holes[0].label, "Title");
        assert_eq!(holes[0].takes, Takes::Text);
    }

    #[test]
    fn titles_can_stay_plain() {
        let (host, clips) = host();
        let piece = host.extract_with(&clips, &[clips[1]]).unwrap();
        assert!(Symbol::new("s", "S", piece).placeholders().is_empty());
    }

    #[test]
    fn placeholders_are_named_by_role() {
        let symbol = lower_third();
        let labels: Vec<_> = symbol.placeholders().into_iter().map(|p| (p.label, p.takes)).collect();
        assert_eq!(
            labels,
            vec![
                ("Title".to_string(), Takes::Text),
                ("Picture".to_string(), Takes::Picture)
            ]
        );
    }

    #[test]
    fn fills_change_only_their_use_and_must_fit() {
        let mut symbol = lower_third();
        symbol
            .fill(
                "title",
                Fill::Text {
                    text: "Grace Hopper".into(),
                },
            )
            .unwrap();
        let other = Media {
            hash: "other".into(),
            ..logo()
        };
        symbol.fill("Picture", Fill::Media { media: Box::new(other) }).unwrap();
        let err = symbol
            .fill(
                "Picture",
                Fill::Media {
                    media: Box::new(tone()),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("takes a picture, not a sound"), "{err}");
        assert!(symbol.fill("Picture", Fill::Color { color: 0 }).is_err());
        assert!(symbol.fill("Nope", Fill::Color { color: 0 }).is_err());

        let filled = symbol.filled();
        let Some(Generator::Text(title)) = &filled.tracks[1].clips[0].generator else {
            panic!("a title")
        };
        assert_eq!(title.text, "Grace Hopper");
        assert_eq!(symbol.title(), "Lower third · Grace Hopper");
        let shown = filled.media(filled.tracks[2].clips[0].media).unwrap();
        assert_eq!(shown.hash, "other");
        assert!(symbol.hashes().contains(&"other".to_string()));
        // Filling back what it has clears the fill; so does unfill.
        symbol
            .fill(
                "Title",
                Fill::Text {
                    text: "Ada Lovelace\nDirector".into(),
                },
            )
            .unwrap();
        assert_eq!(symbol.fills.len(), 1);
        symbol.unfill("Picture").unwrap();
        assert!(symbol.fills.is_empty());
    }

    #[test]
    fn renaming_a_placeholder_keeps_fills() {
        let mut symbol = lower_third();
        symbol.fill("Title", Fill::Text { text: "Grace".into() }).unwrap();
        let mut newer = (*symbol.project).clone();
        let name = newer.tracks[1].clips[0].id;
        newer.set_placeholder(name, Some("Name"));
        let dropped = symbol.update(newer.clone(), "Lower third");
        assert!(dropped.is_empty());
        assert_eq!(
            symbol.placeholder("Name").map(|p| symbol.shown(&p)),
            Some(Fill::Text { text: "Grace".into() })
        );
        // Unmarking it drops the fill, and says so.
        newer.set_placeholder(name, None);
        let dropped = symbol.update(newer, "Lower third");
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].label, "Name");
        assert!(symbol.fills.is_empty());
    }

    #[test]
    fn same_label_links_placeholders_and_rename_reaches_both() {
        let mut p = Project::default();
        let a = p.insert_generated(Generator::Text(Title::default()), 0, 0, SECOND);
        let b = p.insert_generated(Generator::Text(Title::default()), 1, 0, SECOND);
        p.set_placeholder(a, Some("Name"));
        p.set_placeholder(b, Some("name"));
        assert_eq!(p.clip(a).unwrap().placeholder, p.clip(b).unwrap().placeholder);
        p.set_placeholder(a, Some("Who"));
        assert_eq!(p.clip(b).unwrap().placeholder.as_ref().unwrap().label, "Who");
        // An empty label keeps the one it has.
        p.set_placeholder(a, Some(" "));
        assert_eq!(p.clip(a).unwrap().placeholder.as_ref().unwrap().label, "Who");
    }

    #[test]
    fn early_labels_read_as_tags() {
        let tag: Tag = serde_json::from_str("\"Logo\"").unwrap();
        assert_eq!(
            tag,
            Tag {
                id: "Logo".into(),
                label: "Logo".into()
            }
        );
    }

    #[test]
    fn the_selection_is_replaced_without_rising_above_what_was_over_it() {
        let (mut host, clips) = host();
        // Something above the selection, overlapping it.
        let over = host.insert_generated(Generator::Color { color: 0xff }, 3, 2 * SECOND, 4 * SECOND);
        // Something on the lowest track that the use would overlap.
        let media = host.add_media(Media {
            hash: "render".into(),
            kind: MediaKind::Video,
            duration: 4 * SECOND,
            ..logo()
        });
        let c = host.replace_with(&clips, media, 4 * SECOND).unwrap();
        let use_track = host.slot(c).unwrap().track;
        let over_track = host.slot(over).unwrap().track;
        assert!(use_track < over_track);
        assert_eq!(host.clip(c).unwrap().start, 2 * SECOND);
    }

    #[test]
    fn a_shared_use_forks_when_placed_again_or_duplicated() {
        let mut p = Project::default();
        let symbol = lower_third();
        let media = p.add_media(Media {
            hash: "render".into(),
            kind: MediaKind::Video,
            duration: 4 * SECOND,
            recipe: Some(crate::model::Recipe::Symbol(symbol)),
            ..logo()
        });
        let first = p.insert_clip(media, 0, 0).unwrap();
        let second = p.insert_clip(media, 0, 5 * SECOND).unwrap();
        assert_ne!(p.clip(first).unwrap().media, p.clip(second).unwrap().media);
        let copy = p.duplicate(&[first])[0];
        assert_ne!(p.clip(first).unwrap().media, p.clip(copy).unwrap().media);
        // A split keeps one use.
        let right = p.split(first, 2 * SECOND).unwrap();
        assert_eq!(p.clip(first).unwrap().media, p.clip(right).unwrap().media);
    }

    #[test]
    fn uses_render_sharp_in_the_hosts_shape() {
        let mut symbol = lower_third();
        symbol.project.settings = Settings {
            width: 1280,
            height: 720,
            fps: 30,
        };
        let vertical = Settings {
            width: 1080,
            height: 1920,
            fps: 25,
        };
        assert!(symbol.shape_differs(vertical));
        assert_eq!(
            symbol.render_settings(vertical),
            Settings {
                width: 1080,
                height: 608,
                fps: 25
            }
        );
        symbol.reflow = true;
        assert_eq!(symbol.render_settings(vertical), vertical);
    }

    #[test]
    fn breaking_apart_puts_the_filled_clips_back() {
        let mut p = Project::default();
        let mut symbol = lower_third();
        symbol.fill("Title", Fill::Text { text: "Grace".into() }).unwrap();
        let media = p.add_media(Media {
            hash: "render".into(),
            kind: MediaKind::Video,
            duration: 4 * SECOND,
            recipe: Some(crate::model::Recipe::Symbol(symbol)),
            ..logo()
        });
        let c = p.insert_clip(media, 0, 10 * SECOND).unwrap();
        let clips = p.break_apart(c);
        assert_eq!(clips.len(), 3);
        assert!(p.media(media).is_none(), "the use is gone");
        let words = p.clips().find_map(|(_, c)| match &c.generator {
            Some(Generator::Text(t)) => Some((t.text.clone(), c.start)),
            _ => None,
        });
        assert_eq!(words, Some(("Grace".to_string(), 10 * SECOND)));
    }

    #[test]
    fn a_use_stretches_through_its_hold() {
        let mut p = Project::default();
        // In 0–1 s, hold 1–3 s, out 3–4 s.
        let title = p.insert_generated(Generator::Text(Title::default()), 0, 0, 4 * SECOND);
        let sting = p.insert_generated(Generator::Color { color: 0xff }, 1, 3 * SECOND, SECOND);
        p.hold = Some((SECOND, 3 * SECOND));
        let mut symbol = Symbol::new("s", "Card", p);
        symbol.set_length(6 * SECOND);
        assert_eq!(symbol.length(), 6 * SECOND);
        let out = symbol.filled();
        assert_eq!(
            out.clip(title).unwrap().duration,
            6 * SECOND,
            "spans the hold, so stretches"
        );
        assert_eq!(out.clip(sting).unwrap().start, 5 * SECOND, "after the hold, so moves");
        assert_eq!(out.clip(sting).unwrap().duration, SECOND, "and keeps its own length");
        // It can shrink only as far as the hold allows.
        symbol.set_length(SECOND);
        assert_eq!(symbol.length(), 2 * SECOND + symbol.project.frame());
        symbol.set_length(4 * SECOND);
        assert_eq!(symbol.length, None, "its own length is no stretch");
    }

    #[test]
    fn trimming_a_whole_use_retimes_it() {
        let mut p = Project::default();
        let symbol = lower_third();
        let base = symbol.project.duration();
        let media = p.add_media(Media {
            hash: "render".into(),
            kind: MediaKind::Video,
            duration: base,
            recipe: Some(crate::model::Recipe::Symbol(symbol)),
            ..logo()
        });
        let clip = p.insert_clip(media, 0, 0).unwrap();
        p.trim_end(clip, base + 2 * SECOND);
        assert_eq!(p.clip(clip).unwrap().duration, base + 2 * SECOND);
        let Some(crate::model::Recipe::Symbol(s)) = &p.media(media).unwrap().recipe else {
            panic!()
        };
        assert_eq!(s.length(), base + 2 * SECOND);
    }

    #[test]
    fn loops_are_found() {
        let inner = lower_third();
        let mut outer_project = Project::default();
        outer_project.add_media(Media {
            recipe: Some(crate::model::Recipe::Symbol(inner)),
            ..logo()
        });
        let outer = Symbol::new("sym2", "Outer", outer_project);
        assert!(outer.uses("sym1"));
        assert!(outer.uses("sym2"));
        assert!(!outer.uses("sym3"));
    }
}
