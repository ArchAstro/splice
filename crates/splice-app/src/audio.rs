//! Sound out: a mixer that plays the timeline's audible clips from decoded
//! PCM. The device callback reads a shared voice list; the editor swaps that
//! list whenever playback starts.

use crate::ffmpeg::Pcm;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use splice_core::time::{SECOND, Time};
use std::sync::{Arc, Mutex};

pub struct Voice {
    /// Timeline frame (at the device rate) where the clip begins.
    pub start: u64,
    pub length: u64,
    /// Frame in the PCM the clip begins at.
    pub source_in: u64,
    pub volume: f32,
    pub pcm: Arc<Pcm>,
}

#[derive(Default)]
struct Mix {
    playing: bool,
    /// Timeline frame being played.
    position: u64,
    voices: Vec<Voice>,
}

pub struct AudioOut {
    _stream: Option<cpal::Stream>,
    mix: Arc<Mutex<Mix>>,
    pub rate: u32,
}

impl AudioOut {
    /// Opens the default output. Without a device the editor stays silent but works.
    pub fn open() -> AudioOut {
        let mix = Arc::new(Mutex::new(Mix::default()));
        match build(mix.clone()) {
            Ok((stream, rate)) => AudioOut {
                _stream: Some(stream),
                mix,
                rate,
            },
            Err(error) => {
                eprintln!("splice: no audio output ({error})");
                AudioOut {
                    _stream: None,
                    mix,
                    rate: 48_000,
                }
            }
        }
    }

    pub fn frames(&self, t: Time) -> u64 {
        (t.max(0) as i128 * self.rate as i128 / SECOND as i128) as u64
    }

    pub fn play(&self, at: Time, voices: Vec<Voice>) {
        let mut mix = self.mix.lock().unwrap();
        mix.voices = voices;
        mix.position = self.frames(at);
        mix.playing = true;
    }

    pub fn stop(&self) {
        let mut mix = self.mix.lock().unwrap();
        mix.playing = false;
        mix.voices.clear();
    }
}

fn build(mix: Arc<Mutex<Mix>>) -> anyhow::Result<(cpal::Stream, u32)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no output device"))?;
    let supported = device.default_output_config()?;
    let rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let config: cpal::StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |out: &mut [f32], _| fill(&mix, out, channels, |v| v),
            |e| eprintln!("splice: audio error {e}"),
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            move |out: &mut [i16], _| fill(&mix, out, channels, |v| (v * 32767.0) as i16),
            |e| eprintln!("splice: audio error {e}"),
            None,
        )?,
        other => anyhow::bail!("unsupported sample format {other:?}"),
    };
    stream.play()?;
    Ok((stream, rate))
}

fn fill<S: Copy + Default>(mix: &Mutex<Mix>, out: &mut [S], channels: usize, convert: impl Fn(f32) -> S) {
    let Ok(mut mix) = mix.try_lock() else {
        out.fill(S::default());
        return;
    };
    if !mix.playing {
        out.fill(S::default());
        return;
    }
    let start = mix.position;
    for (i, frame) in out.chunks_mut(channels).enumerate() {
        let t = start + i as u64;
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for voice in &mix.voices {
            if t < voice.start || t >= voice.start + voice.length {
                continue;
            }
            let index = (voice.source_in + (t - voice.start)) as usize * 2;
            if index + 1 < voice.pcm.samples.len() {
                left += voice.pcm.samples[index] as f32 / 32768.0 * voice.volume;
                right += voice.pcm.samples[index + 1] as f32 / 32768.0 * voice.volume;
            }
        }
        let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
        for (c, sample) in frame.iter_mut().enumerate() {
            *sample = convert(if c % 2 == 0 { left } else { right });
        }
    }
    mix.position += (out.len() / channels) as u64;
}
