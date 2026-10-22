//! Captions: words heard in a clip, with when each is said, laid out as
//! titles along the timeline. A line holds a few words, breaks at the end
//! of a sentence or a pause, and lights each word up as it is said.

use crate::look::Look;
use crate::model::{ClipId, Generator, Project, Title};
use crate::textmotion::{By, Style, TextMotion};
use crate::time::{SECOND, Time};

/// A word and when it is said, in timeline time.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: Time,
    pub end: Time,
}

/// How captions read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The most words on one line.
    pub words: usize,
    /// The most characters on one line.
    pub chars: usize,
    /// Light words up as they are said (else each line appears whole).
    pub karaoke: bool,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            words: 6,
            chars: 36,
            karaoke: true,
        }
    }
}

/// A pause this long or longer starts a new line.
const PAUSE: Time = SECOND * 7 / 10;
/// A line stays up at least this long.
const SHORTEST: Time = SECOND * 7 / 10;

/// Words grouped into lines: each sentence (or stretch between pauses)
/// split into as few lines as fit `layout.words` words and `layout.chars`
/// characters, its words shared evenly, so no line is left with one word.
pub fn lines(words: &[Word], layout: &Layout) -> Vec<Vec<Word>> {
    // Phrases first: a sentence ends, or the speaker pauses.
    let mut phrases: Vec<Vec<Word>> = Vec::new();
    let mut phrase: Vec<Word> = Vec::new();
    for word in words.iter().filter(|w| !w.text.trim().is_empty()) {
        let paused = phrase.last().is_some_and(|last| word.start - last.end >= PAUSE);
        let ended = phrase
            .last()
            .is_some_and(|last| last.text.trim_end().ends_with(['.', '?', '!', ':', ';']));
        if !phrase.is_empty() && (paused || ended) {
            phrases.push(std::mem::take(&mut phrase));
        }
        phrase.push(Word {
            text: word.text.trim().to_string(),
            ..word.clone()
        });
    }
    if !phrase.is_empty() {
        phrases.push(phrase);
    }
    let mut out = Vec::new();
    for phrase in phrases {
        let chars: usize = phrase.iter().map(|w| w.text.chars().count() + 1).sum();
        let count = phrase
            .len()
            .div_ceil(layout.words.max(1))
            .max(chars.div_ceil(layout.chars.max(8)))
            .clamp(1, phrase.len());
        // Words shared out evenly, the earlier lines taking any extra.
        let (each, extra) = (phrase.len() / count, phrase.len() % count);
        let mut rest = phrase.into_iter();
        for i in 0..count {
            let take = each + usize::from(i < extra);
            out.push(rest.by_ref().take(take).collect());
        }
    }
    out
}

/// The title a caption line becomes, and where it sits: big, light, on a
/// dark box, low in the frame.
pub fn style(canvas: (u32, u32)) -> (Title, Look) {
    let portrait = canvas.1 > canvas.0;
    (
        Title {
            text: String::new(),
            font: "Avenir Next".into(),
            size: if portrait { 0.034 } else { 0.052 },
            bold: true,
            italic: false,
            color: 0xffffffff,
            align: crate::model::Align::Center,
            background: Some(0x000000b3),
            shadow: false,
            animate: None,
            timing: Vec::new(),
        },
        Look {
            y: if portrait { 0.28 } else { 0.36 },
            ..Look::default()
        },
    )
}

impl Project {
    /// Lays captions over the movie, on a new track on top: each line a
    /// title from its first word to just before the next line. Returns them.
    pub fn add_captions(&mut self, words: &[Word], layout: &Layout) -> Vec<ClipId> {
        let lines = lines(words, layout);
        let (base, look) = style((self.settings.width, self.settings.height));
        let track = self.tracks.len();
        let frame = self.frame();
        let mut ids = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let start = line[0].start.max(0);
            let said = line.last().map_or(start, |w| w.end);
            // Up until the next line (a short gap closes), never overlapping it.
            let next = lines.get(i + 1).map_or(Time::MAX, |l| l[0].start);
            let end = if next - said < SECOND / 2 {
                next
            } else {
                said + SECOND / 5
            };
            let end = end.max(start + SHORTEST).min(next).max(start + frame);
            let title = Title {
                text: line.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
                animate: layout.karaoke.then_some(TextMotion {
                    style: Style::Karaoke,
                    by: By::Word,
                    over: SECOND,
                }),
                timing: if layout.karaoke {
                    line.iter().map(|w| (w.start - start).max(0)).collect()
                } else {
                    Vec::new()
                },
                ..base.clone()
            };
            let id = self.insert_generated(Generator::Text(title), track, start, end - start);
            self.set_look(id, |l| *l = look.clone());
            ids.push(id);
        }
        ids
    }
}

/// Reads SubRip (.srt) or WebVTT (.vtt) captions: (start, end, words).
pub fn parse_subtitles(text: &str) -> Vec<(Time, Time, String)> {
    let stamp = |s: &str| -> Option<Time> {
        let s = s.trim().replace(',', ".");
        let mut parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            parts.insert(0, "0");
        }
        let [h, m, sec] = parts[..] else { return None };
        let seconds = h.parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?;
        Some(crate::time::from_secs(seconds))
    };
    let mut out = Vec::new();
    let text = text.replace("\r\n", "\n");
    for block in text.split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty());
        let Some(timing) = lines.by_ref().find(|l| l.contains("-->")) else {
            continue;
        };
        let Some((a, z)) = timing.split_once("-->") else {
            continue;
        };
        // A cue's settings (VTT) follow its end time.
        let z = z.split_whitespace().next().unwrap_or("");
        let (Some(a), Some(z)) = (stamp(a), stamp(z)) else {
            continue;
        };
        let words: Vec<String> = lines
            .map(|l| {
                // Without tags (<i>, <b>, <c.color>).
                let mut plain = String::new();
                let mut inside = false;
                for c in l.chars() {
                    match c {
                        '<' => inside = true,
                        '>' => inside = false,
                        c if !inside => plain.push(c),
                        _ => {}
                    }
                }
                plain.trim().to_string()
            })
            .filter(|l| !l.is_empty())
            .collect();
        if z > a && !words.is_empty() {
            out.push((a, z, words.join("\n")));
        }
    }
    out
}

impl Project {
    /// Lays captions read from a subtitle file over the movie, on a new track
    /// on top, each line appearing whole. Returns them.
    pub fn add_subtitles(&mut self, cues: &[(Time, Time, String)]) -> Vec<ClipId> {
        let (base, look) = style((self.settings.width, self.settings.height));
        let track = self.tracks.len();
        let mut ids = Vec::new();
        let mut last_end = 0;
        for (a, z, words) in cues {
            let start = (*a).max(last_end);
            if *z <= start {
                continue;
            }
            let title = Title {
                text: words.clone(),
                ..base.clone()
            };
            let id = self.insert_generated(Generator::Text(title), track, start, *z - start);
            self.set_look(id, |l| *l = look.clone());
            last_end = *z;
            ids.push(id);
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(words: &[(&str, f64, f64)]) -> Vec<Word> {
        words
            .iter()
            .map(|(t, a, z)| Word {
                text: t.to_string(),
                start: crate::time::from_secs(*a),
                end: crate::time::from_secs(*z),
            })
            .collect()
    }

    #[test]
    fn lines_break_at_sentences_pauses_and_length() {
        let words = said(&[
            ("Hello", 0.0, 0.3),
            ("there.", 0.3, 0.6),
            ("This", 0.7, 0.9),
            ("is", 0.9, 1.0),
            ("a", 1.0, 1.1),
            ("test", 1.1, 1.4),
            ("after", 2.6, 2.9),
            ("a", 2.9, 3.0),
            ("pause", 3.0, 3.4),
        ]);
        let lines = lines(&words, &Layout::default());
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(text, vec!["Hello there.", "This is a test", "after a pause"]);
        // A long sentence splits evenly, never leaving one word behind.
        let long: Vec<Word> = "Almost everybody has a different setup for using AI at home or work."
            .split(' ')
            .enumerate()
            .map(|(i, w)| Word {
                text: w.into(),
                start: i as Time * 300_000,
                end: i as Time * 300_000 + 250_000,
            })
            .collect();
        let split: Vec<usize> = super::lines(&long, &Layout::default()).iter().map(Vec::len).collect();
        assert_eq!(split, vec![5, 4, 4]);
    }

    #[test]
    fn captions_are_titles_timed_to_their_words() {
        let mut p = Project::default();
        let words = said(&[
            ("One", 1.0, 1.4),
            ("two", 1.5, 1.9),
            ("three.", 2.0, 2.5),
            ("Four", 4.0, 4.4),
        ]);
        let ids = p.add_captions(&words, &Layout::default());
        assert_eq!(ids.len(), 2);
        let first = p.clip(ids[0]).unwrap();
        assert_eq!((first.start, first.end()), (SECOND, 2_700_000));
        let Some(Generator::Text(title)) = &first.generator else {
            panic!()
        };
        assert_eq!(title.text, "One two three.");
        assert_eq!(title.timing, vec![0, 500_000, SECOND]);
        assert!(title.animate.is_some_and(|m| m.style == Style::Karaoke));
        assert_eq!(p.slot(ids[1]).unwrap().track, p.slot(ids[0]).unwrap().track);
    }

    #[test]
    fn reads_srt_and_vtt() {
        let srt =
            "1\n00:00:01,000 --> 00:00:02,500\nHello <i>there</i>\n\n2\n00:00:03,000 --> 00:00:04,000\nSecond\nline\n";
        let cues = parse_subtitles(srt);
        assert_eq!(
            cues,
            vec![
                (SECOND, 2_500_000, "Hello there".to_string()),
                (3 * SECOND, 4 * SECOND, "Second\nline".to_string()),
            ]
        );
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000 align:center\nShort form\n";
        assert_eq!(
            parse_subtitles(vtt),
            vec![(SECOND, 2 * SECOND, "Short form".to_string())]
        );
    }
}
