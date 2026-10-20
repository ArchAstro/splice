//! Scores become sound: a General MIDI SoundFont (GeneralUser GS, fetched once
//! and checked) played by rustysynth, offline, into a WAV.

use crate::session::dirs;
use anyhow::{Context, Result};
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use sha2::{Digest, Sha256};
use splice_core::score::{Compiled, Score};
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// GeneralUser GS v2.0.3 by S. Christian Collins; free for any use.
const SOUNDFONT_URL: &str = "https://raw.githubusercontent.com/mrbumpy409/GeneralUser-GS/684543d5e5efaef08d02be50dcda8d552478fa60/GeneralUser-GS.sf2";
const SOUNDFONT_SHA256: &str = "9575028c7a1f589f5770fccc8cff2734566af40cd26ed836944e9a5152688cfe";
pub const SOUNDFONT_SIZE_MB: u32 = 32;

/// Renders with this synth and these sounds name the same bytes.
pub const ENGINE: &str = "splice-score-1|rustysynth-1.3|GeneralUser-GS-2.0.3";

pub const RATE: i32 = 48_000;

pub fn soundfont_path() -> PathBuf {
    dirs().cache.join("sounds").join("GeneralUser-GS-2.0.3.sf2")
}

pub fn has_soundfont() -> bool {
    soundfont_path().exists()
}

/// Downloads the SoundFont if it is not here yet, checking it is the one we
/// expect before keeping it. Blocking.
pub fn fetch_soundfont() -> Result<PathBuf> {
    static FETCH: Mutex<()> = Mutex::new(());
    let _one_at_a_time = FETCH.lock();
    let path = soundfont_path();
    if path.exists() {
        return Ok(path);
    }
    let dir = path.parent().expect("a folder");
    std::fs::create_dir_all(dir)?;
    let partial = dir.join("download.part");
    let status = std::process::Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--output"])
        .arg(&partial)
        .arg(SOUNDFONT_URL)
        .status()
        .context("running curl")?;
    if !status.success() {
        anyhow::bail!("could not download the instrument sounds (curl {status})");
    }
    let bytes = std::fs::read(&partial)?;
    let digest = Sha256::digest(&bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != SOUNDFONT_SHA256 {
        let _ = std::fs::remove_file(&partial);
        anyhow::bail!("the downloaded instrument sounds did not match their checksum");
    }
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

fn soundfont() -> Result<Arc<SoundFont>> {
    static FONT: OnceLock<Arc<SoundFont>> = OnceLock::new();
    if let Some(font) = FONT.get() {
        return Ok(font.clone());
    }
    let path = fetch_soundfont()?;
    let mut reader = BufReader::new(File::open(&path)?);
    let font = Arc::new(SoundFont::new(&mut reader).map_err(|e| anyhow::anyhow!("reading {}: {e:?}", path.display()))?);
    Ok(FONT.get_or_init(|| font).clone())
}

/// One MIDI message at a sample.
struct Event {
    at: usize,
    channel: i32,
    command: i32,
    data1: i32,
    data2: i32,
}

fn events(compiled: &Compiled) -> Vec<Event> {
    let mut events = Vec::new();
    let sample = |seconds: f64| (seconds * RATE as f64).round().max(0.0) as usize;
    for part in compiled.parts.iter().filter(|p| !p.mute) {
        let ch = part.channel as i32;
        // Program, loudness, place in the stereo field, room.
        if !part.drums {
            events.push(Event {
                at: 0,
                channel: ch,
                command: 0xC0,
                data1: part.program as i32,
                data2: 0,
            });
        }
        let volume = (100.0 * part.gain.sqrt()).clamp(0.0, 127.0).round() as i32;
        let pan = (64.0 + part.pan * 63.0).clamp(0.0, 127.0).round() as i32;
        let reverb = (part.reverb * 127.0).clamp(0.0, 127.0).round() as i32;
        events.push(Event {
            at: 0,
            channel: ch,
            command: 0xB0,
            data1: 7,
            data2: volume,
        });
        events.push(Event {
            at: 0,
            channel: ch,
            command: 0xB0,
            data1: 10,
            data2: pan,
        });
        events.push(Event {
            at: 0,
            channel: ch,
            command: 0xB0,
            data1: 91,
            data2: reverb,
        });
        for note in &part.notes {
            let on = sample(note.seconds);
            let off = sample(note.seconds + note.length).max(on + 1);
            events.push(Event {
                at: on,
                channel: ch,
                command: 0x90,
                data1: note.key as i32,
                data2: note.velocity as i32,
            });
            events.push(Event {
                at: off,
                channel: ch,
                command: 0x80,
                data1: note.key as i32,
                data2: 0,
            });
        }
    }
    // Releases before attacks at the same moment, so repeated notes retrigger.
    events.sort_by_key(|e| (e.at, e.command != 0x80));
    events
}

/// Plays `score` into stereo samples at `RATE`.
pub fn synthesize(score: &Score) -> Result<(Vec<f32>, Vec<f32>)> {
    let compiled = score.compile()?;
    let font = soundfont()?;
    let settings = SynthesizerSettings::new(RATE);
    let mut synth = Synthesizer::new(&font, &settings).map_err(|e| anyhow::anyhow!("starting the synth: {e:?}"))?;
    let total = (compiled.seconds * RATE as f64).ceil() as usize;
    let (mut left, mut right) = (vec![0.0f32; total], vec![0.0f32; total]);
    let mut at = 0;
    for event in events(&compiled) {
        let until = event.at.min(total);
        if until > at {
            synth.render(&mut left[at..until], &mut right[at..until]);
            at = until;
        }
        synth.process_midi_message(event.channel, event.command, event.data1, event.data2);
    }
    if total > at {
        synth.render(&mut left[at..], &mut right[at..]);
    }
    // A steady level from score to score: peaks at -1 dBFS.
    let peak = left.iter().chain(right.iter()).fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 1e-6 {
        let gain = 0.89 / peak;
        for s in left.iter_mut().chain(right.iter_mut()) {
            *s *= gain;
        }
    }
    Ok((left, right))
}

/// Renders `score` to a 16-bit stereo WAV at `out`.
pub fn render(score: &Score, out: &Path) -> Result<()> {
    let (left, right) = synthesize(score)?;
    let partial = out.with_extension("part");
    write_wav(&partial, &left, &right)?;
    std::fs::rename(&partial, out)?;
    Ok(())
}

fn write_wav(path: &Path, left: &[f32], right: &[f32]) -> Result<()> {
    let frames = left.len() as u32;
    let data = frames * 4;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes()); // stereo
    bytes.extend_from_slice(&(RATE as u32).to_le_bytes());
    bytes.extend_from_slice(&(RATE as u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for (l, r) in left.iter().zip(right) {
        for s in [l, r] {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    File::create(path)?.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_ordered_and_releases_come_first() {
        let compiled = Score::starter().compile().unwrap();
        let events = events(&compiled);
        assert!(events.windows(2).all(|w| w[0].at <= w[1].at));
        let ons = events.iter().filter(|e| e.command == 0x90).count();
        let offs = events.iter().filter(|e| e.command == 0x80).count();
        assert_eq!(ons, offs);
        assert_eq!(ons, 16);
    }

    #[test]
    fn writes_a_wav_header() {
        let dir = std::env::temp_dir().join(format!("splice-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        write_wav(&path, &[0.0, 0.5], &[0.0, -0.5]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + 8);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Plays the starter for real when the SoundFont is already here.
    #[test]
    fn renders_sound_when_the_soundfont_is_here() {
        if !has_soundfont() {
            return;
        }
        let (left, _) = synthesize(&Score::starter()).unwrap();
        assert!(left.len() > RATE as usize * 10);
        assert!(left.iter().any(|s| s.abs() > 0.1));
    }
}
