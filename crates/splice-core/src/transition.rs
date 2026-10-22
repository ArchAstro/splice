//! Transitions: how one clip gives way to the next at a cut.
//!
//! A transition belongs to the clip coming in. With a clip just before it on
//! the same track, it is centred on the cut: the clip going out plays on past
//! its end and the one coming in starts before its beginning, each for half
//! the transition (frames beyond the media hold still). With nothing before,
//! the clip comes in over whatever is underneath during its first moments.
//! A dip to a colour needs no extra frames: the colour rises over the clip
//! going out and falls away from the one coming in.
//!
//! Every effect is an expression over clip time, so the viewer and export
//! compute the same thing (`compose`).

use crate::model::{Clip, ClipId, Project};
use crate::time::{SECOND, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// One picture melts into the next.
    #[default]
    Dissolve,
    /// Down to black, and up into the next.
    DipBlack,
    /// Up to white, and down into the next.
    DipWhite,
    /// An edge sweeps across, the new picture behind it. Named for the way it travels.
    WipeLeft,
    WipeRight,
    WipeUp,
    WipeDown,
    /// The new picture pushes the old one off.
    PushLeft,
    PushRight,
    PushUp,
    PushDown,
    /// The old picture rushes forward as the new one appears.
    Zoom,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Dissolve,
        Kind::DipBlack,
        Kind::DipWhite,
        Kind::WipeLeft,
        Kind::WipeRight,
        Kind::WipeUp,
        Kind::WipeDown,
        Kind::PushLeft,
        Kind::PushRight,
        Kind::PushUp,
        Kind::PushDown,
        Kind::Zoom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Dissolve => "dissolve",
            Kind::DipBlack => "dip-black",
            Kind::DipWhite => "dip-white",
            Kind::WipeLeft => "wipe-left",
            Kind::WipeRight => "wipe-right",
            Kind::WipeUp => "wipe-up",
            Kind::WipeDown => "wipe-down",
            Kind::PushLeft => "push-left",
            Kind::PushRight => "push-right",
            Kind::PushUp => "push-up",
            Kind::PushDown => "push-down",
            Kind::Zoom => "zoom",
        }
    }

    /// As the inspector says it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Dissolve => "Dissolve",
            Kind::DipBlack => "Dip to black",
            Kind::DipWhite => "Dip to white",
            Kind::WipeLeft => "Wipe left",
            Kind::WipeRight => "Wipe right",
            Kind::WipeUp => "Wipe up",
            Kind::WipeDown => "Wipe down",
            Kind::PushLeft => "Push left",
            Kind::PushRight => "Push right",
            Kind::PushUp => "Push up",
            Kind::PushDown => "Push down",
            Kind::Zoom => "Zoom",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        let name = name.trim().to_lowercase().replace([' ', '_'], "-");
        let name = match name.as_str() {
            "crossfade" | "cross-dissolve" | "cross-fade" | "fade" | "mix" => "dissolve",
            "dip" | "dip-to-black" | "black" | "fade-black" | "fade-to-black" => "dip-black",
            "dip-to-white" | "white" | "flash" | "fade-white" | "fade-to-white" => "dip-white",
            "wipe" => "wipe-left",
            "push" | "slide" => "push-left",
            "zoom-through" => "zoom",
            other => other,
        }
        .to_string();
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The colour a dip passes through, 0xRRGGBB.
    pub fn dip(self) -> Option<u32> {
        match self {
            Kind::DipBlack => Some(0x000000),
            Kind::DipWhite => Some(0xffffff),
            _ => None,
        }
    }

    /// Whether the clip going out moves (and so plays on past its end).
    fn moves_outgoing(self) -> bool {
        matches!(
            self,
            Kind::Dissolve | Kind::PushLeft | Kind::PushRight | Kind::PushUp | Kind::PushDown | Kind::Zoom
        ) || self.is_wipe()
    }

    fn is_wipe(self) -> bool {
        matches!(self, Kind::WipeLeft | Kind::WipeRight | Kind::WipeUp | Kind::WipeDown)
    }
}

/// The way into a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: Kind,
    pub duration: Time,
}

impl Transition {
    pub const DEFAULT_LENGTH: Time = SECOND * 3 / 5;
    pub const LONGEST: Time = 5 * SECOND;

    pub fn new(kind: Kind) -> Transition {
        Transition {
            kind,
            duration: Transition::DEFAULT_LENGTH,
        }
    }
}

/// A transition as it plays at one cut: the clips on either side, and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub kind: Kind,
    /// The clip going out; None when the clip comes in over what is beneath.
    pub from: Option<ClipId>,
    pub to: ClipId,
    /// The stretch of timeline it takes.
    pub start: Time,
    pub end: Time,
}

impl Cut {
    pub fn duration(&self) -> Time {
        self.end - self.start
    }
}

impl Project {
    /// The visual clip just before `id` on its track, when it ends exactly
    /// where `id` starts: what a transition into `id` comes from.
    pub fn clip_before(&self, id: ClipId) -> Option<&Clip> {
        let slot = self.slot(id)?;
        let track = &self.tracks[slot.track];
        let clip = &track.clips[slot.index];
        let before = track.clips.get(slot.index.checked_sub(1)?)?;
        let visual = self.source(before).is_some_and(|s| s.is_visual());
        (before.end() == clip.start && visual).then_some(before)
    }

    /// The visual clip just after `id` on its track, starting where it ends.
    pub fn clip_after(&self, id: ClipId) -> Option<&Clip> {
        let slot = self.slot(id)?;
        let track = &self.tracks[slot.track];
        let clip = &track.clips[slot.index];
        let after = track.clips.get(slot.index + 1)?;
        let visual = self.source(after).is_some_and(|s| s.is_visual());
        (after.start == clip.end() && visual).then_some(after)
    }

    /// The longest a transition into `id` can be: no longer than either clip.
    pub fn longest_transition(&self, id: ClipId) -> Time {
        let Some(clip) = self.clip(id) else { return 0 };
        let mut most = clip.duration.min(Transition::LONGEST);
        if let Some(before) = self.clip_before(id) {
            most = most.min(before.duration);
        }
        most
    }

    /// The transition into `id`, as it plays.
    pub fn cut_into(&self, id: ClipId) -> Option<Cut> {
        let clip = self.clip(id)?;
        let t = clip.transition?;
        if !self.source(clip).is_some_and(|s| s.is_visual()) {
            return None;
        }
        let d = t
            .duration
            .clamp(2 * self.frame(), self.longest_transition(id).max(2 * self.frame()));
        Some(match self.clip_before(id) {
            Some(before) => Cut {
                kind: t.kind,
                from: Some(before.id),
                to: id,
                start: clip.start - d / 2,
                end: clip.start - d / 2 + d,
            },
            None => Cut {
                kind: t.kind,
                from: None,
                to: id,
                start: clip.start,
                end: clip.start + d,
            },
        })
    }

    /// The transition out of `id`: the one into the clip after it.
    pub fn cut_out_of(&self, id: ClipId) -> Option<Cut> {
        let after = self.clip_after(id)?;
        self.cut_into(after.id).filter(|c| c.from == Some(id))
    }

    /// Every transition, in timeline order.
    pub fn cuts(&self) -> Vec<Cut> {
        let mut cuts: Vec<Cut> = self.clips().filter_map(|(_, c)| self.cut_into(c.id)).collect();
        cuts.sort_by_key(|c| c.start);
        cuts
    }

    /// How long before its start and after its end a clip is drawn, for
    /// transitions that need it to play on.
    pub fn handles(&self, id: ClipId) -> (Time, Time) {
        let head = self
            .cut_into(id)
            .filter(|c| c.from.is_some() && c.kind.moves_outgoing())
            .and_then(|c| self.clip(id).map(|clip| clip.start - c.start))
            .unwrap_or(0);
        let tail = self
            .cut_out_of(id)
            .filter(|c| c.kind.moves_outgoing())
            .and_then(|c| self.clip(id).map(|clip| c.end - clip.end()))
            .unwrap_or(0);
        (head.max(0), tail.max(0))
    }

    /// Sets (or with None, takes away) the transition into each clip.
    pub fn set_transition(&mut self, ids: &[ClipId], transition: Option<Transition>) {
        for &id in ids {
            let most = self.longest_transition(id);
            if let Some(clip) = self.clip_mut(id) {
                clip.transition = transition.map(|t| Transition {
                    kind: t.kind,
                    duration: t.duration.clamp(SECOND / 10, most.max(SECOND / 10)),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Media, MediaId, MediaKind};
    use std::path::PathBuf;

    fn two_clips() -> (Project, ClipId, ClipId) {
        let mut p = Project::default();
        let m = p.add_media(Media {
            id: MediaId(0),
            name: "v".into(),
            hash: "a".into(),
            ext: "mp4".into(),
            kind: MediaKind::Video,
            duration: 4 * SECOND,
            width: 1920,
            height: 1080,
            has_audio: false,
            source: PathBuf::new(),
            recipe: None,
            content: None,
            clicks: Vec::new(),
        });
        let a = p.append_clip(m).unwrap();
        let b = p.append_clip(m).unwrap();
        (p, a, b)
    }

    #[test]
    fn a_transition_is_centred_on_the_cut() {
        let (mut p, a, b) = two_clips();
        p.set_transition(&[b], Some(Transition::new(Kind::Dissolve)));
        let cut = p.cut_into(b).unwrap();
        assert_eq!(cut.from, Some(a));
        assert_eq!((cut.start, cut.end), (4 * SECOND - 300_000, 4 * SECOND + 300_000));
        assert_eq!(p.handles(a), (0, 300_000), "the clip going out plays on");
        assert_eq!(p.handles(b), (300_000, 0), "the one coming in starts early");
        assert_eq!(p.cut_out_of(a), Some(cut));
    }

    #[test]
    fn dips_need_no_extra_frames_and_lengths_are_kept_short_enough() {
        let (mut p, a, b) = two_clips();
        p.set_transition(
            &[b],
            Some(Transition {
                kind: Kind::DipBlack,
                duration: 60 * SECOND,
            }),
        );
        assert_eq!(p.clip(b).unwrap().transition.unwrap().duration, 4 * SECOND);
        assert_eq!(p.handles(a), (0, 0));
        // With nothing before, it plays over the clip's first moments.
        p.set_transition(&[a], Some(Transition::new(Kind::PushLeft)));
        let first = p.cut_into(a).unwrap();
        assert_eq!((first.from, first.start, first.end), (None, 0, 600_000));
        assert_eq!(p.handles(a).0, 0);
    }

    #[test]
    fn kinds_read_loosely() {
        assert_eq!(Kind::parse("crossfade"), Some(Kind::Dissolve));
        assert_eq!(Kind::parse("Dip to white"), Some(Kind::DipWhite));
        assert_eq!(Kind::parse("push_up"), Some(Kind::PushUp));
        assert_eq!(Kind::parse("spin"), None);
    }
}
