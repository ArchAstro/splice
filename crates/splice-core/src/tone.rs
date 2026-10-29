//! A look's colour and key worked out directly on BGRA pixels, for live
//! pictures that can't wait for ffmpeg: the Record tab's preview. The sums
//! follow the filters export runs (`Grade::ffmpeg`, `chromakey`, `despill`)
//! closely enough to judge by; a grade's preset is left to export.

use crate::look::{Grade, Key};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tone {
    pub grade: Grade,
    pub key: Option<Key>,
}

impl Tone {
    /// Nothing to do.
    pub fn is_none(&self) -> bool {
        self.key.is_none() && !self.graded()
    }

    /// Whether the colour changes here (a preset alone is left to export).
    fn graded(&self) -> bool {
        !Grade {
            preset: Default::default(),
            ..self.grade.clamped()
        }
        .is_none()
    }

    /// Changes BGRA pixels in place: the key first (as export does, so a
    /// grade doesn't change what counts as screen), then the colour.
    pub fn apply(&self, bgra: &mut [u8]) {
        if self.is_none() {
            return;
        }
        let g = self.grade.clamped();
        let graded = self.graded();
        let k = 2f32.powf(g.exposure);
        let gain = [
            (k * (1.0 - 0.14 * g.temperature)).clamp(0.0, 2.0),
            (k * (1.0 - 0.1 * g.tint)).clamp(0.0, 2.0),
            (k * (1.0 + 0.14 * g.temperature)).clamp(0.0, 2.0),
        ];
        let contrast = if g.contrast >= 0.0 {
            1.0 + 0.8 * g.contrast
        } else {
            1.0 + 0.6 * g.contrast
        };
        let saturation = (1.0 + g.saturation).clamp(0.0, 3.0);
        let key = self.key.map(|key| {
            let (r, gr, b) = (
                ((key.color >> 16) & 0xff) as i32,
                ((key.color >> 8) & 0xff) as i32,
                (key.color & 0xff) as i32,
            );
            let spill = if gr >= b && gr > r {
                Some(1)
            } else if b > gr && b > r {
                Some(0)
            } else {
                None
            };
            (
                chroma(r, gr, b),
                key.similarity.clamp(0.01, 0.6),
                key.blend.clamp(0.0, 0.5),
                spill.filter(|_| key.despill),
            )
        });
        for px in bgra.as_chunks_mut::<4>().0 {
            if let Some(((ku, kv), similarity, blend, spill)) = key {
                let (u, v) = chroma(px[2] as i32, px[1] as i32, px[0] as i32);
                let (du, dv) = ((u - ku) as f32, (v - kv) as f32);
                let diff = ((du * du + dv * dv) / (255.0 * 255.0 * 2.0)).sqrt();
                let alpha = if blend > 0.0001 {
                    ((diff - similarity) / blend).clamp(0.0, 1.0)
                } else if diff > similarity {
                    1.0
                } else {
                    0.0
                };
                px[3] = (px[3] as f32 * alpha).round() as u8;
                // What's left loses the screen's cast: its channel no more
                // than the other two's mean.
                if let Some(c) = spill {
                    let (a, b) = if c == 1 { (0, 2) } else { (1, 2) };
                    let mean = (px[a] as u16 + px[b] as u16) / 2;
                    px[c] = px[c].min(mean as u8);
                }
            }
            if !graded {
                continue;
            }
            let mut c = [0f32; 3];
            for i in 0..3 {
                c[i] = px[i] as f32 / 255.0 * gain[i];
            }
            let y = 0.114 * c[0] + 0.587 * c[1] + 0.299 * c[2];
            let shown = (y - 0.5) * contrast + 0.5;
            for i in 0..3 {
                px[i] = ((shown + (c[i] - y) * saturation).clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
}

/// U and V as ffmpeg's chromakey measures them (BT.601, limited range).
fn chroma(r: i32, g: i32, b: i32) -> (i32, i32) {
    (
        ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128,
        ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_goes_clear_and_a_face_stays() {
        // BGRA: pure green, then a skin tone.
        let mut px = [0, 255, 0, 255, 120, 150, 210, 255];
        Tone {
            key: Some(Key::default()),
            ..Tone::default()
        }
        .apply(&mut px);
        assert_eq!(px[3], 0);
        assert_eq!(px[7], 255);
    }

    #[test]
    fn brighter_is_brighter_and_grey_is_grey() {
        let mut px = [100, 100, 100, 255, 40, 80, 200, 255];
        Tone {
            grade: Grade {
                exposure: 0.5,
                saturation: -1.0,
                ..Grade::default()
            },
            key: None,
        }
        .apply(&mut px);
        assert!(px[0] > 130, "{px:?}");
        // No saturation: every channel the same.
        assert!(px[4] == px[5] && px[5] == px[6], "{px:?}");
        // Untouched does nothing at all.
        let mut same = [1, 2, 3, 4];
        Tone::default().apply(&mut same);
        assert_eq!(same, [1, 2, 3, 4]);
    }
}
