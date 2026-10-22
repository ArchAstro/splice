//! Titles that come in a piece at a time: word by word, letter by letter
//! or line by line, fading, rising, popping or typed; or, for captions,
//! words lighting up as they are said.
//!
//! The app draws a title's letters apart from its box and says where each
//! word, letter and line is in that picture (`Unit`). Here each of those
//! pieces gets its own pose over clip time, built on the title's, so the
//! viewer and export move them the same way.

use crate::look::build::{C, T, add, clip01, div, mul, smooth, sub};
use crate::look::{Expr, PoseExpr};
use crate::time::{SECOND, Time, secs};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Each piece fades in.
    #[default]
    Fade,
    /// Fades in rising a little.
    Rise,
    /// Grows in with a small overshoot.
    Pop,
    /// Appears at once, as if typed.
    Type,
    /// Words wait dimmed and light up as they are said.
    Karaoke,
}

impl Style {
    pub const ALL: [Style; 5] = [Style::Fade, Style::Rise, Style::Pop, Style::Type, Style::Karaoke];

    pub fn name(self) -> &'static str {
        match self {
            Style::Fade => "fade",
            Style::Rise => "rise",
            Style::Pop => "pop",
            Style::Type => "type",
            Style::Karaoke => "karaoke",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Style::Fade => "Fade",
            Style::Rise => "Rise",
            Style::Pop => "Pop",
            Style::Type => "Type",
            Style::Karaoke => "Karaoke",
        }
    }

    pub fn parse(name: &str) -> Option<Style> {
        let name = name.trim().to_lowercase();
        let name = match name.as_str() {
            "typewriter" | "typed" => "type",
            "highlight" | "sing" | "light" => "karaoke",
            "up" | "slide-up" => "rise",
            other => other,
        };
        Style::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// What comes in a piece at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum By {
    #[default]
    Word,
    Letter,
    Line,
}

impl By {
    pub const ALL: [By; 3] = [By::Word, By::Letter, By::Line];

    pub fn name(self) -> &'static str {
        match self {
            By::Word => "word",
            By::Letter => "letter",
            By::Line => "line",
        }
    }

    pub fn parse(name: &str) -> Option<By> {
        let name = name.trim().to_lowercase();
        By::ALL.into_iter().find(|b| b.name() == name.trim_end_matches('s'))
    }
}

/// How a title's pieces come in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct TextMotion {
    pub style: Style,
    pub by: By,
    /// How long until the last piece starts coming in (without word
    /// timings); each piece takes a moment more to arrive.
    pub over: Time,
}

impl Default for TextMotion {
    fn default() -> TextMotion {
        TextMotion {
            style: Style::Rise,
            by: By::Word,
            over: SECOND,
        }
    }
}

/// Where one piece of a title's letters is, in the picture of its letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Left, top, width, height, in pixels.
    pub rect: [u32; 4],
    /// The word it is (a letter: the word it is in; a line: its first word).
    pub word: usize,
}

/// The pieces of a title's letters, each way of cutting them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Units {
    pub words: Vec<Unit>,
    pub letters: Vec<Unit>,
    pub lines: Vec<Unit>,
}

impl Units {
    pub fn of(&self, by: By) -> &[Unit] {
        match by {
            By::Word => &self.words,
            By::Letter => &self.letters,
            By::Line => &self.lines,
        }
    }
}

/// One piece of a title as it is drawn: its part of the letters' picture,
/// and its pose (centre, size and opacity), over clip time.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub rect: [u32; 4],
    pub pose: PoseExpr,
}

/// How long a piece takes to arrive.
const ARRIVE: f64 = 0.4;

/// When each word starts, from the clip's start: as said (`timing`, one
/// per word) when there is a time for every word, else spread over `over`.
fn word_starts(motion: &TextMotion, words: usize, pieces: usize, timing: &[Time]) -> Option<Vec<f64>> {
    (timing.len() >= words && words > 0)
        .then(|| timing.iter().map(|t| secs(*t)).collect())
        .or_else(|| {
            let step = secs(motion.over) / pieces.max(2).saturating_sub(1).max(1) as f64;
            (pieces > 0).then(|| (0..pieces).map(|i| i as f64 * step).collect())
        })
}

/// The pieces of a title drawn in `size` pixels at scale 1 (on a canvas
/// `canvas` wide and high), placed by `pose` (the title's own), coming in
/// as `motion` says. `letter_size` is the title's letter size as a
/// fraction of the canvas height.
pub fn pieces(
    motion: &TextMotion,
    units: &Units,
    timing: &[Time],
    size: (f64, f64),
    canvas: (f64, f64),
    letter_size: f64,
    pose: &PoseExpr,
) -> Vec<Piece> {
    let list = units.of(motion.by);
    let words = units.words.len();
    // Pieces' starts: by word when timed (a letter or line takes its word's).
    let timed = timing.len() >= words && words > 0;
    let starts = match word_starts(motion, words, if timed { words } else { list.len() }, timing) {
        Some(s) => s,
        None => return Vec::new(),
    };
    list.iter()
        .enumerate()
        .map(|(i, unit)| {
            let start = if timed {
                starts.get(unit.word).copied().unwrap_or(0.0)
            } else {
                starts.get(i).copied().unwrap_or(0.0)
            };
            // When the next word starts: how long a word is said.
            let next = if timed {
                starts.get(unit.word + 1).copied().unwrap_or(start + 0.6)
            } else {
                start + ARRIVE
            };
            let p = |length: f64| clip01(div(sub(T, C(start)), C(length.max(1e-3))));
            let (opacity, lift, grow): (Expr, Expr, Expr) = match motion.style {
                Style::Fade => (p(ARRIVE), C(0.0), C(1.0)),
                Style::Rise => (
                    p(ARRIVE * 0.8),
                    mul(C(0.4 * letter_size), sub(C(1.0), smooth(p(ARRIVE)))),
                    C(1.0),
                ),
                Style::Pop => {
                    // Overshoots a little before settling (back-out).
                    let q = sub(p(ARRIVE), C(1.0));
                    let cube = mul(mul(q.clone(), q.clone()), q.clone());
                    let back = add(C(1.0), add(mul(C(2.70158), cube), mul(C(1.70158), mul(q.clone(), q))));
                    (p(0.12), C(0.0), add(C(0.4), mul(C(0.6), back)))
                }
                Style::Type => (clip01(mul(sub(T, C(start)), C(1000.0))), C(0.0), C(1.0)),
                Style::Karaoke => {
                    // Dim until said; lifted a touch while it is (growing
                    // would crowd its neighbours).
                    let said = clip01(mul(sub(T, C(start)), C(30.0)));
                    let saying = mul(said.clone(), clip01(mul(sub(C(next), T), C(12.0))));
                    (
                        add(C(0.42), mul(C(0.58), said)),
                        mul(C(-0.08 * letter_size), saying),
                        C(1.0),
                    )
                }
            };
            // Where its centre is from the picture's centre, at scale 1.
            let [x, y, w, h] = unit.rect;
            let (dx, dy) = (
                x as f64 + w as f64 / 2.0 - size.0 / 2.0,
                y as f64 + h as f64 / 2.0 - size.1 / 2.0,
            );
            Piece {
                rect: unit.rect,
                pose: PoseExpr {
                    x: add(pose.x.clone(), mul(C(dx / canvas.0.max(1.0)), pose.scale.clone())),
                    y: add(
                        add(pose.y.clone(), mul(C(dy / canvas.1.max(1.0)), pose.scale.clone())),
                        lift,
                    ),
                    scale: mul(pose.scale.clone(), grow),
                    opacity: mul(pose.opacity.clone(), opacity),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::Look;

    fn units() -> Units {
        let word = |x: u32, n: usize| Unit {
            rect: [x, 10, 80, 40],
            word: n,
        };
        Units {
            words: vec![word(0, 0), word(100, 1), word(200, 2)],
            letters: Vec::new(),
            lines: vec![Unit {
                rect: [0, 10, 280, 40],
                word: 0,
            }],
        }
    }

    #[test]
    fn words_come_in_one_after_another_where_they_are() {
        let motion = TextMotion {
            style: Style::Fade,
            by: By::Word,
            over: SECOND,
        };
        let pose = Look::default().pose_expr(4 * SECOND);
        let pieces = pieces(&motion, &units(), &[], (280.0, 60.0), (1920.0, 1080.0), 0.08, &pose);
        assert_eq!(pieces.len(), 3);
        let at = |i: usize, t: f64| pieces[i].pose.at(t);
        assert!(at(0, 0.2).opacity > 0.0 && at(2, 0.2).opacity == 0.0, "the last waits");
        assert!((at(2, 2.0).opacity - 1.0).abs() < 1e-9);
        // The first word's centre: 40 px in from a 280 px picture's centre.
        assert!((at(0, 2.0).x - (40.0 - 140.0) / 1920.0).abs() < 1e-9);
    }

    #[test]
    fn karaoke_follows_word_timings() {
        let motion = TextMotion {
            style: Style::Karaoke,
            ..TextMotion::default()
        };
        let pose = Look::default().pose_expr(4 * SECOND);
        let timing = [0, SECOND, 2 * SECOND];
        let pieces = pieces(&motion, &units(), &timing, (280.0, 60.0), (1920.0, 1080.0), 0.08, &pose);
        let o = |i: usize, t: f64| pieces[i].pose.at(t).opacity;
        assert!(o(1, 0.5) < 0.5 && o(1, 1.2) > 0.99, "word two lights at 1s");
        let rest = pieces[1].pose.at(2.5).y;
        assert!(pieces[1].pose.at(1.5).y < rest, "and stands up while said");
    }

    #[test]
    fn names_read_loosely() {
        assert_eq!(Style::parse("typewriter"), Some(Style::Type));
        assert_eq!(By::parse("letters"), Some(By::Letter));
    }
}
