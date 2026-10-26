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
    /// No caption goes past this (the end of the clip heard).
    pub until: Option<Time>,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            words: 6,
            chars: 36,
            karaoke: true,
            until: None,
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
    let words = tidy(words);
    // Phrases first: a sentence ends, or the speaker pauses.
    let mut phrases: Vec<Vec<Word>> = Vec::new();
    let mut phrase: Vec<Word> = Vec::new();
    for word in words.iter().filter(|w| !w.text.trim().is_empty()) {
        let paused = phrase.last().is_some_and(|last| word.start - last.end >= PAUSE);
        let ended = phrase.last().is_some_and(|last| {
            last.text
                .trim_end()
                .ends_with(['.', '?', '!', ':', ';', '。', '？', '！'])
        });
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
        // Unspaced writing (Chinese, Japanese) runs about twice as wide a character.
        let chars: usize = phrase
            .iter()
            .map(|w| w.text.chars().map(|c| if unspaced(c) { 2 } else { 1 }).sum::<usize>() + 1)
            .sum();
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

/// Whether a character is written without spaces between words (Chinese,
/// Japanese, Thai, and their punctuation), so words are found another way.
/// Korean has spaces.
pub fn unspaced(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0xff00..=0xffef | 0x0e00..=0x0e7f)
}

/// Kinds of unspaced character, for finding where a word ends.
#[derive(Clone, Copy, PartialEq)]
enum Script {
    Hiragana,
    Katakana,
    Han,
    Mark,
    Other,
}

fn script(c: char) -> Script {
    match c as u32 {
        0x3040..=0x309f => Script::Hiragana,
        0x30a0..=0x30ff => Script::Katakana,
        0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff => Script::Han,
        0x3000..=0x303f | 0xff00..=0xffef => Script::Mark,
        _ => Script::Other,
    }
}

/// The most characters in one piece of unspaced writing.
const PIECE: usize = 8;

/// Whether a new piece of unspaced writing starts at `next`, after `piece`:
/// after punctuation, where kana that ended a phrase meets the kanji or
/// katakana that starts the next (Japanese), or when it has grown long
/// (Chinese, which this cannot read further).
fn breaks(piece: &str, next: char) -> bool {
    let Some(last) = piece.chars().last() else {
        return false;
    };
    let (last, now) = (script(last), script(next));
    if now == Script::Mark {
        return false;
    }
    last == Script::Mark
        || (last == Script::Hiragana && matches!(now, Script::Han | Script::Katakana))
        || piece.chars().count() >= PIECE
}

/// Words in order, each starting after the last and ending after it
/// starts; unspaced writing (heard a character, a token or a whole sentence
/// at a time) regrouped into short pieces that read as words, each taking
/// its share of the time.
fn tidy(words: &[Word]) -> Vec<Word> {
    let mut sorted: Vec<Word> = words.iter().filter(|w| !w.text.trim().is_empty()).cloned().collect();
    sorted.sort_by_key(|w| w.start);
    // Unspaced writing a character at a time, with its time.
    let mut out: Vec<Word> = Vec::new();
    let mut piece: Option<Word> = None;
    for word in sorted {
        let text = word.text.trim().to_string();
        let chars: Vec<char> = text.chars().collect();
        if !chars.iter().any(|c| unspaced(*c)) {
            out.extend(piece.take());
            out.push(Word { text, ..word });
            continue;
        }
        let span = (word.end - word.start).max(0);
        let n = chars.len().max(1) as Time;
        for (i, c) in chars.iter().enumerate() {
            let (a, z) = (
                word.start + span * i as Time / n,
                word.start + span * (i as Time + 1) / n,
            );
            if c.is_whitespace() || *c == WORD_BREAK {
                out.extend(piece.take());
                continue;
            }
            // Latin letters inside it ("iPhoneで") stay together.
            let latin = |p: &Word| !unspaced(*c) && p.text.chars().last().is_some_and(|l| !unspaced(l));
            match piece.as_mut() {
                Some(p) if latin(p) || (unspaced(*c) && !breaks(&p.text, *c)) => {
                    p.text.push(*c);
                    p.end = z;
                }
                _ => {
                    out.extend(piece.take());
                    piece = Some(Word {
                        text: c.to_string(),
                        start: a,
                        end: z,
                    });
                }
            }
        }
    }
    out.extend(piece);
    // Starts that never run backwards or tie, ends after starts.
    let mut last = Time::MIN;
    for w in &mut out {
        w.start = w.start.max(last + 1);
        w.end = w.end.max(w.start + 1);
        last = w.start;
    }
    out
}

/// Between words of unspaced writing: no space shows, but each still lights
/// up on its own.
pub const WORD_BREAK: char = '\u{200B}';

/// Words joined as written: spaced, except within unspaced writing.
fn join(line: &[Word]) -> String {
    let mut text = String::new();
    for w in line {
        let glued = text.chars().last().is_some_and(unspaced) && w.text.chars().next().is_some_and(unspaced);
        if !text.is_empty() {
            text.push(if glued { WORD_BREAK } else { ' ' });
        }
        text.push_str(&w.text);
    }
    text
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
        let (base, look) = style((self.settings.width, self.settings.height));
        // No longer than fits across the canvas at the caption's size (a
        // bold letter is about 0.6 of its size wide), with a margin.
        let (w, h) = (self.settings.width as f32, self.settings.height as f32);
        let fits = ((w * 0.86) / (base.size * h * 0.6)).floor().max(8.0) as usize;
        let layout = Layout {
            chars: layout.chars.min(fits),
            words: layout.words.min((fits / 5).max(2)),
            ..*layout
        };
        let mut lines = lines(words, &layout);
        // A line starting within two frames of the one before joins it, so
        // none is too short to show (or would have to go on another track).
        let frame = self.frame();
        let mut kept: Vec<Vec<Word>> = Vec::new();
        for line in lines.drain(..) {
            match kept.last_mut() {
                Some(before) if line[0].start - before[0].start < 2 * frame => before.extend(line),
                _ => kept.push(line),
            }
        }
        let lines = kept;
        let layout = &layout;
        let track = self.tracks.len();
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
            let until = layout.until.unwrap_or(Time::MAX);
            let end = end.max(start + SHORTEST).min(next).min(until).max(start + frame);
            let title = Title {
                text: join(line),
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
        let mut cues = cues.to_vec();
        cues.sort_by_key(|c| c.0);
        let track = self.tracks.len();
        let mut ids = Vec::new();
        let mut last_end = 0;
        for (a, z, words) in &cues {
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
    fn unspaced_writing_is_grouped_where_words_end() {
        let pieces = |words: &[Word]| -> Vec<String> { tidy(words).into_iter().map(|w| w.text).collect() };
        // As whisper hears Japanese, a token at a time.
        let tokens = said(&[
            ("今日は", 0.1, 0.7),
            ("動画", 0.7, 1.0),
            ("の", 1.0, 1.1),
            ("編", 1.1, 1.7),
            ("集", 1.7, 1.9),
            ("方", 1.9, 2.1),
            ("法", 2.1, 2.3),
            ("を", 2.3, 2.5),
            ("紹", 2.5, 2.7),
            ("介", 2.7, 2.8),
            ("します。", 2.8, 3.6),
            // Joined as heard (`speech::parse`): no space before "で".
            ("iPhoneで", 3.7, 4.2),
        ]);
        assert_eq!(
            pieces(&tokens),
            vec!["今日は", "動画の", "編集方法を", "紹介します。", "iPhoneで"]
        );
        let t = tidy(&tokens);
        assert_eq!(
            (t[2].start, t[2].end),
            (1_100_000, 2_500_000),
            "each piece keeps its own time"
        );
        // Or a whole sentence at once: the same pieces, time shared out.
        let whole = said(&[("今日は動画の編集方法を紹介します", 0.0, 3.2)]);
        assert_eq!(pieces(&whole), vec!["今日は", "動画の", "編集方法を", "紹介します"]);
        // Chinese has no kana to go by: punctuation, and pieces of a few characters.
        let chinese = said(&[("今天我想给你们介绍一下我是怎么剪辑视频的，很简单。", 0.0, 5.0)]);
        let p = pieces(&chinese);
        assert!(p.iter().all(|p| p.chars().count() <= PIECE), "{p:?}");
        assert!(p.iter().any(|p| p.ends_with('，')), "{p:?}");
        // Korean is spaced: its words stay as heard, joined with spaces.
        let korean = said(&[("오늘은", 0.0, 0.5), ("영상을", 0.5, 1.0)]);
        assert_eq!(join(&tidy(&korean)), "오늘은 영상을");
        assert_eq!(
            join(&tidy(&tokens)[..2]),
            "今日は\u{200B}動画の",
            "unspaced words keep a break between"
        );
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
        // Never past the end of the clip heard.
        let mut q = Project::default();
        let until = Layout {
            until: Some(4_500_000),
            ..Layout::default()
        };
        let ids = q.add_captions(&words, &until);
        assert_eq!(q.clip(ids[1]).unwrap().end(), 4_500_000);
    }

    #[test]
    fn captions_fit_the_canvas_and_never_stack() {
        let mut p = Project::default();
        (p.settings.width, p.settings.height) = (1080, 1920);
        let long: Vec<Word> = "one two three four five six seven eight nine ten eleven twelve"
            .split(' ')
            .enumerate()
            .map(|(i, w)| Word {
                text: w.into(),
                start: i as Time * 200_000,
                end: i as Time * 200_000 + 150_000,
            })
            .collect();
        let ids = p.add_captions(&long, &Layout::default());
        for id in &ids {
            let Some(Generator::Text(t)) = &p.clip(*id).unwrap().generator else {
                panic!()
            };
            assert!(t.text.chars().count() <= 24, "fits a portrait canvas: {}", t.text);
        }
        // Words heard at one moment (whisper does this) do not stack captions.
        let mut q = Project::default();
        let tied = vec![
            Word {
                text: "Yes.".into(),
                start: SECOND,
                end: SECOND,
            },
            Word {
                text: "And".into(),
                start: SECOND,
                end: SECOND,
            },
            Word {
                text: "here".into(),
                start: SECOND,
                end: SECOND,
            },
        ];
        let ids = q.add_captions(&tied, &Layout::default());
        assert!(
            ids.iter()
                .all(|id| q.slot(*id).unwrap().track == q.slot(ids[0]).unwrap().track)
        );
        // Chinese heard as one long "word" is cut into lines.
        let mut r = Project::default();
        let heard = vec![Word {
            text: "我們今天討論人工智能的發展和未來的挑戰".into(),
            start: 0,
            end: 4 * SECOND,
        }];
        let ids = r.add_captions(&heard, &Layout::default());
        assert!(ids.len() >= 2, "{}", ids.len());
        let Some(Generator::Text(t)) = &r.clip(ids[0]).unwrap().generator else {
            panic!()
        };
        assert!(!t.text.contains(' '), "written without spaces: {}", t.text);
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
