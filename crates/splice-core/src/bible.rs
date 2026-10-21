//! The bible: who and what the movie keeps coming back to (its characters,
//! places, things, and its look), each with pictures and the words that must
//! stay the same from shot to shot. Beats name their cast from it, so a brief
//! for any beat (to shoot it, or to generate it) carries the same anchors.

use crate::model::Project;
use crate::story::{BeatId, Still};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(pub u64);

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Character,
    Place,
    Thing,
    /// The movie's look: light, colour, lens, grain, mood. It applies to
    /// every beat, so it is never cast.
    Look,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Character, Kind::Place, Kind::Thing, Kind::Look];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Character => "character",
            Kind::Place => "place",
            Kind::Thing => "thing",
            Kind::Look => "look",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            Kind::Character => "Characters",
            Kind::Place => "Places",
            Kind::Thing => "Things",
            Kind::Look => "Look",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        match text.trim().to_lowercase().trim_end_matches('s') {
            "character" | "person" | "people" => Some(Kind::Character),
            "place" | "location" => Some(Kind::Place),
            "thing" | "prop" | "object" => Some(Kind::Thing),
            "look" | "style" => Some(Kind::Look),
            _ => None,
        }
    }

    /// 0xRRGGBB, shared with the story's palette.
    pub fn color(self) -> u32 {
        match self {
            Kind::Character => 0xef6a55,
            Kind::Place => 0x86cfc0,
            Kind::Thing => 0xf2c14e,
            Kind::Look => 0xc792ea,
        }
    }

    /// What its words are for, shown while they are empty.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Character => "What never changes: face, hair, build, clothes",
            Kind::Place => "What never changes: the space, the light, the details",
            Kind::Thing => "What never changes: shape, colour, material, marks",
            Kind::Look => "Light, colour, lens, grain, mood",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: EntryId,
    pub kind: Kind,
    pub name: String,
    /// The continuity words: what must read the same in every shot.
    #[serde(default)]
    pub words: String,
    /// Reference pictures, the first being its face.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<Still>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bible {
    #[serde(default)]
    pub entries: Vec<Entry>,
}

impl Bible {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.kind == kind)
    }

    /// The entry `key` names: its id, or its name in any case.
    pub fn find(&self, key: &str) -> Option<&Entry> {
        let key = key.trim();
        if let Ok(n) = key.parse::<u64>()
            && let Some(e) = self.entry(EntryId(n))
        {
            return Some(e);
        }
        let lower = key.to_lowercase();
        self.entries.iter().find(|e| e.name.to_lowercase() == lower)
    }
}

impl Project {
    /// Adds an entry to the bible. Returns its id.
    pub fn add_entry(&mut self, kind: Kind, name: &str) -> EntryId {
        let id = EntryId(self.alloc());
        self.bible.entries.push(Entry {
            id,
            kind,
            name: name.to_string(),
            words: String::new(),
            pictures: Vec::new(),
        });
        id
    }

    pub fn set_entry(&mut self, id: EntryId, f: impl FnOnce(&mut Entry)) {
        if let Some(entry) = self.bible.entries.iter_mut().find(|e| e.id == id) {
            f(entry);
        }
    }

    /// Takes an entry out of the bible and out of every beat's cast.
    pub fn remove_entry(&mut self, id: EntryId) {
        self.bible.entries.retain(|e| e.id != id);
        for beat in &mut self.story.beats {
            beat.cast.retain(|c| *c != id);
        }
    }

    /// Puts an entry at position `to` among those of its kind.
    pub fn move_entry(&mut self, id: EntryId, to: usize) {
        let Some(from) = self.bible.entries.iter().position(|e| e.id == id) else {
            return;
        };
        let entry = self.bible.entries.remove(from);
        let kind = entry.kind;
        let at = self
            .bible
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == kind)
            .map(|(i, _)| i)
            .nth(to)
            .unwrap_or(self.bible.entries.len());
        self.bible.entries.insert(at, entry);
    }

    /// Whether `entry` is in `beat`'s cast; it joins or leaves.
    pub fn toggle_cast(&mut self, beat: BeatId, entry: EntryId) {
        if self.bible.entry(entry).is_none_or(|e| e.kind == Kind::Look) {
            return;
        }
        self.set_beat(beat, |b| {
            if let Some(i) = b.cast.iter().position(|c| *c == entry) {
                b.cast.remove(i);
            } else {
                b.cast.push(entry);
            }
        });
    }

    /// The beats an entry is cast in.
    pub fn cast_in(&self, entry: EntryId) -> Vec<BeatId> {
        self.story
            .beats
            .iter()
            .filter(|b| b.cast.contains(&entry))
            .map(|b| b.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::SECOND;

    #[test]
    fn entries_are_cast_into_beats_and_leave_cleanly() {
        let mut p = Project::default();
        let ada = p.add_entry(Kind::Character, "Ada");
        let look = p.add_entry(Kind::Look, "Warm dusk");
        let beat = p.add_beat(None, "Open", 4 * SECOND);
        p.toggle_cast(beat, ada);
        p.toggle_cast(beat, look);
        assert_eq!(p.story.beats[0].cast, vec![ada], "the look is never cast");
        assert_eq!(p.cast_in(ada), vec![beat]);
        assert_eq!(p.bible.find("ada").map(|e| e.id), Some(ada));
        p.remove_entry(ada);
        assert!(p.story.beats[0].cast.is_empty());
    }

    #[test]
    fn kinds_read_loosely() {
        assert_eq!(Kind::parse("People"), Some(Kind::Character));
        assert_eq!(Kind::parse("locations"), Some(Kind::Place));
        assert_eq!(Kind::parse("style"), Some(Kind::Look));
        assert_eq!(Kind::parse("weather"), None);
    }
}
