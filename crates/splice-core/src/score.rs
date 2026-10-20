//! Music written as a score: instruments, notes in beats, a tempo map. The
//! format is Clapper's (github.com/ArchAstro/clapper, `@archastro/clapper-music`),
//! so a score moves between the two unchanged, and compiles to the same notes.
//! Splice plays it with General MIDI instruments; `program` picks one, or the
//! instrument's name does.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Ticks per quarter-note beat.
pub const PPQ: i64 = 960;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Pitch {
    Midi(u8),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Beats from the start of its clip.
    pub at: f64,
    pub duration: f64,
    pub pitch: Pitch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Phrase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    pub notes: Vec<Note>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transpose: Option<i32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Humanize {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub id: String,
    pub instrument: String,
    /// Clapper calls a part's passages clips.
    #[serde(rename = "clips")]
    pub phrases: Vec<Phrase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<u8>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drums: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverb: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mute: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub humanize: Option<Humanize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing: Option<f64>,
    /// Everything else Clapper knows (controls, bends, drive…), kept as is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TempoPoint {
    pub at: f64,
    pub bpm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tempo {
    Constant(f64),
    Map(Vec<TempoPoint>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeterPoint {
    pub at: f64,
    pub meter: [u32; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Meter {
    Constant([u32; 2]),
    Map(Vec<MeterPoint>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub name: String,
    pub at: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub title: String,
    pub tempo: Tempo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<Meter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Beats; defaults to the end of the last note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    /// Seconds of ring-out after the music ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<f64>,
    #[serde(rename = "tracks")]
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A note as it sounds: when, how long, which key, how hard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sounding {
    pub tick: i64,
    pub ticks: i64,
    pub key: u8,
    pub velocity: u8,
    pub seconds: f64,
    pub length: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledPart {
    pub id: String,
    pub instrument: String,
    /// General MIDI program, 0–127.
    pub program: u8,
    pub drums: bool,
    pub channel: u8,
    pub gain: f64,
    pub pan: f64,
    pub reverb: f64,
    pub mute: bool,
    pub notes: Vec<Sounding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub title: String,
    pub tempos: Vec<(i64, f64)>,
    /// (tick, beats per bar, beat unit)
    pub meters: Vec<(i64, u32, u32)>,
    pub parts: Vec<CompiledPart>,
    pub length_ticks: i64,
    pub music_seconds: f64,
    /// Music plus its tail.
    pub seconds: f64,
    pub markers: Vec<(String, i64, f64)>,
}

impl Compiled {
    pub fn seconds_at(&self, tick: i64) -> f64 {
        ticks_to_seconds(tick, &self.tempos)
    }

    /// The tick sounding `seconds` in.
    pub fn tick_at(&self, seconds: f64) -> i64 {
        let mut remaining = seconds.max(0.0);
        for (i, &(tick, bpm)) in self.tempos.iter().enumerate() {
            let next = self.tempos.get(i + 1).map_or(i64::MAX, |t| t.0);
            let span = (next - tick) as f64 * 60.0 / (bpm * PPQ as f64);
            if remaining <= span || next == i64::MAX {
                return tick + (remaining * bpm * PPQ as f64 / 60.0).round() as i64;
            }
            remaining -= span;
        }
        0
    }

    /// The first beat of every bar up to `end`, with its bar number from one.
    pub fn bars(&self, end: i64) -> Vec<(i64, u32)> {
        let mut out = Vec::new();
        let mut tick = 0;
        let mut number = 1;
        while tick <= end && out.len() < 100_000 {
            let (_, n, d) = self
                .meters
                .iter()
                .rev()
                .find(|m| m.0 <= tick)
                .copied()
                .unwrap_or((0, 4, 4));
            out.push((tick, number));
            tick += (n as i64 * 4 * PPQ / d as i64).max(1);
            number += 1;
        }
        out
    }
}

pub fn ticks_to_seconds(tick: i64, tempos: &[(i64, f64)]) -> f64 {
    let mut seconds = 0.0;
    for (i, &(at, bpm)) in tempos.iter().enumerate() {
        if tick <= at {
            break;
        }
        let next = tempos.get(i + 1).map_or(tick, |t| t.0.min(tick));
        seconds += (next - at) as f64 * 60.0 / (bpm * PPQ as f64);
    }
    seconds
}

/// `C4` is 60, `A4` 69; sharps `#`, flats `b`.
pub fn pitch_to_midi(pitch: &Pitch) -> anyhow::Result<u8> {
    match pitch {
        Pitch::Midi(n) if *n <= 127 => Ok(*n),
        Pitch::Midi(n) => anyhow::bail!("MIDI pitch out of range: {n}"),
        Pitch::Name(name) => {
            let mut chars = name.chars();
            let letter = chars.next().map(|c| c.to_ascii_uppercase());
            let step = match letter {
                Some('C') => 0,
                Some('D') => 2,
                Some('E') => 4,
                Some('F') => 5,
                Some('G') => 7,
                Some('A') => 9,
                Some('B') => 11,
                _ => anyhow::bail!("Invalid note: {name}"),
            };
            let rest: String = chars.collect();
            let (accidental, octave) = match rest.chars().next() {
                Some('#') => (1, &rest[1..]),
                Some('b') => (-1, &rest[1..]),
                _ => (0, rest.as_str()),
            };
            let octave: i32 = octave.parse().map_err(|_| anyhow::anyhow!("Invalid note: {name}"))?;
            let n = (octave + 1) * 12 + step + accidental;
            u8::try_from(n)
                .ok()
                .filter(|n| *n <= 127)
                .ok_or_else(|| anyhow::anyhow!("MIDI pitch out of range: {name}"))
        }
    }
}

/// `60` → `C4`.
pub fn midi_name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[key as usize % 12], key as i32 / 12 - 1)
}

fn check(n: f64, name: &str, min: f64, max: f64) -> anyhow::Result<()> {
    if !n.is_finite() || n < min || n > max {
        anyhow::bail!("{name} must be between {min} and {max}");
    }
    Ok(())
}

/// Clapper's seeded random (mulberry32), so humanized timing matches.
fn rng(seed: i64) -> impl FnMut() -> f64 {
    let mut a = seed as i32;
    move || {
        a = a.wrapping_add(0x6d2b79f5u32 as i32);
        let mut t = ((a ^ ((a as u32) >> 15) as i32) as i64 * (1 | a) as i64) as i32;
        t = (t.wrapping_add(((t ^ ((t as u32) >> 7) as i32) as i64 * (61 | t) as i64) as i32)) ^ t;
        ((t ^ ((t as u32) >> 14) as i32) as u32) as f64 / 4294967296.0
    }
}

impl Score {
    /// Tempo, meter, every note in time order: what plays, and when.
    pub fn compile(&self) -> anyhow::Result<Compiled> {
        if self.title.trim().is_empty() {
            anyhow::bail!("a score needs a title");
        }
        let tick = |beats: f64| -> anyhow::Result<i64> {
            check(beats, "beat", f64::MIN, f64::MAX)?;
            Ok((beats * PPQ as f64).round() as i64)
        };
        let mut tempos = match &self.tempo {
            Tempo::Constant(bpm) => vec![TempoPoint { at: 0.0, bpm: *bpm }],
            Tempo::Map(points) => points.clone(),
        }
        .into_iter()
        .map(|t| {
            check(t.bpm, "tempo", 1.0, 1000.0)?;
            Ok((tick(t.at)?, t.bpm))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
        tempos.sort_by_key(|t| t.0);
        if tempos.first().map(|t| t.0) != Some(0) || tempos.windows(2).any(|w| w[0].0 == w[1].0) {
            anyhow::bail!("the tempo map must start at beat zero and have unique positions");
        }
        let mut meters = match &self.meter {
            None => vec![(0, 4, 4)],
            Some(Meter::Constant([n, d])) => vec![(0, *n, *d)],
            Some(Meter::Map(points)) => points
                .iter()
                .map(|m| Ok((tick(m.at)?, m.meter[0], m.meter[1])))
                .collect::<anyhow::Result<Vec<_>>>()?,
        };
        meters.sort_by_key(|m| m.0);
        for &(_, n, d) in &meters {
            if n < 1 || ![1, 2, 4, 8, 16, 32].contains(&d) {
                anyhow::bail!("invalid meter {n}/{d}");
            }
        }
        if meters.first().map(|m| m.0) != Some(0) {
            anyhow::bail!("the meter map must start at beat zero");
        }

        let mut ids = std::collections::HashSet::new();
        let mut end: i64 = 0;
        let mut channel: u8 = 0;
        let solo = self.parts.iter().any(|p| p.solo);
        let mut random = rng(self.seed.unwrap_or(0));
        let mut parts = Vec::new();
        for part in &self.parts {
            let valid_id = !part.id.is_empty()
                && part
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if !valid_id || !ids.insert(part.id.clone()) {
                anyhow::bail!("invalid or duplicate track id: {}", part.id);
            }
            if part.instrument.trim().is_empty() {
                anyhow::bail!("track {} needs an instrument", part.id);
            }
            check(part.gain.unwrap_or(1.0), "gain", 0.0, 4.0)?;
            check(part.pan.unwrap_or(0.0), "pan", -1.0, 1.0)?;
            check(part.reverb.unwrap_or(0.0), "reverb", 0.0, 1.0)?;
            check(part.swing.unwrap_or(0.0), "swing", 0.0, 1.0)?;
            let timing = part.humanize.as_ref().and_then(|h| h.timing).unwrap_or(0.0);
            let loudness = part.humanize.as_ref().and_then(|h| h.velocity).unwrap_or(0.0);
            check(timing, "timing humanize", 0.0, 0.25)?;
            check(loudness, "velocity humanize", 0.0, 30.0)?;
            if channel == 9 {
                channel += 1;
            }
            let ch = if part.drums {
                9
            } else {
                let c = channel;
                channel = channel.saturating_add(1);
                c
            };
            if ch > 15 {
                anyhow::bail!("a score can have at most 15 instruments besides drums");
            }
            let mut notes = Vec::new();
            for phrase in &part.phrases {
                let length = match phrase.length {
                    Some(l) => l,
                    None => phrase.notes.iter().map(|n| n.at + n.duration).fold(0.0, f64::max),
                };
                let repeat = phrase.repeat.unwrap_or(1);
                check(length, "clip length", 0.0, f64::MAX)?;
                if !(1..=10_000).contains(&repeat) {
                    anyhow::bail!("invalid repeat count {repeat}");
                }
                let origin = phrase.at.unwrap_or(0.0);
                end = end.max(tick(origin + length * repeat as f64)?);
                for r in 0..repeat {
                    for note in &phrase.notes {
                        check(note.at, "note position", f64::MIN, f64::MAX)?;
                        check(note.duration, "note duration", 1.0 / PPQ as f64, f64::MAX)?;
                        let velocity = note.velocity.unwrap_or(80.0);
                        check(velocity, "velocity", 1.0, 127.0)?;
                        let key = pitch_to_midi(&note.pitch)? as i32 + phrase.transpose.unwrap_or(0);
                        let key = u8::try_from(key)
                            .ok()
                            .filter(|k| *k <= 127)
                            .ok_or_else(|| anyhow::anyhow!("MIDI pitch out of range: {key}"))?;
                        let mut at = origin + r as f64 * length + note.at;
                        if (at * 2.0).round() as i64 % 2 == 1 {
                            at += part.swing.unwrap_or(0.0) / 6.0;
                        }
                        at = (at + (random() * 2.0 - 1.0) * timing).max(0.0);
                        let start = tick(at)?;
                        let ticks = tick(note.duration)?.max(1);
                        end = end.max(start + ticks);
                        let velocity = (velocity + (random() * 2.0 - 1.0) * loudness).clamp(1.0, 127.0).round() as u8;
                        let seconds = ticks_to_seconds(start, &tempos);
                        notes.push(Sounding {
                            tick: start,
                            ticks,
                            key,
                            velocity,
                            seconds,
                            length: ticks_to_seconds(start + ticks, &tempos) - seconds,
                        });
                    }
                }
            }
            notes.sort_by_key(|n| (n.tick, n.key));
            parts.push(CompiledPart {
                id: part.id.clone(),
                instrument: part.instrument.clone(),
                program: part.program.unwrap_or_else(|| program_for(&part.instrument)),
                drums: part.drums || is_drums(&part.instrument),
                channel: if is_drums(&part.instrument) { 9 } else { ch },
                gain: part.gain.unwrap_or(1.0),
                pan: part.pan.unwrap_or(0.0),
                reverb: part.reverb.unwrap_or(0.0),
                mute: part.mute || (solo && !part.solo),
                notes,
            });
        }
        let length_ticks = match self.length {
            None => end,
            Some(l) => tick(l)?,
        };
        if length_ticks < end {
            anyhow::bail!("the score's length cuts off notes");
        }
        let tail = self.tail.unwrap_or(2.0);
        check(tail, "tail", 0.0, 30.0)?;
        let music_seconds = ticks_to_seconds(length_ticks, &tempos);
        let markers = self
            .markers
            .iter()
            .map(|m| {
                let t = tick(m.at)?;
                Ok((m.name.clone(), t, ticks_to_seconds(t, &tempos)))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Compiled {
            title: self.title.clone(),
            tempos,
            meters,
            parts,
            length_ticks,
            music_seconds,
            seconds: music_seconds + tail,
            markers,
        })
    }

    /// A short piece to start from: piano chords and a bass line.
    pub fn starter() -> Score {
        let chord = |at: f64, keys: [u8; 3]| -> Vec<Note> {
            keys.iter()
                .map(|&k| Note {
                    at,
                    duration: 3.8,
                    pitch: Pitch::Name(midi_name(k)),
                    velocity: Some(64.0),
                })
                .collect()
        };
        let piano: Vec<Note> = [[60, 64, 67], [57, 60, 64], [53, 57, 60], [55, 59, 62]]
            .iter()
            .enumerate()
            .flat_map(|(i, keys)| chord(i as f64 * 4.0, *keys))
            .collect();
        let bass: Vec<Note> = [36u8, 33, 29, 31]
            .iter()
            .enumerate()
            .map(|(i, &k)| Note {
                at: i as f64 * 4.0,
                duration: 3.5,
                pitch: Pitch::Name(midi_name(k)),
                velocity: Some(72.0),
            })
            .collect();
        let part = |id: &str, instrument: &str, notes: Vec<Note>, gain: f64| Part {
            id: id.into(),
            instrument: instrument.into(),
            phrases: vec![Phrase {
                notes,
                ..Phrase::default()
            }],
            program: None,
            drums: false,
            gain: Some(gain),
            pan: None,
            reverb: Some(0.3),
            mute: false,
            solo: false,
            humanize: None,
            swing: None,
            extra: Map::new(),
        };
        Score {
            title: "Untitled score".into(),
            tempo: Tempo::Constant(96.0),
            meter: Some(Meter::Constant([4, 4])),
            seed: None,
            length: Some(16.0),
            tail: Some(2.0),
            parts: vec![
                part("piano", "acoustic-grand-piano", piano, 0.8),
                part("bass", "acoustic-bass", bass, 0.9),
            ],
            markers: Vec::new(),
            extra: Map::new(),
        }
    }
}

/// The 128 General MIDI instruments, by program number.
pub const GM: [&str; 128] = [
    "acoustic-grand-piano",
    "bright-acoustic-piano",
    "electric-grand-piano",
    "honky-tonk-piano",
    "electric-piano-1",
    "electric-piano-2",
    "harpsichord",
    "clavinet",
    "celesta",
    "glockenspiel",
    "music-box",
    "vibraphone",
    "marimba",
    "xylophone",
    "tubular-bells",
    "dulcimer",
    "drawbar-organ",
    "percussive-organ",
    "rock-organ",
    "church-organ",
    "reed-organ",
    "accordion",
    "harmonica",
    "tango-accordion",
    "nylon-guitar",
    "steel-guitar",
    "jazz-guitar",
    "clean-guitar",
    "muted-guitar",
    "overdriven-guitar",
    "distortion-guitar",
    "guitar-harmonics",
    "acoustic-bass",
    "finger-bass",
    "pick-bass",
    "fretless-bass",
    "slap-bass-1",
    "slap-bass-2",
    "synth-bass-1",
    "synth-bass-2",
    "violin",
    "viola",
    "cello",
    "contrabass",
    "tremolo-strings",
    "pizzicato-strings",
    "orchestral-harp",
    "timpani",
    "string-ensemble-1",
    "string-ensemble-2",
    "synth-strings-1",
    "synth-strings-2",
    "choir-aahs",
    "voice-oohs",
    "synth-voice",
    "orchestra-hit",
    "trumpet",
    "trombone",
    "tuba",
    "muted-trumpet",
    "french-horn",
    "brass-section",
    "synth-brass-1",
    "synth-brass-2",
    "soprano-sax",
    "alto-sax",
    "tenor-sax",
    "baritone-sax",
    "oboe",
    "english-horn",
    "bassoon",
    "clarinet",
    "piccolo",
    "flute",
    "recorder",
    "pan-flute",
    "blown-bottle",
    "shakuhachi",
    "whistle",
    "ocarina",
    "square-lead",
    "sawtooth-lead",
    "calliope-lead",
    "chiff-lead",
    "charang-lead",
    "voice-lead",
    "fifths-lead",
    "bass-lead",
    "new-age-pad",
    "warm-pad",
    "polysynth-pad",
    "choir-pad",
    "bowed-pad",
    "metallic-pad",
    "halo-pad",
    "sweep-pad",
    "rain",
    "soundtrack",
    "crystal",
    "atmosphere",
    "brightness",
    "goblins",
    "echoes",
    "sci-fi",
    "sitar",
    "banjo",
    "shamisen",
    "koto",
    "kalimba",
    "bagpipe",
    "fiddle",
    "shanai",
    "tinkle-bell",
    "agogo",
    "steel-drums",
    "woodblock",
    "taiko-drum",
    "melodic-tom",
    "synth-drum",
    "reverse-cymbal",
    "guitar-fret-noise",
    "breath-noise",
    "seashore",
    "bird-tweet",
    "telephone-ring",
    "helicopter",
    "applause",
    "gunshot",
];

/// Names that play the drum kit (General MIDI channel 10).
pub fn is_drums(instrument: &str) -> bool {
    let i = instrument.to_ascii_lowercase();
    i == "drums" || i.contains("drum-kit") || i.ends_with("-drums") || i == "gm-style-perc" || i == "percussion"
}

/// The General MIDI program an instrument name means: a GM name exactly, or
/// the nearest family for other names (Clapper's sampled instruments too).
pub fn program_for(instrument: &str) -> u8 {
    let i = instrument.to_ascii_lowercase();
    if let Some(p) = GM.iter().position(|g| *g == i) {
        return p as u8;
    }
    let has = |w: &str| i.contains(w);
    let program = if has("pizz") {
        45
    } else if has("trem") {
        44
    } else if has("violin") && (has("ens") || has("section")) || has("strings") {
        48
    } else if has("viola") {
        41
    } else if has("cello") {
        42
    } else if has("contrabass") || has("double-bass") {
        43
    } else if has("violin") || has("fiddle") {
        40
    } else if has("piccolo") {
        72
    } else if has("flute") {
        73
    } else if has("oboe") {
        68
    } else if has("clarinet") {
        71
    } else if has("bassoon") {
        70
    } else if has("horn") {
        60
    } else if has("trumpet") && has("mute") {
        59
    } else if has("trumpet") {
        56
    } else if has("trombone") {
        57
    } else if has("tuba") {
        58
    } else if has("harp") {
        46
    } else if has("glocken") {
        9
    } else if has("marimba") {
        12
    } else if has("xylo") {
        13
    } else if has("bell") {
        14
    } else if has("timpani") {
        47
    } else if has("organ") {
        19
    } else if has("guitar") && (has("dist") || has("drive") || has("green")) {
        29
    } else if has("guitar") {
        25
    } else if has("bass") {
        33
    } else if has("sax") {
        65
    } else if has("choir") || has("voice") {
        52
    } else if has("pad") {
        89
    } else if has("synth") {
        81
    } else {
        0
    };
    program as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitches() {
        assert_eq!(pitch_to_midi(&Pitch::Name("C4".into())).unwrap(), 60);
        assert_eq!(pitch_to_midi(&Pitch::Name("A4".into())).unwrap(), 69);
        assert_eq!(pitch_to_midi(&Pitch::Name("F#5".into())).unwrap(), 78);
        assert_eq!(pitch_to_midi(&Pitch::Name("Bb2".into())).unwrap(), 46);
        assert_eq!(pitch_to_midi(&Pitch::Midi(36)).unwrap(), 36);
        assert!(pitch_to_midi(&Pitch::Name("H2".into())).is_err());
        assert_eq!(midi_name(61), "C#4");
    }

    #[test]
    fn reads_clapper_json_and_keeps_what_it_does_not_use() {
        let json = r#"{
            "title": "Waltz", "tempo": [{"at": 0, "bpm": 120}, {"at": 4, "bpm": 60}], "meter": [3, 4],
            "tail": 1,
            "tracks": [
                {"id": "piano", "instrument": "vsupright1", "program": 0, "gain": 0.6,
                 "clips": [{"notes": [{"at": 0, "duration": 1, "pitch": "C4"}, {"at": 1, "duration": 1, "pitch": 64, "velocity": 100}], "repeat": 3, "length": 2}],
                 "controls": [{"at": 2, "cc": 11, "value": 90}]},
                {"id": "flute", "instrument": "flute-sus-nv", "clips": [{"at": 4, "notes": [{"at": 0, "duration": 2, "pitch": "G5"}], "transpose": -12}]}
            ]
        }"#;
        let score: Score = serde_json::from_str(json).unwrap();
        let compiled = score.compile().unwrap();
        assert_eq!(compiled.parts.len(), 2);
        let piano = &compiled.parts[0];
        assert_eq!(piano.notes.len(), 6);
        assert_eq!(piano.notes[2].tick, 2 * PPQ);
        assert_eq!(compiled.parts[1].program, 73, "a flute plays the GM flute");
        assert_eq!(compiled.parts[1].notes[0].key, 67);
        // Four beats at 120 (2s), then two at 60 (2s).
        assert!((compiled.parts[1].notes[0].seconds - 2.0).abs() < 1e-9);
        assert!((compiled.music_seconds - 4.0).abs() < 1e-9);
        assert!((compiled.seconds - 5.0).abs() < 1e-9);
        assert_eq!(compiled.tick_at(3.0), 5 * PPQ);
        // Controls survive a round trip.
        let back = serde_json::to_string(&score).unwrap();
        assert!(back.contains("\"controls\""));
        assert!(back.contains("\"clips\""));
        assert_eq!(serde_json::from_str::<Score>(&back).unwrap(), score);
    }

    #[test]
    fn rejects_what_clapper_rejects() {
        let mut score = Score::starter();
        score.parts[1].id = "piano".into();
        assert!(score.compile().is_err());
        let mut score = Score::starter();
        score.length = Some(4.0);
        assert!(score.compile().is_err());
        let mut score = Score::starter();
        score.tempo = Tempo::Map(vec![TempoPoint { at: 1.0, bpm: 90.0 }]);
        assert!(score.compile().is_err());
    }

    #[test]
    fn drums_go_on_channel_ten() {
        let mut score = Score::starter();
        score.parts[1].instrument = "drums".into();
        let compiled = score.compile().unwrap();
        assert!(compiled.parts[1].drums);
        assert_eq!(compiled.parts[1].channel, 9);
        assert_eq!(compiled.parts[0].channel, 0);
    }

    #[test]
    fn humanize_matches_clapper_random() {
        // mulberry32(1) → first values, as JavaScript computes them.
        let mut r = rng(1);
        let a = r();
        assert!((a - 0.6270739405881613).abs() < 1e-12, "{a}");
    }

    #[test]
    fn bars_follow_the_meter() {
        let mut score = Score::starter();
        score.meter = Some(Meter::Constant([3, 4]));
        score.length = Some(18.0);
        let c = score.compile().unwrap();
        let bars = c.bars(c.length_ticks);
        assert_eq!(bars[1], (3 * PPQ, 2));
        assert_eq!(bars.len(), 7);
    }
}
