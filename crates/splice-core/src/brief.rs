//! Briefs: everything known about one beat, gathered for whoever makes its
//! footage (a crew, or a video model): what happens, what to capture, who and
//! what is in it with their continuity words and pictures, the movie's look,
//! its shape and length, and what comes just before and after. The outline
//! follows how video models are best prompted: references first, each with
//! one job; the look once; the subject anchored in words; one action and one
//! camera move per shot; what must not change.

use crate::bible::{Entry, Kind};
use crate::model::{Project, Settings};
use crate::story::{BeatId, Still};
use crate::time::secs;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Brief {
    pub beat: u64,
    pub number: usize,
    pub title: String,
    pub story: String,
    pub capture: String,
    /// Where it starts on the timeline, and how long it runs (seconds).
    pub start: f64,
    pub duration: f64,
    /// How long to ask a video model for: the beat's length, rounded up into
    /// what models make (4 to 30 seconds). Trim the result to `duration`.
    pub generate_seconds: u64,
    /// The canvas: what the footage should be.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub aspect: String,
    pub cast: Vec<Anchor>,
    pub look: Vec<Anchor>,
    /// The beat's reference shots, in order.
    pub shots: Vec<ShotRef>,
    /// The references to send, at most five, each with one job, numbered
    /// as the outline names them (@Image1…): the look, faces, places,
    /// things, then shots.
    pub images: Vec<Picture>,
    /// Pictures that did not make the five: other views of the cast, more shots.
    pub more_pictures: Vec<Picture>,
    /// Cues inside the beat: what happens in the sound.
    pub cues: Vec<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    /// A prompt to start from, written to how video models read best.
    pub outline: String,
}

/// A bible entry as a brief carries it.
#[derive(Clone, Debug, Serialize)]
pub struct Anchor {
    pub entry: u64,
    pub kind: Kind,
    pub name: String,
    pub words: String,
    #[serde(skip)]
    pub pictures: Vec<Still>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ShotRef {
    pub shot: u64,
    pub note: String,
    #[serde(skip)]
    pub picture: Option<Still>,
}

/// A picture a brief sends, and the one job it has.
#[derive(Clone, Debug, Serialize)]
pub struct Picture {
    /// "@Image1", in `images`; empty in `more_pictures`.
    #[serde(rename = "ref")]
    pub tag: String,
    /// Who or what it shows.
    pub of: String,
    /// What the model should take from it.
    pub job: String,
    #[serde(skip)]
    pub still: Still,
}

/// References that work best: models follow four or five far better than
/// the nine or ten they accept.
pub const MOST_REFERENCES: usize = 5;

/// The aspect ratio a model names: 16:9, 9:16, 1:1, 4:3, 3:4, 21:9, else W:H.
pub fn aspect(s: Settings) -> String {
    let r = s.width as f64 / s.height.max(1) as f64;
    let named = [
        ("21:9", 21.0 / 9.0),
        ("16:9", 16.0 / 9.0),
        ("4:3", 4.0 / 3.0),
        ("1:1", 1.0),
        ("3:4", 3.0 / 4.0),
        ("9:16", 9.0 / 16.0),
    ];
    named
        .iter()
        .find(|(_, v)| (r - v).abs() < 0.02)
        .map_or_else(|| format!("{}:{}", s.width, s.height), |(n, _)| n.to_string())
}

fn anchor(e: &Entry) -> Anchor {
    Anchor {
        entry: e.id.0,
        kind: e.kind,
        name: e.name.clone(),
        words: e.words.trim().to_string(),
        pictures: e.pictures.clone(),
    }
}

fn job(kind: Kind) -> &'static str {
    match kind {
        Kind::Look => "colour, light and grain only",
        Kind::Character => "face, hair and wardrobe only",
        Kind::Place => "the location and its light only",
        Kind::Thing => "its shape, colour and marks only",
    }
}

impl Project {
    /// The brief for one beat.
    pub fn brief(&self, beat: BeatId) -> Option<Brief> {
        let index = self.story.index(beat)?;
        let b = &self.story.beats[index];
        let (start, end) = self.story.span(beat)?;
        let cast: Vec<Anchor> = b
            .cast
            .iter()
            .filter_map(|id| self.bible.entry(*id))
            .map(anchor)
            .collect();
        let look: Vec<Anchor> = self.bible.of(Kind::Look).map(anchor).collect();
        let shots: Vec<ShotRef> = b
            .shots
            .iter()
            .map(|s| ShotRef {
                shot: s.id.0,
                note: s.note.clone(),
                picture: s.image.clone(),
            })
            .collect();
        // Every candidate picture, best first: the look, faces, places,
        // things, shots; then the further pictures of each.
        let mut ranked: Vec<Picture> = Vec::new();
        let mut later: Vec<Picture> = Vec::new();
        let by_kind = |k: Kind| cast.iter().chain(look.iter()).filter(move |a| a.kind == k);
        for kind in [Kind::Look, Kind::Character, Kind::Place, Kind::Thing] {
            for a in by_kind(kind) {
                for (i, still) in a.pictures.iter().enumerate() {
                    let picture = Picture {
                        tag: String::new(),
                        of: if kind == Kind::Look {
                            "the look".into()
                        } else {
                            a.name.clone()
                        },
                        job: job(kind).into(),
                        still: still.clone(),
                    };
                    if i == 0 {
                        ranked.push(picture)
                    } else {
                        later.push(picture)
                    }
                }
            }
        }
        for s in &shots {
            if let Some(still) = &s.picture {
                ranked.push(Picture {
                    tag: String::new(),
                    of: if s.note.trim().is_empty() {
                        "a shot".into()
                    } else {
                        s.note.trim().to_string()
                    },
                    job: "the framing only".into(),
                    still: still.clone(),
                });
            }
        }
        let mut images: Vec<Picture> = Vec::new();
        for mut p in ranked {
            if images.len() < MOST_REFERENCES {
                p.tag = format!("@Image{}", images.len() + 1);
                images.push(p);
            } else {
                later.push(p);
            }
        }
        let cues = self
            .story
            .cues
            .iter()
            .filter(|c| c.at >= start && c.at < end)
            .map(|c| format!("{} at {:.0}s", c.label, secs(c.at - start)))
            .collect();
        let title = |i: usize| self.story.beats.get(i).map(|x| x.title.clone());
        let length = end - start;
        let mut brief = Brief {
            beat: beat.0,
            number: index + 1,
            title: b.title.clone(),
            story: b.story.trim().to_string(),
            capture: b.capture.trim().to_string(),
            start: secs(start),
            duration: secs(length),
            generate_seconds: (secs(length).ceil() as u64).clamp(4, 30),
            width: self.settings.width,
            height: self.settings.height,
            fps: self.settings.fps,
            aspect: aspect(self.settings),
            cast,
            look,
            shots,
            images,
            more_pictures: later,
            cues,
            before: index.checked_sub(1).and_then(title),
            after: title(index + 1),
            outline: String::new(),
        };
        brief.outline = outline(&brief);
        Some(brief)
    }
}

fn sentence(text: &str) -> String {
    text.trim().trim_end_matches('.').to_string()
}

/// A prompt skeleton for the beat, in the order video models read best.
fn outline(b: &Brief) -> String {
    let mut out = Vec::new();
    // References, each with one job.
    if !b.images.is_empty() {
        let refs: Vec<String> = b
            .images
            .iter()
            .map(|p| format!("{} is {}: use it for {}.", p.tag, p.of, p.job))
            .collect();
        out.push(refs.join(" "));
    }
    // The look, once, before any shot.
    let look: Vec<String> = b
        .look
        .iter()
        .map(|l| sentence(&l.words))
        .filter(|w| !w.is_empty())
        .collect();
    if !look.is_empty() {
        out.push(format!("Style: {}.", look.join("; ")));
    }
    // Who and where, anchored in words even when a picture shows them.
    for a in &b.cast {
        if !a.words.is_empty() {
            out.push(format!("{}: {}.", a.name, sentence(&a.words)));
        }
    }
    // The shots: one action each, timed across the length asked for. With
    // several reference shots, one timed shot each.
    let total = b.generate_seconds as f64;
    let noted: Vec<&ShotRef> = b.shots.iter().filter(|s| !s.note.trim().is_empty()).collect();
    let action = sentence(if b.story.is_empty() { &b.title } else { &b.story });
    if noted.len() >= 2 {
        let each = total / noted.len() as f64;
        out.push(format!("{action}."));
        for (i, s) in noted.iter().enumerate() {
            let (a, z) = ((i as f64 * each).round(), ((i + 1) as f64 * each).round());
            out.push(format!("{a:.0}–{z:.0}s: {}.", sentence(&s.note)));
        }
    } else {
        out.push(format!("0–{total:.0}s: {action}."));
    }
    if !b.capture.is_empty() {
        out.push(format!("Capture: {}.", sentence(&b.capture)));
    }
    if !b.cues.is_empty() {
        out.push(format!("Sound: {}.", b.cues.join("; ")));
    }
    // What must not change, and what must not appear.
    let keep: Vec<&str> = b.cast.iter().map(|a| a.name.as_str()).collect();
    if !keep.is_empty() {
        out.push(format!("Keep {} exactly as described throughout.", keep.join(", ")));
    }
    out.push("No on-screen text, no watermark, no extra characters.".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::SECOND;

    fn still(hash: &str) -> Still {
        Still {
            hash: hash.into(),
            ext: "png".into(),
            width: 10,
            height: 10,
        }
    }

    #[test]
    fn references_are_ranked_capped_and_numbered_once() {
        let mut p = Project::default();
        let look = p.add_entry(Kind::Look, "Look");
        p.set_entry(look, |e| e.pictures.push(still("look")));
        let beat = p.add_beat(None, "Busy", 2 * SECOND);
        for n in 0..4 {
            let c = p.add_entry(Kind::Character, &format!("C{n}"));
            p.set_entry(c, |e| {
                e.pictures.push(still(&format!("face{n}")));
                e.pictures.push(still(&format!("side{n}")));
            });
            p.toggle_cast(beat, c);
        }
        p.add_shot(beat, "wide", Some(still("shot")));
        let brief = p.brief(beat).unwrap();
        assert_eq!(brief.images.len(), MOST_REFERENCES);
        assert_eq!(brief.images[0].of, "the look", "the look leads");
        assert_eq!(brief.images[1].tag, "@Image2");
        assert!(
            brief.more_pictures.iter().any(|p| p.still.hash == "shot"),
            "overflow is kept"
        );
        assert_eq!(brief.generate_seconds, 4, "models make at least four seconds");
        assert!(brief.outline.starts_with("@Image1 is the look"));
    }

    #[test]
    fn a_brief_carries_the_cast_the_look_and_an_outline() {
        let mut p = Project::default();
        let ada = p.add_entry(Kind::Character, "Ada");
        p.set_entry(ada, |e| e.words = "short silver hair, red coat".into());
        let look = p.add_entry(Kind::Look, "Dusk");
        p.set_entry(look, |e| e.words = "warm low sun, 35mm, soft grain".into());
        let open = p.add_beat(None, "Open", 4 * SECOND);
        let walk = p.add_beat(Some(open), "Walk", 6 * SECOND);
        p.set_beat(walk, |b| {
            b.story = "Ada crosses the bridge".into();
            b.capture = "slow dolly left".into();
        });
        p.toggle_cast(walk, ada);
        let brief = p.brief(walk).unwrap();
        assert_eq!((brief.number, brief.duration, brief.aspect.as_str()), (2, 6.0, "16:9"));
        assert_eq!(brief.before.as_deref(), Some("Open"));
        assert_eq!(brief.cast[0].name, "Ada");
        assert!(brief.outline.contains("Style: warm low sun"), "{}", brief.outline);
        assert!(brief.outline.contains("Ada: short silver hair"));
        assert!(
            brief.outline.contains("0–6s: Ada crosses the bridge."),
            "{}",
            brief.outline
        );
        assert!(brief.outline.contains("Capture: slow dolly left."));
        assert!(brief.outline.contains("Keep Ada exactly"));
        assert_eq!(brief.generate_seconds, 6);
    }
}
