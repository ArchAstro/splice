//! Hearing what is said in a clip: whisper.cpp's `whisper-cli` on this Mac,
//! with its speech model (downloaded once, checked), giving each word and
//! when it is said. Nothing leaves the machine.

use crate::ffmpeg;
use crate::session::dirs;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
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
    let work = std::env::temp_dir().join(format!("splice-speech-{}-{}", std::process::id(), from));
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
        .args(["-l", "auto", "-ml", "1", "-sow", "-oj", "-np", "-of"])
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
    parse(&json?)
}

/// whisper-cli's JSON, one segment a word (`-ml 1 -sow`), as words.
fn parse(json: &str) -> Result<Vec<Heard>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let segments = value["transcription"].as_array().cloned().unwrap_or_default();
    Ok(segments
        .iter()
        .filter_map(|s| {
            let text = s["text"].as_str()?.trim().trim_matches('"').to_string();
            // Whisper marks what it cannot make out in brackets: [MUSIC], (laughs).
            let noise = text.starts_with('[') || text.starts_with('(');
            let (a, z) = (s["offsets"]["from"].as_i64()?, s["offsets"]["to"].as_i64()?);
            (!text.is_empty() && !noise).then_some(Heard {
                text,
                start: a * 1000,
                end: z.max(a) * 1000,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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
