//! Cleaning up a recorded voice: rumble cut, steady background noise (fans,
//! hiss, hum) pushed down, and quiet and loud passages brought closer
//! together. Written once as ffmpeg filters, which export and the viewer's
//! decoder both run, so what you hear is what you get.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Clean {
    /// How hard steady noise is pushed down, 0 (not at all) to 1.
    pub noise: f32,
    /// The level evened out: quiet words lifted, loud ones held back.
    pub even: bool,
}

impl Default for Clean {
    fn default() -> Clean {
        Clean { noise: 0.5, even: true }
    }
}

impl Clean {
    /// The filters, in order, for a chain of sound at any rate.
    pub fn filters(&self) -> Vec<String> {
        // Below 80 Hz is rumble, desk knocks and breath on the mic, not voice.
        let mut filters = vec!["highpass=f=80".to_string()];
        let noise = if self.noise.is_finite() {
            self.noise.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if noise > 0.0 {
            // Lowered by up to 36 dB, the noise followed as it changes. A
            // stronger setting also counts more as noise (a higher floor): at
            // 0.5, a webcam's room noise drops about 5 dB and the voice hardly.
            filters.push(format!(
                "afftdn=nr={:.1}:nf={:.1}:tn=1",
                12.0 + 24.0 * noise,
                -45.0 + 25.0 * noise
            ));
        }
        if self.even {
            // Each quarter second brought towards peaks of −6 dB, by at most
            // 6×, the gain smoothed over a few seconds: quiet words come up,
            // loud ones go down, and a pause keeps the gain of the words
            // around it rather than having its noise raised to meet them.
            filters.push("dynaudnorm=f=250:g=15:m=6:p=0.5".into());
        }
        filters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_cleaning_takes_more_and_evening_is_its_own_choice() {
        let gentle = Clean {
            noise: 0.2,
            even: false,
        }
        .filters();
        assert_eq!(gentle.len(), 2);
        assert!(gentle[0].starts_with("highpass") && gentle[1].starts_with("afftdn=nr=16.8:nf=-40.0"));
        let hard = Clean { noise: 1.0, even: true }.filters();
        // ffmpeg takes a floor no higher than −20 dB.
        assert!(hard[1].starts_with("afftdn=nr=36.0:nf=-20.0"));
        assert!(hard[2].starts_with("dynaudnorm"));
        // No noise reduction at 0, and nonsense is no reduction either.
        for noise in [0.0, f32::NAN] {
            let f = Clean { noise, even: false }.filters();
            assert_eq!(f, vec!["highpass=f=80".to_string()]);
        }
    }
}
