//! What the picture is made of, moment to moment: each visual clip's stretch
//! of timeline (with the frames a transition borrows), its pose over time
//! (its look, its keyframes and its transitions), the part of it a wipe
//! shows, and the colours dips pass through. Export prints these as ffmpeg
//! expressions; the viewer evaluates the same expressions.

use crate::look::build::{C, T, add, clip01, div, mul, smooth, sub};
use crate::look::{Expr, Pose, PoseExpr};
use crate::model::{Clip, Project, Recipe, Source};
use crate::time::{Time, secs};
use crate::transition::{Cut, Kind};

/// How far a push moves a picture: the canvas's width or height.
const PUSH: f64 = 1.0;
/// How much bigger a zoom makes the picture going out.
const ZOOM: f64 = 0.25;
/// The soft edge of a wipe, as a fraction of the picture.
pub const WIPE_SOFT: f64 = 0.004;

/// A side of a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// The part of a picture a wipe shows: a band growing from `from` that
/// covers `progress` (0 to 1) of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reveal {
    pub from: Edge,
    /// Over clip time in seconds.
    pub progress: Expr,
    /// How far ahead of `progress` the band reaches, as a fraction: the
    /// picture going out reaches one soft edge further than the one coming
    /// in leaves it, so the soft edge never shows what is beneath.
    pub lead: f64,
}

impl Reveal {
    /// What shows at clip time `t`: left, top, right, bottom, as fractions.
    pub fn part(&self, t: f64) -> [f32; 4] {
        let e = self.progress.eval(t).clamp(0.0, 1.0) as f32;
        match self.from {
            Edge::Left => [0.0, 0.0, e, 1.0],
            Edge::Right => [1.0 - e, 0.0, 1.0, 1.0],
            Edge::Top => [0.0, 0.0, 1.0, e],
            Edge::Bottom => [0.0, 1.0 - e, 1.0, 1.0],
        }
    }

    /// How much of the pixel at (`X`, `Y`) shows, for ffmpeg's `geq`, with
    /// clip time written as `t`.
    pub fn ffmpeg(&self, t: &str) -> String {
        let e = format!("({}*{:.6}+{:.6})", self.progress.ffmpeg(t), 1.0 + WIPE_SOFT, self.lead);
        let (along, grows_back) = match self.from {
            Edge::Left => ("((X+0.5)/W)", false),
            Edge::Right => ("((X+0.5)/W)", true),
            Edge::Top => ("((Y+0.5)/H)", false),
            Edge::Bottom => ("((Y+0.5)/H)", true),
        };
        if grows_back {
            format!("clip(({along}-1+{e})/{WIPE_SOFT:.6},0,1)")
        } else {
            format!("clip(({e}-{along})/{WIPE_SOFT:.6},0,1)")
        }
    }
}

/// A visual clip as it is drawn.
#[derive(Clone, Debug)]
pub struct Drawn<'a> {
    pub clip: &'a Clip,
    pub source: Source<'a>,
    pub track: usize,
    /// The stretch of timeline it is drawn over: its own, and any frames a
    /// transition has it play before its start or after its end.
    pub from: Time,
    pub to: Time,
    /// Its pose over clip time in seconds (negative before its start).
    pub pose: PoseExpr,
    /// The part of it a wipe shows, when one is under way.
    pub reveal: Option<Reveal>,
    /// The part of it a wipe out of it still shows.
    pub conceal: Option<Reveal>,
    /// Coming in from a colour (a dip with nothing before it): the colour,
    /// over the stretch of timeline it clears.
    pub tint: Option<(u32, Time, Time)>,
}

impl Drawn<'_> {
    /// How long before its start it is drawn.
    pub fn head(&self) -> Time {
        self.clip.start - self.from
    }

    /// How long after its end it is drawn.
    pub fn tail(&self) -> Time {
        self.to - self.clip.end()
    }

    /// Its pose at timeline time `t`.
    pub fn pose_at(&self, t: Time) -> Pose {
        self.pose.at(secs(t - self.clip.start))
    }

    /// The part of its frame that shows at timeline time `t`.
    pub fn shows(&self, t: Time) -> [f32; 4] {
        let local = secs(t - self.clip.start);
        let mut part: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
        for reveal in [&self.reveal, &self.conceal].into_iter().flatten() {
            let p = reveal.part(local);
            part = [
                part[0].max(p[0]),
                part[1].max(p[1]),
                part[2].min(p[2]),
                part[3].min(p[3]),
            ];
        }
        part
    }
}

/// A colour a dip passes through, drawn over the clips of its cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Dip {
    pub track: usize,
    /// 0xRRGGBB.
    pub color: u32,
    pub from: Time,
    pub to: Time,
}

impl Dip {
    /// How much of it shows at timeline time `t`.
    pub fn opacity(&self, t: Time) -> f64 {
        let p = ((t - self.from) as f64 / (self.to - self.from).max(1) as f64).clamp(0.0, 1.0);
        1.0 - (2.0 * p - 1.0).abs()
    }
}

/// How a clip sounds across transitions: when its sound plays (past its
/// edges where a transition crossfades it with a neighbour's), and its fades.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sound {
    /// Timeline stretch it plays over.
    pub from: Time,
    pub to: Time,
    /// Where in its source the sound at `from` is.
    pub source: Time,
    /// Rising, and falling, over these stretches of timeline.
    pub fade_in: Option<(Time, Time)>,
    pub fade_out: Option<(Time, Time)>,
}

impl Sound {
    /// How loud it plays at timeline time `t`, 0 to 1: quarter-sine fades,
    /// so a crossfade keeps the loudness even.
    pub fn gain(&self, t: Time) -> f32 {
        let ramp = |(a, z): (Time, Time), rising: bool| {
            let p = ((t - a) as f64 / (z - a).max(1) as f64).clamp(0.0, 1.0);
            let p = if rising { p } else { 1.0 - p };
            (p * std::f64::consts::FRAC_PI_2).sin() as f32
        };
        self.fade_in.map_or(1.0, |f| ramp(f, true)) * self.fade_out.map_or(1.0, |f| ramp(f, false))
    }
}

/// What the viewer draws at a moment, bottom first.
#[derive(Clone, Debug)]
pub enum Shown<'a> {
    Clip {
        drawn: Box<Drawn<'a>>,
        pose: Pose,
        /// The part of its frame that shows.
        part: [f32; 4],
    },
    Fill {
        color: u32,
        opacity: f64,
        /// Over just this clip's picture (a clip coming in from a colour),
        /// or the whole canvas.
        over: Option<crate::model::ClipId>,
    },
}

/// Whether a clip hides everything beneath it wherever it is drawn: a
/// picture of its own, with no key, mask, crop or see-through parts.
fn solid(project: &Project, clip: &Clip) -> bool {
    let look = &clip.look;
    let plain = look.key.is_none()
        && look.mask.is_none()
        && look.crop.is_none_or(|c| c.is_none())
        && look.opacity >= 1.0
        && look.keys.opacity.is_empty();
    plain
        && match project.source(clip) {
            Some(Source::Media(m)) => match &m.recipe {
                Some(Recipe::Scene(scene)) => !scene.transparent,
                Some(Recipe::Symbol(_)) => false,
                _ => true,
            },
            Some(Source::Generated(crate::model::Generator::Color { color })) => color & 0xff == 0xff,
            _ => false,
        }
}

/// Whether a clip covers the whole canvas, all the way through its first
/// moments: solid, unmoved, at least full size, its media the canvas's shape.
fn covers(project: &Project, clip: &Clip) -> bool {
    let look = &clip.look;
    let still = look.keys.x.is_empty()
        && look.keys.y.is_empty()
        && look.keys.scale.is_empty()
        && look.x == 0.0
        && look.y == 0.0
        && look.scale >= 0.999
        && look.enter.kind == crate::look::MotionKind::None;
    let shape = match project.source(clip) {
        Some(Source::Media(m)) if m.width > 0 && m.height > 0 => {
            let (w, h) = (project.settings.width as f64, project.settings.height as f64);
            ((m.width as f64 / m.height as f64) - w / h.max(1.0)).abs() < 0.01
        }
        Some(Source::Generated(_)) => true,
        _ => false,
    };
    still && shape && solid(project, clip)
}

/// Progress through a cut over clip time, 0 to 1, for a clip starting at `start`.
fn progress(cut: &Cut, start: Time) -> Expr {
    let at = secs(cut.start - start);
    let d = secs(cut.duration()).max(1e-3);
    clip01(div(sub(T, C(at)), C(d)))
}

impl Project {
    /// How clip `clip` (on track `track`) is drawn, if it shows at all.
    pub fn drawn<'a>(&'a self, clip: &'a Clip, track: usize) -> Option<Drawn<'a>> {
        let source = self.source(clip).filter(|s| s.is_visual())?;
        let (head, tail) = self.handles(clip.id);
        let mut pose = clip.look.pose_expr(clip.duration);
        let mut reveal = None;
        let mut conceal = None;
        let mut tint = None;
        // The way in.
        if let Some(cut) = self.cut_into(clip.id) {
            let p = progress(&cut, clip.start);
            let e = smooth(p.clone());
            let rest = sub(C(1.0), e.clone());
            match cut.kind {
                Kind::Dissolve => pose.opacity = mul(pose.opacity, p),
                Kind::DipBlack | Kind::DipWhite if cut.from.is_some() => {}
                // With nothing before: a solid picture comes up out of the
                // colour; anything else (a title, a picture in picture) fades in,
                // so the colour never covers the rest of the frame.
                Kind::DipBlack | Kind::DipWhite if solid(self, clip) => {
                    tint = Some((cut.kind.dip().unwrap_or(0), cut.start, cut.end));
                }
                Kind::DipBlack | Kind::DipWhite => pose.opacity = mul(pose.opacity, p),
                Kind::WipeLeft => {
                    reveal = Some(Reveal {
                        from: Edge::Right,
                        progress: e,
                        lead: 0.0,
                    })
                }
                Kind::WipeRight => {
                    reveal = Some(Reveal {
                        from: Edge::Left,
                        progress: e,
                        lead: 0.0,
                    })
                }
                Kind::WipeUp => {
                    reveal = Some(Reveal {
                        from: Edge::Bottom,
                        progress: e,
                        lead: 0.0,
                    })
                }
                Kind::WipeDown => {
                    reveal = Some(Reveal {
                        from: Edge::Top,
                        progress: e,
                        lead: 0.0,
                    })
                }
                Kind::PushLeft => pose.x = add(pose.x, mul(C(PUSH), rest)),
                Kind::PushRight => pose.x = sub(pose.x, mul(C(PUSH), rest)),
                Kind::PushUp => pose.y = add(pose.y, mul(C(PUSH), rest)),
                Kind::PushDown => pose.y = sub(pose.y, mul(C(PUSH), rest)),
                Kind::Zoom => {
                    pose.opacity = mul(pose.opacity, p);
                    pose.scale = mul(pose.scale, add(C(1.0), mul(C(ZOOM), rest)));
                }
            }
        }
        // The way out, into the clip after.
        if let Some(cut) = self.cut_out_of(clip.id) {
            let p = progress(&cut, clip.start);
            let e = smooth(p.clone());
            let gone = sub(C(1.0), e.clone());
            // It stays solid under the clip coming in only when that one
            // covers it entirely; otherwise it would vanish at the end.
            let fades = !(solid(self, clip) && self.clip(cut.to).is_some_and(|next| covers(self, next)));
            match cut.kind {
                Kind::Dissolve if fades => pose.opacity = mul(pose.opacity, sub(C(1.0), p)),
                Kind::Dissolve | Kind::DipBlack | Kind::DipWhite => {}
                // What a wipe has not yet reached.
                Kind::WipeLeft => {
                    conceal = Some(Reveal {
                        from: Edge::Left,
                        progress: gone,
                        lead: WIPE_SOFT,
                    })
                }
                Kind::WipeRight => {
                    conceal = Some(Reveal {
                        from: Edge::Right,
                        progress: gone,
                        lead: WIPE_SOFT,
                    })
                }
                Kind::WipeUp => {
                    conceal = Some(Reveal {
                        from: Edge::Top,
                        progress: gone,
                        lead: WIPE_SOFT,
                    })
                }
                Kind::WipeDown => {
                    conceal = Some(Reveal {
                        from: Edge::Bottom,
                        progress: gone,
                        lead: WIPE_SOFT,
                    })
                }
                Kind::PushLeft => pose.x = sub(pose.x, mul(C(PUSH), e)),
                Kind::PushRight => pose.x = add(pose.x, mul(C(PUSH), e)),
                Kind::PushUp => pose.y = sub(pose.y, mul(C(PUSH), e)),
                Kind::PushDown => pose.y = add(pose.y, mul(C(PUSH), e)),
                Kind::Zoom => {
                    pose.scale = mul(pose.scale, add(C(1.0), mul(C(ZOOM), e)));
                    if fades {
                        pose.opacity = mul(pose.opacity, sub(C(1.0), p));
                    }
                }
            }
        }
        Some(Drawn {
            clip,
            source,
            track,
            from: clip.start - head,
            to: clip.end() + tail,
            pose,
            reveal,
            conceal,
            tint,
        })
    }

    /// The dip into clip `id` from the clip before it, if its transition is one.
    pub fn dip_into(&self, clip: &Clip, track: usize) -> Option<Dip> {
        let cut = self.cut_into(clip.id).filter(|c| c.from.is_some())?;
        Some(Dip {
            track,
            color: cut.kind.dip()?,
            from: cut.start,
            to: cut.end,
        })
    }

    /// How clip `clip` sounds: a transition between two clips crossfades
    /// their sound across it (each playing on past its edge where the media
    /// has more); a dip fades out and back in at the cut; a clip coming in
    /// over nothing fades in.
    pub fn sound(&self, clip: &Clip) -> Sound {
        let mut sound = Sound {
            from: clip.start,
            to: clip.end(),
            source: clip.source_in,
            fade_in: None,
            fade_out: None,
        };
        // Only a clip playing as recorded can borrow sound past its edges.
        let length = self.media(clip.media).map_or(0, |m| m.duration);
        let plain = !clip.is_retimed() && clip.generator.is_none();
        if let Some(cut) = self.cut_into(clip.id) {
            if cut.from.is_some() && cut.kind.dip().is_none() {
                if plain {
                    sound.from = (clip.start - (clip.start - cut.start).min(clip.source_in)).max(cut.start);
                    sound.source = clip.source_in - (clip.start - sound.from);
                }
                sound.fade_in = Some((cut.start, cut.end));
            } else if cut.from.is_some() {
                sound.fade_in = Some((clip.start, cut.end));
            } else {
                sound.fade_in = Some((cut.start, cut.end));
            }
        }
        if let Some(cut) = self.cut_out_of(clip.id) {
            if cut.kind.dip().is_none() {
                if plain {
                    let more = (length - clip.source_out()).max(0);
                    sound.to = clip.end() + (cut.end - clip.end()).min(more);
                }
                sound.fade_out = Some((cut.start, cut.end));
            } else {
                sound.fade_out = Some((cut.start, clip.end()));
            }
        }
        sound
    }

    /// What the viewer draws at `t`, bottom first.
    pub fn picture(&self, t: Time) -> Vec<Shown<'_>> {
        let mut out = Vec::new();
        for (track, lane) in self.tracks.iter().enumerate().filter(|(_, l)| !l.hidden) {
            for clip in &lane.clips {
                let Some(drawn) = self.drawn(clip, track) else { continue };
                if t >= drawn.from && t < drawn.to {
                    let pose = drawn.pose_at(t);
                    let part = drawn.shows(t);
                    let tint = drawn.tint.filter(|(_, a, z)| t >= *a && t < *z);
                    out.push(Shown::Clip {
                        drawn: Box::new(drawn),
                        pose,
                        part,
                    });
                    if let Some((color, a, z)) = tint {
                        out.push(Shown::Fill {
                            color,
                            opacity: 1.0 - ((t - a) as f64 / (z - a).max(1) as f64).clamp(0.0, 1.0),
                            over: Some(clip.id),
                        });
                    }
                }
                if let Some(dip) = self.dip_into(clip, track)
                    && t >= dip.from
                    && t < dip.to
                {
                    out.push(Shown::Fill {
                        color: dip.color,
                        opacity: dip.opacity(t),
                        over: None,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Media, MediaId, MediaKind};
    use crate::time::SECOND;
    use crate::transition::Transition;
    use std::path::PathBuf;

    fn film(p: &mut Project) -> MediaId {
        p.add_media(Media {
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
        })
    }

    #[test]
    fn a_dissolve_draws_both_clips_across_the_cut() {
        let mut p = Project::default();
        let m = film(&mut p);
        let a = p.append_clip(m).unwrap();
        let b = p.append_clip(m).unwrap();
        p.set_transition(&[b], Some(Transition::new(Kind::Dissolve)));
        let mid = p.picture(4 * SECOND);
        assert_eq!(mid.len(), 2, "the clip going out plays on at the cut");
        let opacities: Vec<f64> = mid
            .iter()
            .map(|s| match s {
                Shown::Clip { pose, .. } => pose.opacity,
                Shown::Fill { .. } => -1.0,
            })
            .collect();
        assert_eq!(opacities[0], 1.0, "a solid picture stays solid under the dissolve");
        assert!((opacities[1] - 0.5).abs() < 1e-6, "halfway in: {opacities:?}");
        let ids: Vec<_> = mid
            .iter()
            .filter_map(|s| match s {
                Shown::Clip { drawn, .. } => Some(drawn.clip.id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![a, b]);
        assert_eq!(p.picture(4 * SECOND + 400_000).len(), 1, "past the transition");
    }

    #[test]
    fn a_dip_fills_with_colour_at_the_cut() {
        let mut p = Project::default();
        let m = film(&mut p);
        p.append_clip(m);
        let b = p.append_clip(m).unwrap();
        p.set_transition(&[b], Some(Transition::new(Kind::DipBlack)));
        let at_cut = p.picture(4 * SECOND);
        assert!(
            matches!(at_cut.last(), Some(Shown::Fill { color: 0, opacity, over: None }) if (*opacity - 1.0).abs() < 1e-6),
            "black at the cut"
        );
        assert_eq!(at_cut.len(), 2, "only the clip coming in, under the black");
    }

    #[test]
    fn pushes_and_wipes_move_and_reveal() {
        let mut p = Project::default();
        let m = film(&mut p);
        let a = p.append_clip(m).unwrap();
        let b = p.append_clip(m).unwrap();
        p.set_transition(&[b], Some(Transition::new(Kind::PushLeft)));
        let shown = p.picture(4 * SECOND);
        let xs: Vec<f64> = shown
            .iter()
            .filter_map(|s| match s {
                Shown::Clip { pose, .. } => Some(pose.x),
                _ => None,
            })
            .collect();
        assert!((xs[0] + 0.5).abs() < 1e-6 && (xs[1] - 0.5).abs() < 1e-6, "{xs:?}");
        p.set_transition(&[b], Some(Transition::new(Kind::WipeRight)));
        let a_drawn = p.drawn(p.clip(a).unwrap(), 0).unwrap();
        let b_drawn = p.drawn(p.clip(b).unwrap(), 0).unwrap();
        assert_eq!(b_drawn.shows(4 * SECOND), [0.0, 0.0, 0.5, 1.0], "the left half is in");
        assert_eq!(
            a_drawn.shows(4 * SECOND),
            [0.5, 0.0, 1.0, 1.0],
            "the right half is still out"
        );
        assert!(b_drawn.reveal.unwrap().ffmpeg("t").starts_with("clip(("));
    }

    #[test]
    fn sound_crossfades_across_a_dissolve_and_dips_at_a_dip() {
        let mut p = Project::default();
        let m = film(&mut p);
        let a = p.append_clip(m).unwrap();
        p.trim_end(a, 3 * SECOND);
        let b = p.append_clip(m).unwrap();
        // The second second of the media onwards, right after the first clip.
        let clip = p.clip_mut(b).unwrap();
        clip.source_in = SECOND;
        clip.duration = 3 * SECOND;
        p.set_transition(&[b], Some(Transition::new(Kind::Dissolve)));
        let (out, into) = (p.sound(p.clip(a).unwrap()), p.sound(p.clip(b).unwrap()));
        assert_eq!(
            (out.to, into.from),
            (3_300_000, 2_700_000),
            "each plays on into the other"
        );
        assert_eq!(into.source, SECOND - 300_000);
        let mid = 3 * SECOND;
        let total = out.gain(mid).powi(2) + into.gain(mid).powi(2);
        assert!((total - 1.0).abs() < 1e-3, "even loudness: {total}");
        p.set_transition(&[b], Some(Transition::new(Kind::DipBlack)));
        let (out, into) = (p.sound(p.clip(a).unwrap()), p.sound(p.clip(b).unwrap()));
        assert_eq!((out.to, into.from), (3 * SECOND, 3 * SECOND), "a dip borrows nothing");
        assert_eq!((out.gain(mid), into.gain(mid)), (0.0, 0.0), "silent at the cut");
    }
}
