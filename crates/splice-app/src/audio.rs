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
    /// PCM frames per frame of playback: 1 normally; a clip whose retimed
    /// sound is not ready yet plays its own faster or slower (or, negative,
    /// backwards), pitch and all.
    pub step: f64,
    pub volume: f32,
    pub pcm: Arc<Pcm>,
    /// Rising and falling across transitions: timeline frames, start and end.
    pub fade_in: Option<(u64, u64)>,
    pub fade_out: Option<(u64, u64)>,
}

impl Voice {
    /// Quarter-sine fades, as export makes them (`afade` with `qsin`).
    fn gain(&self, t: u64) -> f32 {
        let ramp = |(a, z): (u64, u64), rising: bool| {
            let p = (t.saturating_sub(a) as f64 / (z.saturating_sub(a)).max(1) as f64).clamp(0.0, 1.0);
            let p = if rising { p } else { 1.0 - p };
            (p * std::f64::consts::FRAC_PI_2).sin() as f32
        };
        self.fade_in.map_or(1.0, |f| ramp(f, true)) * self.fade_out.map_or(1.0, |f| ramp(f, false))
    }
}

#[derive(Default)]
struct Mix {
    playing: bool,
    /// Timeline frame being played.
    position: u64,
    voices: Vec<Voice>,
    limiter: Limiter,
}

/// Holds the mix under full scale when clips overlap, instead of clipping:
/// the gain drops at once to meet a peak and recovers over a few tens of
/// milliseconds. Export does the same with ffmpeg's `alimiter`.
struct Limiter {
    gain: f32,
    /// How much of the way back to unity the gain recovers each frame.
    release: f32,
}

impl Default for Limiter {
    fn default() -> Limiter {
        Limiter::new(48_000)
    }
}

impl Limiter {
    const CEILING: f32 = 0.97;

    fn new(rate: u32) -> Limiter {
        Limiter {
            gain: 1.0,
            release: 1.0 / (rate as f32 * 0.08),
        }
    }

    fn apply(&mut self, left: f32, right: f32) -> (f32, f32) {
        let peak = left.abs().max(right.abs());
        let need = if peak > Self::CEILING {
            Self::CEILING / peak
        } else {
            1.0
        };
        if need < self.gain {
            self.gain = need;
        } else {
            self.gain += (1.0 - self.gain) * self.release;
        }
        (left * self.gain, right * self.gain)
    }
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
            Ok((stream, rate)) => {
                mix.lock().unwrap().limiter = Limiter::new(rate);
                AudioOut {
                    _stream: Some(stream),
                    mix,
                    rate,
                }
            }
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
        mix.limiter.gain = 1.0;
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
    let mix = &mut *mix;
    let start = mix.position;
    for (i, frame) in out.chunks_mut(channels).enumerate() {
        let t = start + i as u64;
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for voice in &mix.voices {
            if t < voice.start || t >= voice.start + voice.length {
                continue;
            }
            let along = (t - voice.start) as f64 * voice.step;
            let frame = voice.source_in as f64 + along;
            if frame < 0.0 {
                continue;
            }
            let index = frame as usize * 2;
            if index + 1 < voice.pcm.samples.len() {
                let gain = voice.volume * voice.gain(t);
                left += voice.pcm.samples[index] as f32 / 32768.0 * gain;
                right += voice.pcm.samples[index + 1] as f32 / 32768.0 * gain;
            }
        }
        let (left, right) = mix.limiter.apply(left, right);
        let (left, right) = (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0));
        for (c, sample) in frame.iter_mut().enumerate() {
            *sample = convert(if c % 2 == 0 { left } else { right });
        }
    }
    mix.position += (out.len() / channels) as u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limiter_holds_peaks_and_recovers() {
        let mut limiter = Limiter::new(48_000);
        assert_eq!(limiter.apply(0.5, -0.5), (0.5, -0.5), "quiet sound passes untouched");
        let (l, r) = limiter.apply(1.6, -0.8);
        assert!(l <= Limiter::CEILING + 1e-6 && r.abs() < 0.5);
        for _ in 0..48_000 {
            limiter.apply(0.1, 0.1);
        }
        assert!(limiter.gain > 0.99, "a second later the gain is back");
    }
}
