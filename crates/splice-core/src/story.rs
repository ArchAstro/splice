//! The plan for a movie: story beats laid end to end from the start of the
//! timeline, each saying what happens and what to capture, and cues at
//! moments worth marking. Beats are the edit's spine: footage laid into a
//! beat belongs to it, so moving or resizing a beat carries its footage.

use crate::model::{Clip, ClipId, Project};
use crate::time::{SECOND, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BeatId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CueId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShotId(pub u64);

/// A shot planned for a beat: what it is, and a picture of what it should
/// look like (a sketch, a screenshot, a frame from somewhere).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub id: ShotId,
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Still>,
}

/// A picture kept in the project file by content hash, like media, but not
/// in the media panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Still {
    pub hash: String,
    pub ext: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Story {
    #[serde(default)]
    pub beats: Vec<Beat>,
    #[serde(default)]
    pub cues: Vec<Cue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Beat {
    pub id: BeatId,
    pub title: String,
    /// What happens: the story, the line, the point of it.
    #[serde(default)]
    pub story: String,
    /// What to capture: the shots, the screen, the sound.
    #[serde(default)]
    pub capture: String,
    pub duration: Time,
    /// 0xRRGGBB.
    pub color: u32,
    /// Reference shots, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shots: Vec<Shot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub id: CueId,
    pub at: Time,
    pub label: String,
}

/// Colours beats take in turn.
pub const COLORS: [u32; 6] = [0xef6a55, 0xf2c14e, 0x86cfc0, 0x6c9ef8, 0xc792ea, 0xf39c6b];

/// How long a new beat runs.
pub const BEAT_DURATION: Time = 5 * SECOND;

/// How much of a beat has pictures under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    Empty,
    Partial,
    Full,
}

impl Story {
    pub fn is_empty(&self) -> bool {
        self.beats.is_empty() && self.cues.is_empty()
    }

    /// Where each beat begins.
    pub fn starts(&self) -> Vec<Time> {
        let mut at = 0;
        self.beats
            .iter()
            .map(|b| {
                let start = at;
                at += b.duration;
                start
            })
            .collect()
    }

    pub fn end(&self) -> Time {
        self.beats.iter().map(|b| b.duration).sum()
    }

    pub fn index(&self, id: BeatId) -> Option<usize> {
        self.beats.iter().position(|b| b.id == id)
    }

    /// The beat's start and end.
    pub fn span(&self, id: BeatId) -> Option<(Time, Time)> {
        let i = self.index(id)?;
        let start = self.starts()[i];
        Some((start, start + self.beats[i].duration))
    }

    /// The beat under `t`.
    pub fn beat_at(&self, t: Time) -> Option<&Beat> {
        let starts = self.starts();
        self.beats
            .iter()
            .zip(starts)
            .find(|(b, s)| t >= *s && t < s + b.duration)
            .map(|(b, _)| b)
    }
}

impl Project {
    /// Adds a beat after `after` (at the end when None). Returns its id.
    pub fn add_beat(&mut self, after: Option<BeatId>, title: &str, duration: Time) -> BeatId {
        let id = BeatId(self.alloc());
        let at = after
            .and_then(|a| self.story.index(a))
            .map_or(self.story.beats.len(), |i| i + 1);
        let color = COLORS[self.story.beats.len() % COLORS.len()];
        let beat = Beat {
            id,
            title: title.to_string(),
            story: String::new(),
            capture: String::new(),
            duration: duration.max(self.frame()),
            color,
            shots: Vec::new(),
        };
        // Beats after it make room, and their footage moves with them.
        let shift = beat.duration;
        let from = if at < self.story.beats.len() {
            Some(self.story.starts()[at])
        } else {
            None
        };
        self.story.beats.insert(at, beat);
        if let Some(from) = from {
            self.shift_clips_from(from, shift, &[]);
        }
        id
    }

    /// Clips that belong to a beat: those that start inside it.
    pub fn beat_clips(&self, id: BeatId) -> Vec<ClipId> {
        let Some((start, end)) = self.story.span(id) else {
            return Vec::new();
        };
        self.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.start >= start && c.start < end)
            .map(|c| c.id)
            .collect()
    }

    /// Moves every clip starting at or after `from` by `by` (never before
    /// zero), except `keep`.
    fn shift_clips_from(&mut self, from: Time, by: Time, keep: &[ClipId]) {
        if by == 0 {
            return;
        }
        let moving: Vec<(Clip, usize)> = self
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(i, t)| t.clips.iter().map(move |c| (c.clone(), i)))
            .filter(|(c, _)| c.start >= from && !keep.contains(&c.id))
            .collect();
        self.replace_clips(moving.into_iter().map(|(mut c, i)| {
            c.start = (c.start + by).max(0);
            (c, i)
        }));
    }

    /// Takes the given clips out and puts them back where they now say,
    /// each on its old track or the first free one above.
    fn replace_clips(&mut self, clips: impl Iterator<Item = (Clip, usize)>) {
        let clips: Vec<(Clip, usize)> = clips.collect();
        let ids: Vec<ClipId> = clips.iter().map(|(c, _)| c.id).collect();
        for track in &mut self.tracks {
            track.clips.retain(|c| !ids.contains(&c.id));
        }
        let mut clips = clips;
        clips.sort_by_key(|(c, _)| c.start);
        for (clip, track) in clips {
            self.place(clip, track);
        }
        self.prune_tracks();
    }

    /// Changes a beat's length. The beats after it, and their footage, move
    /// to keep the story end to end.
    pub fn set_beat_duration(&mut self, id: BeatId, duration: Time) {
        let Some((_, end)) = self.story.span(id) else { return };
        let duration = duration.max(self.frame());
        let i = self.story.index(id).expect("found above");
        let by = duration - self.story.beats[i].duration;
        // Footage of this beat stays; footage after it moves.
        let own = self.beat_clips(id);
        self.story.beats[i].duration = duration;
        self.shift_clips_from(end, by, &own);
    }

    /// Puts a beat at position `to` in the story, carrying every beat's
    /// footage to where its beat now starts.
    pub fn move_beat(&mut self, id: BeatId, to: usize) {
        let Some(from) = self.story.index(id) else { return };
        let to = to.min(self.story.beats.len() - 1);
        if from == to {
            return;
        }
        let before: Vec<(BeatId, Time, Vec<ClipId>)> = self
            .story
            .beats
            .iter()
            .zip(self.story.starts())
            .map(|(b, s)| (b.id, s, self.beat_clips(b.id)))
            .collect();
        let beat = self.story.beats.remove(from);
        self.story.beats.insert(to, beat);
        let after = self.story.starts();
        let mut moving = Vec::new();
        for (bid, old, clips) in before {
            let i = self.story.index(bid).expect("still here");
            let by = after[i] - old;
            if by == 0 {
                continue;
            }
            for (t, track) in self.tracks.iter().enumerate() {
                for c in track.clips.iter().filter(|c| clips.contains(&c.id)) {
                    let mut c = c.clone();
                    c.start += by;
                    moving.push((c, t));
                }
            }
        }
        self.replace_clips(moving.into_iter());
    }

    /// Takes a beat out of the story. Later beats and their footage close
    /// the gap; the beat's own footage stays where it is.
    pub fn remove_beat(&mut self, id: BeatId) {
        let Some((start, end)) = self.story.span(id) else {
            return;
        };
        let own = self.beat_clips(id);
        self.story.beats.retain(|b| b.id != id);
        self.shift_clips_from(end, start - end, &own);
    }

    /// Changes a beat's words or colour.
    pub fn set_beat(&mut self, id: BeatId, f: impl FnOnce(&mut Beat)) {
        if let Some(beat) = self.story.beats.iter_mut().find(|b| b.id == id) {
            f(beat);
        }
    }

    /// How much of a beat has pictures on visible tracks.
    pub fn coverage(&self, id: BeatId) -> Coverage {
        let Some((start, end)) = self.story.span(id) else {
            return Coverage::Empty;
        };
        let mut spans: Vec<(Time, Time)> = self
            .tracks
            .iter()
            .filter(|t| !t.hidden)
            .flat_map(|t| &t.clips)
            .filter(|c| self.source(c).is_some_and(|s| s.is_visual()))
            .filter_map(|c| {
                let (a, b) = (c.start.max(start), c.end().min(end));
                (a < b).then_some((a, b))
            })
            .collect();
        if spans.is_empty() {
            return Coverage::Empty;
        }
        spans.sort();
        let mut covered = 0;
        let mut reach = start;
        for (a, b) in spans {
            let a = a.max(reach);
            if b > a {
                covered += b - a;
                reach = b;
            }
        }
        if covered + self.frame() >= end - start {
            Coverage::Full
        } else {
            Coverage::Partial
        }
    }

    /// Lays media into a beat: at its start, as long as the beat (or the
    /// media, if shorter), on the lowest track with room.
    pub fn fill_beat(&mut self, id: BeatId, media: crate::model::MediaId) -> Option<ClipId> {
        let (start, end) = self.story.span(id)?;
        let clip = self.insert_clip(media, 0, start)?;
        let room = end - start;
        let timed = self.media(media).is_some_and(|m| m.is_timed());
        if let Some(c) = self.clip_mut(clip) {
            c.duration = if timed { c.duration.min(room) } else { room };
        }
        Some(clip)
    }

    /// Adds a reference shot to a beat. Returns its id.
    pub fn add_shot(&mut self, beat: BeatId, note: &str, image: Option<Still>) -> Option<ShotId> {
        self.story.index(beat)?;
        let id = ShotId(self.alloc());
        self.set_beat(beat, |b| {
            b.shots.push(Shot {
                id,
                note: note.to_string(),
                image,
            })
        });
        Some(id)
    }

    /// The beat a shot belongs to.
    pub fn shot_beat(&self, shot: ShotId) -> Option<BeatId> {
        self.story
            .beats
            .iter()
            .find(|b| b.shots.iter().any(|s| s.id == shot))
            .map(|b| b.id)
    }

    pub fn set_shot(&mut self, shot: ShotId, f: impl FnOnce(&mut Shot)) {
        if let Some(s) = self
            .story
            .beats
            .iter_mut()
            .flat_map(|b| b.shots.iter_mut())
            .find(|s| s.id == shot)
        {
            f(s);
        }
    }

    pub fn remove_shot(&mut self, shot: ShotId) {
        for beat in &mut self.story.beats {
            beat.shots.retain(|s| s.id != shot);
        }
    }

    /// Moves a shot to position `to` within its beat.
    pub fn move_shot(&mut self, shot: ShotId, to: usize) {
        for beat in &mut self.story.beats {
            if let Some(from) = beat.shots.iter().position(|s| s.id == shot) {
                let s = beat.shots.remove(from);
                let to = to.min(beat.shots.len());
                beat.shots.insert(to, s);
            }
        }
    }

    /// Every content hash the project needs kept: its media and the story's
    /// reference pictures.
    pub fn hashes(&self) -> Vec<String> {
        self.media
            .iter()
            .flat_map(|m| {
                let inner = match &m.recipe {
                    Some(crate::model::Recipe::Symbol(symbol)) => symbol.hashes(),
                    _ => Vec::new(),
                };
                std::iter::once(m.hash.clone()).chain(inner)
            })
            .chain(
                self.story
                    .beats
                    .iter()
                    .flat_map(|b| &b.shots)
                    .filter_map(|s| s.image.as_ref().map(|i| i.hash.clone())),
            )
            .collect()
    }

    pub fn add_cue(&mut self, at: Time, label: &str) -> CueId {
        let id = CueId(self.alloc());
        self.story.cues.push(Cue {
            id,
            at: at.max(0),
            label: label.to_string(),
        });
        self.story.cues.sort_by_key(|c| c.at);
        id
    }

    pub fn remove_cue(&mut self, id: CueId) {
        self.story.cues.retain(|c| c.id != id);
    }

    pub fn set_cue(&mut self, id: CueId, f: impl FnOnce(&mut Cue)) {
        if let Some(cue) = self.story.cues.iter_mut().find(|c| c.id == id) {
            f(cue);
        }
        self.story.cues.sort_by_key(|c| c.at);
    }

    /// The plan as plain text: each beat with its time, story and shots, then
    /// the cues. For a shot list, a script, an assistant.
    pub fn shot_list(&self) -> String {
        let fmt = |t: Time| {
            let s = t / SECOND;
            format!("{}:{:02}", s / 60, s % 60)
        };
        let mut out = String::new();
        for (n, (beat, start)) in self.story.beats.iter().zip(self.story.starts()).enumerate() {
            let state = match self.coverage(beat.id) {
                Coverage::Empty => "needs footage",
                Coverage::Partial => "partly covered",
                Coverage::Full => "covered",
            };
            out.push_str(&format!(
                "{}. {}  ({}–{}, {})\n",
                n + 1,
                if beat.title.is_empty() { "Untitled" } else { &beat.title },
                fmt(start),
                fmt(start + beat.duration),
                state
            ));
            if !beat.story.trim().is_empty() {
                out.push_str(&format!("   {}\n", beat.story.trim().replace('\n', "\n   ")));
            }
            if !beat.capture.trim().is_empty() {
                out.push_str(&format!(
                    "   Capture: {}\n",
                    beat.capture.trim().replace('\n', "\n            ")
                ));
            }
            for (i, shot) in beat.shots.iter().enumerate() {
                let note = if shot.note.trim().is_empty() {
                    "(no note)"
                } else {
                    shot.note.trim()
                };
                let picture = if shot.image.is_some() {
                    "  [reference picture]"
                } else {
                    ""
                };
                out.push_str(&format!("   Shot {}: {note}{picture}\n", i + 1));
            }
        }
        if !self.story.cues.is_empty() {
            out.push_str("\nCues\n");
            for cue in &self.story.cues {
                out.push_str(&format!("   {}  {}\n", fmt(cue.at), cue.label));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Media, MediaId, MediaKind};
    use std::path::PathBuf;

    fn project() -> (Project, MediaId) {
        let mut p = Project::default();
        let m = p.add_media(Media {
            id: MediaId(0),
            name: "a".into(),
            hash: "a".into(),
            ext: "mp4".into(),
            kind: MediaKind::Video,
            duration: 20 * SECOND,
            width: 1280,
            height: 720,
            has_audio: false,
            source: PathBuf::new(),
            recipe: None,
            content: None,
        });
        (p, m)
    }

    #[test]
    fn beats_lie_end_to_end() {
        let (mut p, _) = project();
        let a = p.add_beat(None, "Open", 3 * SECOND);
        let b = p.add_beat(None, "Problem", 4 * SECOND);
        let mid = p.add_beat(Some(a), "Turn", 2 * SECOND);
        assert_eq!(p.story.starts(), vec![0, 3 * SECOND, 5 * SECOND]);
        assert_eq!(p.story.beats[1].id, mid);
        assert_eq!(p.story.span(b), Some((5 * SECOND, 9 * SECOND)));
        assert_eq!(p.story.end(), 9 * SECOND);
        assert_eq!(p.story.beat_at(4 * SECOND).map(|b| b.id), Some(mid));
        assert_ne!(p.story.beats[0].color, p.story.beats[1].color);
    }

    #[test]
    fn footage_fills_and_follows_its_beat() {
        let (mut p, m) = project();
        let a = p.add_beat(None, "A", 3 * SECOND);
        let b = p.add_beat(None, "B", 4 * SECOND);
        let ca = p.fill_beat(a, m).unwrap();
        let cb = p.fill_beat(b, m).unwrap();
        assert_eq!(p.clip(ca).unwrap().duration, 3 * SECOND, "trimmed to the beat");
        assert_eq!(p.clip(cb).unwrap().start, 3 * SECOND);
        assert_eq!(p.coverage(a), Coverage::Full);

        // Longer A: B and its footage move later.
        p.set_beat_duration(a, 5 * SECOND);
        assert_eq!(p.clip(cb).unwrap().start, 5 * SECOND);
        assert_eq!(p.coverage(a), Coverage::Partial, "the A clip is still 3s");

        // B first: each beat's footage goes with it.
        p.move_beat(b, 0);
        assert_eq!(p.story.beats[0].id, b);
        assert_eq!(p.clip(cb).unwrap().start, 0);
        assert_eq!(p.clip(ca).unwrap().start, 4 * SECOND);

        // Remove B: A and its footage close the gap.
        p.remove_beat(b);
        assert_eq!(p.clip(ca).unwrap().start, 0);
        assert!(p.clip(cb).is_some(), "footage is never deleted with a beat");
    }

    #[test]
    fn cues_stay_in_order_and_the_plan_reads() {
        let (mut p, _) = project();
        let a = p.add_beat(None, "Hook", 2 * SECOND);
        p.set_beat(a, |b| {
            b.story = "Why this matters".into();
            b.capture = "Close on the screen".into();
        });
        p.add_cue(5 * SECOND, "music lifts");
        p.add_cue(SECOND, "VO starts");
        assert_eq!(p.story.cues[0].label, "VO starts");
        let list = p.shot_list();
        assert!(list.contains("1. Hook  (0:00–0:02, needs footage)"), "{list}");
        assert!(list.contains("Capture: Close on the screen"));
        assert!(list.contains("0:05  music lifts"));
    }

    #[test]
    fn shots_belong_to_their_beat_and_keep_their_pictures() {
        let (mut p, _) = project();
        let a = p.add_beat(None, "Hook", 2 * SECOND);
        let still = Still {
            hash: "abc".into(),
            ext: "png".into(),
            width: 10,
            height: 10,
        };
        let one = p.add_shot(a, "Wide on the desk", Some(still)).unwrap();
        let two = p.add_shot(a, "Close on the hands", None).unwrap();
        assert_eq!(p.shot_beat(two), Some(a));
        assert!(p.hashes().contains(&"abc".to_string()));
        p.move_shot(two, 0);
        assert_eq!(p.story.beats[0].shots[0].id, two);
        p.set_shot(one, |s| s.note = "Wider".into());
        assert!(
            p.shot_list().contains("Shot 2: Wider  [reference picture]"),
            "{}",
            p.shot_list()
        );
        p.remove_shot(one);
        assert_eq!(p.story.beats[0].shots.len(), 1);
        assert!(p.add_shot(BeatId(999), "x", None).is_none());
    }

    #[test]
    fn old_projects_have_no_story() {
        let json = serde_json::to_string(&Project::default()).unwrap();
        assert!(!json.contains("story"));
    }
}
