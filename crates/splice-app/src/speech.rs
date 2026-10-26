//! Hearing what is said in a clip: whisper.cpp's `whisper-cli` on this Mac,
//! with its speech model (downloaded once, checked), giving each word and
//! when it is said. Nothing leaves the machine.

use crate::ffmpeg;
use crate::session::dirs;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use splice_core::captions::unspaced;
use splice_core::time::{Time, secs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
const MODEL_SHA256: &str = "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe";
pub const MODEL_SIZE_MB: u32 = 148;

pub fn model_path() -> PathBuf {
    dirs().data.join("models").join("ggml-base.bin")
}

pub fn has_model() -> bool {
    model_path().exists()
}

/// The recognizer, if installed.
pub fn whisper() -> Option<PathBuf> {
    ffmpeg::tool("whisper-cli")
}

/// What to say when the recognizer is missing.
pub const INSTALL: &str = "Captions need whisper.cpp, which hears speech on this Mac: brew install whisper-cpp";

/// Downloads the speech model if it is not here yet, checking it is the one
/// expected before keeping it. Blocking.
pub fn fetch_model() -> Result<PathBuf> {
    static FETCH: Mutex<()> = Mutex::new(());
    let _one_at_a_time = FETCH.lock();
    let path = model_path();
    if path.exists() {
        return Ok(path);
    }
    let dir = path.parent().expect("a folder");
    std::fs::create_dir_all(dir)?;
    let partial = dir.join("download.part");
    let status = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", "--output"])
        .arg(&partial)
        .arg(MODEL_URL)
        .status()
        .context("running curl")?;
    if !status.success() {
        bail!("could not download the speech model (curl {status})");
    }
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(&partial)?;
    std::io::copy(&mut file, &mut hasher)?;
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if hex != MODEL_SHA256 {
        let _ = std::fs::remove_file(&partial);
        bail!("the downloaded speech model did not match its checksum");
    }
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// A word heard, and when (from the start of what was heard).
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub text: String,
    pub start: Time,
    pub end: Time,
}

/// The words said in `length` of `path` from `from`. Blocking: about a
/// second for every ten of speech.
pub fn transcribe(path: &Path, from: Time, length: Time) -> Result<Vec<Heard>> {
    let whisper = whisper().context(INSTALL)?;
    let model = fetch_model()?;
    // Its own folder: two clips can be heard at once.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let run = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let work = std::env::temp_dir().join(format!("splice-speech-{}-{run}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let wav = work.join("speech.wav");
    // 16 kHz mono, as whisper hears.
    let status = ffmpeg::export_command()?
        .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
        .args([
            "-ss",
            &format!("{:.6}", secs(from)),
            "-t",
            &format!("{:.6}", secs(length)),
            "-i",
        ])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(&wav)
        .status()?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&work);
        bail!("could not read the sound to hear it");
    }
    let output = Command::new(whisper)
        .arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(&wav)
        .args(["-l", "auto", "-ml", "1", "-oj", "-np", "-of"])
        .arg(work.join("words"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("running whisper-cli")?;
    let json = std::fs::read_to_string(work.join("words.json"));
    let _ = std::fs::remove_dir_all(&work);
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        bail!("whisper could not hear it: {}", text.lines().last().unwrap_or(""));
    }
    // Whisper's last times can run past the end of what it heard.
    Ok(parse(&json?)?
        .into_iter()
        .filter(|w| w.start < length)
        .map(|w| Heard {
            end: w.end.min(length),
            ..w
        })
        .collect())
}

/// whisper-cli's JSON, one segment a token (`-ml 1`), as words: a token
/// without a space before it is the rest of the word before ("every" +
/// "body", "work" + "."), except in writing without spaces (Japanese,
/// Chinese), where each token keeps its own time.
fn parse(json: &str) -> Result<Vec<Heard>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let segments = value["transcription"].as_array().cloned().unwrap_or_default();
    let mut words: Vec<Heard> = Vec::new();
    for s in &segments {
        let (Some(text), Some(a), Some(z)) = (
            s["text"].as_str(),
            s["offsets"]["from"].as_i64(),
            s["offsets"]["to"].as_i64(),
        ) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let (start, end) = (a * 1000, z.max(a) * 1000);
        let first = text.chars().next().unwrap_or(' ');
        let rest = !first.is_whitespace()
            && words.last().is_some_and(|w| {
                let last = w.text.chars().last().unwrap_or(' ');
                !(unspaced(last) && unspaced(first))
            });
        match words.last_mut() {
            Some(word) if rest => {
                word.text.push_str(text);
                word.end = word.end.max(end);
            }
            _ => words.push(Heard {
                text: text.trim().to_string(),
                start,
                end,
            }),
        }
    }
    Ok(words
        .into_iter()
        .filter_map(|w| {
            let text = w.text.trim().trim_matches('"').to_string();
            // Whisper marks what it cannot make out in brackets: [MUSIC], (laughs).
            let noise = text.starts_with('[') || text.starts_with('(');
            (!text.is_empty() && !noise).then_some(Heard { text, ..w })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_join_into_words_except_where_words_are_unspaced() {
        let json = r#"{"transcription":[
            {"offsets":{"from":0,"to":100},"text":" Today"},
            {"offsets":{"from":100,"to":200},"text":","},
            {"offsets":{"from":200,"to":300},"text":" I"},
            {"offsets":{"from":300,"to":400},"text":"'m"},
            {"offsets":{"from":400,"to":500},"text":" [MUS"},
            {"offsets":{"from":500,"to":600},"text":"IC]"},
            {"offsets":{"from":600,"to":700},"text":" 今日は"},
            {"offsets":{"from":700,"to":800},"text":"動画"},
            {"offsets":{"from":800,"to":900},"text":"。"}]}"#;
        let words: Vec<(String, Time, Time)> = parse(json)
            .unwrap()
            .into_iter()
            .map(|w| (w.text, w.start / 1000, w.end / 1000))
            .collect();
        assert_eq!(
            words,
            vec![
                ("Today,".into(), 0, 200),
                ("I'm".into(), 200, 400),
                ("今日は".into(), 600, 700),
                // Unspaced tokens stay apart; captions group them.
                ("動画".into(), 700, 800),
                ("。".into(), 800, 900),
            ]
        );
    }

    #[test]
    fn reads_whisper_words() {
        let json = r#"{"transcription":[
            {"offsets":{"from":0,"to":250},"text":""},
            {"offsets":{"from":250,"to":350},"text":" Almost"},
            {"offsets":{"from":2830,"to":3650},"text":" \"AI\""},
            {"offsets":{"from":4000,"to":4500},"text":" [MUSIC]"}]}"#;
        let words = parse(json).unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(
            words[0],
            Heard {
                text: "Almost".into(),
                start: 250_000,
                end: 350_000
            }
        );
        assert_eq!(words[1].text, "AI");
    }

    #[test]
    fn hears_speech_when_whisper_is_here() {
        if whisper().is_none() || !has_model() || !ffmpeg::available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("splice-hear-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let speech = dir.join("speech.aiff");
        let said = Command::new("say")
            .arg("-o")
            .arg(&speech)
            .arg("Hello there, this is a test.")
            .status();
        if !said.is_ok_and(|s| s.success()) {
            return;
        }
        let words = transcribe(&speech, 0, 5 * splice_core::time::SECOND).unwrap();
        let text: Vec<String> = words.iter().map(|w| w.text.to_lowercase()).collect();
        assert!(text.iter().any(|w| w.starts_with("hello")), "{text:?}");
        assert!(words.windows(2).all(|p| p[0].start <= p[1].start));
        let _ = std::fs::remove_dir_all(dir);
    }
}
