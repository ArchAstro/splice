//! Zooming in on a screen recording where it was clicked: each burst of
//! clicks gets a smooth push in towards them, a pan from click to click, and
//! a pull back out once the clicking stops. It is written as keyframes on
//! the clip's place and size, so it can be adjusted like any others.

use crate::look::{Ease, Prop};
use crate::model::{ClipId, Project};
use crate::time::Time;

/// Clicks closer than this belong to one burst, and the zoom stays in.
const BURST: Time = 2_500_000;
/// How long the zoom takes to come in, ahead of the first click.
const IN: Time = 600_000;
/// How long it stays in after the last click, then takes to go out.
const STAY: Time = 1_200_000;
const OUT: Time = 700_000;

impl Project {
    /// Zooms clip `id` in by `zoom` towards each burst of clicks recorded in
    /// its media, replacing its place and size keyframes. Returns how many
    /// zooms it made (0: no clicks in the stretch it plays).
    pub fn zoom_to_clicks(&mut self, id: ClipId, zoom: f32) -> usize {
        let Some(clip) = self.clip(id).cloned() else { return 0 };
        let Some(media) = self.media(clip.media).cloned() else {
            return 0;
        };
        if clip.reverse || clip.frozen || clip.generator.is_some() || media.clicks.is_empty() {
            return 0;
        }
        let zoom = zoom.clamp(1.05, 4.0);
        // Where each click lands in the clip, and where on its frame.
        let mut clicks: Vec<(Time, f32, f32)> = media
            .clicks
            .iter()
            .filter(|c| c.at >= clip.source_in && c.at < clip.source_out())
            .map(|c| (((c.at - clip.source_in) as f64 / clip.speed) as Time, c.x, c.y))
            .collect();
        clicks.sort_by_key(|c| c.0);
        let mut bursts: Vec<Vec<(Time, f32, f32)>> = Vec::new();
        for click in clicks {
            match bursts.last_mut() {
                Some(burst) if click.0 - burst.last().map_or(0, |c| c.0) <= BURST => burst.push(click),
                _ => bursts.push(vec![click]),
            }
        }
        if bursts.is_empty() {
            return 0;
        }
        // Centring a point of the frame while zoomed, kept so the frame still
        // covers the canvas.
        let (cw, ch) = (self.settings.width as f64, self.settings.height as f64);
        let fit = (cw / media.width.max(1) as f64).min(ch / media.height.max(1) as f64);
        let (fw, fh) = (media.width as f64 * fit / cw, media.height as f64 * fit / ch);
        let centre = |u: f32, along: f64| {
            let room = ((zoom as f64 * along - 1.0) / 2.0).max(0.0);
            ((0.5 - u as f64) * zoom as f64 * along).clamp(-room, room) as f32
        };
        let length = clip.duration;
        let count = bursts.len();
        self.set_look(id, |look| {
            for prop in [Prop::X, Prop::Y, Prop::Scale] {
                look.keys.of_mut(prop).clear();
            }
            let mut key = |at: Time, scale: f32, x: f32, y: f32| {
                let at = at.clamp(0, length);
                look.keys.set(Prop::Scale, at, scale, Some(Ease::InOut));
                look.keys.set(Prop::X, at, x, Some(Ease::InOut));
                look.keys.set(Prop::Y, at, y, Some(Ease::InOut));
            };
            key(0, 1.0, 0.0, 0.0);
            for burst in &bursts {
                let first = burst[0];
                let last = *burst.last().expect("a burst has a click");
                key(first.0 - IN, 1.0, 0.0, 0.0);
                for &(at, u, v) in burst {
                    key(at, zoom, centre(u, fw), centre(v, fh));
                }
                let stay = last.0 + STAY;
                key(stay, zoom, centre(last.1, fw), centre(last.2, fh));
                key(stay + OUT, 1.0, 0.0, 0.0);
            }
        });
        count
    }

    /// Takes away a clip's zoom: its place and size keyframes.
    pub fn clear_zoom(&mut self, id: ClipId) {
        self.set_look(id, |look| {
            for prop in [Prop::X, Prop::Y, Prop::Scale] {
                look.keys.of_mut(prop).clear();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{Click, Media, MediaId, MediaKind, Project};
    use crate::time::SECOND;
    use std::path::PathBuf;

    #[test]
    fn zooms_in_on_each_burst_of_clicks_and_back_out() {
        let mut p = Project::default();
        let m = p.add_media(Media {
            id: MediaId(0),
            name: "rec".into(),
            hash: "rec".into(),
            ext: "mov".into(),
            kind: MediaKind::Video,
            duration: 20 * SECOND,
            width: 1920,
            height: 1080,
            has_audio: false,
            source: PathBuf::new(),
            recipe: None,
            content: None,
            clicks: vec![
                Click {
                    at: 3 * SECOND,
                    x: 0.9,
                    y: 0.1,
                },
                Click {
                    at: 4 * SECOND,
                    x: 0.8,
                    y: 0.2,
                },
                Click {
                    at: 12 * SECOND,
                    x: 0.5,
                    y: 0.5,
                },
            ],
        });
        let clip = p.append_clip(m).unwrap();
        assert_eq!(p.zoom_to_clicks(clip, 2.0), 2, "two bursts");
        let look = &p.clip(clip).unwrap().look;
        let at = |t: f64| crate::time::from_secs(t);
        assert!(
            (look.value(crate::look::Prop::Scale, at(1.0)) - 1.0).abs() < 1e-6,
            "not yet"
        );
        assert!(
            (look.value(crate::look::Prop::Scale, at(3.0)) - 2.0).abs() < 1e-6,
            "in at the click"
        );
        // Towards the top right, but never past the frame's edge.
        let x = look.value(crate::look::Prop::X, at(3.0));
        assert!((x + 0.5).abs() < 1e-6, "as far as the edge allows: {x}");
        assert!(
            (look.value(crate::look::Prop::Scale, at(8.0)) - 1.0).abs() < 1e-6,
            "out between bursts"
        );
        p.clear_zoom(clip);
        assert!(p.clip(clip).unwrap().look.keys.is_empty());
    }
}
