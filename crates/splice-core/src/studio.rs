//! The recording setup a project keeps: what is recorded (a screen or a
//! window, a camera, a microphone, the sound the Mac plays), how the
//! pictures sit together, and how the files are made. Every source is
//! written to its own file; the scene is how a take is laid on the timeline,
//! so it can still be changed after recording.

use crate::look::{Crop, Look, Mask, Shape};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Studio {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<Screen>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<Device>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microphone: Option<Device>,
    /// The sound the Mac plays (not Splice's own).
    pub system_audio: bool,
    pub scene: Scene,
    pub output: Output,
}

/// What of the screen is recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Screen {
    /// A display, counting from 1 (the main one).
    Display { number: u32 },
    /// A window, found again by its app and title (window numbers do not
    /// last from one launch to the next).
    Window { app: String, title: String },
    /// Part of a display, in points from its top left.
    Region {
        display: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
}

/// A camera or microphone: the system's id for it, and its name, to show
/// and to find it by when the id has changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// The screen alone.
    Screen,
    /// The screen, with the camera small in a corner.
    #[default]
    Bubble,
    /// The screen beside the camera.
    Side,
    /// The camera alone.
    Camera,
}

impl Layout {
    pub const ALL: [Layout; 4] = [Layout::Screen, Layout::Bubble, Layout::Side, Layout::Camera];

    pub fn name(self) -> &'static str {
        match self {
            Layout::Screen => "Screen",
            Layout::Bubble => "Bubble",
            Layout::Side => "Side by side",
            Layout::Camera => "Camera",
        }
    }

    pub fn parse(s: &str) -> Option<Layout> {
        match s.to_lowercase().as_str() {
            "screen" => Some(Layout::Screen),
            "bubble" | "corner" => Some(Layout::Bubble),
            "side" | "side-by-side" | "sidebyside" => Some(Layout::Side),
            "camera" | "cam" => Some(Layout::Camera),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    pub fn parse(s: &str) -> Option<Corner> {
        match s.to_lowercase().as_str() {
            "tl" | "top-left" => Some(Corner::TopLeft),
            "tr" | "top-right" => Some(Corner::TopRight),
            "bl" | "bottom-left" => Some(Corner::BottomLeft),
            "br" | "bottom-right" => Some(Corner::BottomRight),
            _ => None,
        }
    }

    fn right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }

    fn bottom(self) -> bool {
        matches!(self, Corner::BottomLeft | Corner::BottomRight)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bubble {
    #[default]
    Circle,
    Square,
}

/// How the pictures sit together.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub layout: Layout,
    pub corner: Corner,
    /// The bubble's height, as a fraction of the canvas's.
    pub size: f32,
    pub shape: Bubble,
    /// Side by side: the camera's share of the width.
    pub split: f32,
}

impl Default for Scene {
    fn default() -> Scene {
        Scene {
            layout: Layout::Bubble,
            corner: Corner::BottomRight,
            size: 0.3,
            shape: Bubble::Circle,
            split: 0.32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    /// Plays everywhere.
    #[default]
    H264,
    /// Smaller files, the same picture.
    Hevc,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Good,
    #[default]
    High,
    Best,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Good, Quality::High, Quality::Best];

    /// The encoder's constant quality, 0 to 1.
    pub fn value(self) -> f64 {
        match self {
            Quality::Good => 0.5,
            Quality::High => 0.65,
            Quality::Best => 0.8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Quality::Good => "Good",
            Quality::High => "High",
            Quality::Best => "Best",
        }
    }
}

/// How the files are made.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    pub codec: Codec,
    pub quality: Quality,
    pub fps: u32,
    /// The screen file is no taller than this; None keeps every pixel.
    pub height: Option<u32>,
    /// The pointer drawn in the screen file.
    pub cursor: bool,
    /// Clicks noted, to zoom in on later.
    pub clicks: bool,
}

impl Default for Output {
    fn default() -> Output {
        Output {
            codec: Codec::H264,
            quality: Quality::High,
            fps: 30,
            height: Some(2160),
            cursor: true,
            clicks: true,
        }
    }
}

/// Heights the screen file can be held to.
pub const HEIGHTS: [Option<u32>; 4] = [None, Some(2160), Some(1440), Some(1080)];

pub fn height_name(h: Option<u32>) -> String {
    match h {
        None => "Full".into(),
        Some(2160) => "4K".into(),
        Some(h) => format!("{h}p"),
    }
}

impl Studio {
    pub fn is_default(&self) -> bool {
        *self == Studio::default()
    }

    /// Whether anything is chosen to record.
    pub fn has_sources(&self) -> bool {
        self.screen.is_some() || self.camera.is_some() || self.microphone.is_some() || self.system_audio
    }

    /// What the take's pictures look like on a canvas of `canvas` pixels:
    /// the screen's and the camera's, given their sizes. None leaves that
    /// picture off the timeline (it is still recorded, and in the media).
    pub fn looks(&self, canvas: (u32, u32), screen: Option<(u32, u32)>, camera: Option<(u32, u32)>) -> Placed {
        let scene = self.scene;
        let (w, h) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);
        // Scale 1 is the picture fitted inside the canvas.
        let fit = |s: (u32, u32)| (w / s.0.max(1) as f32).min(h / s.1.max(1) as f32);
        let full = |s: (u32, u32)| Look {
            // Filling the canvas, any spare cut away.
            scale: (w / s.0.max(1) as f32).max(h / s.1.max(1) as f32) / fit(s),
            ..Look::default()
        };
        let layout = match (scene.layout, screen, camera) {
            // A layout with a picture missing falls back to the one there is.
            (Layout::Camera, Some(_), None) | (_, Some(_), None) => Layout::Screen,
            (_, None, Some(_)) => Layout::Camera,
            (layout, _, _) => layout,
        };
        match (layout, screen, camera) {
            (Layout::Screen, Some(_), _) => Placed {
                screen: Some(Look::default()),
                camera: None,
            },
            (Layout::Camera, _, Some(c)) => Placed {
                screen: None,
                camera: Some(full(c)),
            },
            (Layout::Bubble, Some(_), Some(c)) => {
                let (cw, ch) = (c.0.max(1) as f32, c.1.max(1) as f32);
                // The bubble shows most of the camera's shorter side.
                let side = cw.min(ch) * 0.94;
                let diameter = scene.size.clamp(0.08, 0.9) * h;
                let margin = 0.035 * h;
                let scale = diameter / (side * fit(c));
                let cx = if scene.corner.right() {
                    w - margin - diameter / 2.0
                } else {
                    margin + diameter / 2.0
                };
                let cy = if scene.corner.bottom() {
                    h - margin - diameter / 2.0
                } else {
                    margin + diameter / 2.0
                };
                let mask = Mask {
                    shape: match scene.shape {
                        Bubble::Circle => Shape::Ellipse,
                        Bubble::Square => Shape::Rect,
                    },
                    x: 0.5,
                    y: 0.5,
                    width: side / cw,
                    height: side / ch,
                    feather: 0.015,
                    invert: false,
                };
                Placed {
                    screen: Some(Look::default()),
                    camera: Some(Look {
                        x: (cx - w / 2.0) / w,
                        y: (cy - h / 2.0) / h,
                        scale,
                        mask: Some(mask),
                        ..Look::default()
                    }),
                }
            }
            (Layout::Side, Some(s), Some(c)) => {
                let share = scene.split.clamp(0.15, 0.6);
                let (left, right) = (w * (1.0 - share), w * share);
                // The screen fitted into its part, the camera filling its own.
                let (sw, sh) = (s.0.max(1) as f32, s.1.max(1) as f32);
                let screen_fit = (left / sw).min(h / sh);
                let (cw, ch) = (c.0.max(1) as f32, c.1.max(1) as f32);
                let cover = (right / cw).max(h / ch);
                let (shown_w, shown_h) = (cw * cover, ch * cover);
                let cut_x = ((shown_w - right) / shown_w / 2.0).max(0.0);
                let cut_y = ((shown_h - h) / shown_h / 2.0).max(0.0);
                Placed {
                    screen: Some(Look {
                        x: (left / 2.0 - w / 2.0) / w,
                        scale: screen_fit / fit(s),
                        ..Look::default()
                    }),
                    camera: Some(Look {
                        x: (w - right / 2.0 - w / 2.0) / w,
                        scale: cover / fit(c),
                        crop: Some(Crop {
                            left: cut_x,
                            right: cut_x,
                            top: cut_y,
                            bottom: cut_y,
                        }),
                        ..Look::default()
                    }),
                }
            }
            _ => Placed::default(),
        }
    }
}

/// Where a take's pictures go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placed {
    pub screen: Option<Look>,
    pub camera: Option<Look>,
}

/// One recording: its folder, and the file each source was written to,
/// each starting at the same moment and lasting as long.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Take {
    pub screen: Option<std::path::PathBuf>,
    pub camera: Option<std::path::PathBuf>,
    pub microphone: Option<std::path::PathBuf>,
    pub system: Option<std::path::PathBuf>,
    pub seconds: f64,
}

impl Take {
    /// The files, pictures first (the screen lowest).
    pub fn files(&self) -> Vec<(&'static str, &std::path::Path)> {
        [
            ("screen", &self.screen),
            ("camera", &self.camera),
            ("microphone", &self.microphone),
            ("system", &self.system),
        ]
        .into_iter()
        .filter_map(|(name, p)| p.as_deref().map(|p| (name, p)))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio(layout: Layout) -> Studio {
        Studio {
            scene: Scene {
                layout,
                ..Scene::default()
            },
            ..Studio::default()
        }
    }

    const CANVAS: (u32, u32) = (1920, 1080);
    const SCREEN: Option<(u32, u32)> = Some((3840, 2160));
    const CAMERA: Option<(u32, u32)> = Some((1920, 1080));

    #[test]
    fn the_bubble_is_round_and_sits_in_its_corner() {
        let placed = studio(Layout::Bubble).looks(CANVAS, SCREEN, CAMERA);
        assert_eq!(placed.screen, Some(Look::default()));
        let cam = placed.camera.unwrap();
        let mask = cam.mask.unwrap();
        // Round in pixels: as wide as tall on the camera's own picture.
        assert!((mask.width * 1920.0 - mask.height * 1080.0).abs() < 0.5);
        // Its height on the canvas is the size asked for.
        let shown = mask.height * 1080.0 * cam.scale;
        assert!((shown - 0.3 * 1080.0).abs() < 0.5, "{shown}");
        // Bottom right, clear of the edges.
        let (cx, cy) = (960.0 + cam.x * 1920.0, 540.0 + cam.y * 1080.0);
        assert!(cx + shown / 2.0 < 1920.0 && cx > 1440.0);
        assert!(cy + shown / 2.0 < 1080.0 && cy > 700.0);
        let left = Studio {
            scene: Scene {
                corner: Corner::TopLeft,
                ..Scene::default()
            },
            ..Studio::default()
        }
        .looks(CANVAS, SCREEN, CAMERA)
        .camera
        .unwrap();
        assert!(left.x < 0.0 && left.y < 0.0);
    }

    #[test]
    fn side_by_side_shares_the_width_without_overlap() {
        let placed = studio(Layout::Side).looks(CANVAS, SCREEN, CAMERA);
        let (screen, cam) = (placed.screen.unwrap(), placed.camera.unwrap());
        // The screen's right edge and the camera's left edge meet.
        let screen_w = 1920.0 * screen.scale;
        let screen_right = 960.0 + screen.x * 1920.0 + screen_w / 2.0;
        let crop = cam.crop.unwrap();
        let cam_w = 1920.0 * cam.scale * (1.0 - crop.left - crop.right);
        let cam_left = 960.0 + cam.x * 1920.0 - cam_w / 2.0;
        assert!((cam_w - 0.32 * 1920.0).abs() < 1.0, "{cam_w}");
        assert!(screen_right <= cam_left + 1.0, "{screen_right} {cam_left}");
        // The camera fills the height.
        assert!((1080.0 * cam.scale * (1.0 - crop.top - crop.bottom) - 1080.0).abs() < 1.0);
    }

    #[test]
    fn a_missing_picture_leaves_the_other_alone() {
        let placed = studio(Layout::Bubble).looks(CANVAS, SCREEN, None);
        assert_eq!(placed.screen, Some(Look::default()));
        assert!(placed.camera.is_none());
        let placed = studio(Layout::Side).looks(CANVAS, None, Some((1080, 1920)));
        assert!(placed.screen.is_none());
        // A tall camera fills a wide canvas.
        assert!((placed.camera.unwrap().scale - (1920.0 / 1080.0) / (1080.0 / 1920.0)).abs() < 1e-3);
        let placed = studio(Layout::Screen).looks(CANVAS, SCREEN, CAMERA);
        assert!(placed.camera.is_none(), "recorded, in the media, not laid out");
    }

    #[test]
    fn a_studio_reads_back_what_it_wrote() {
        let s = Studio {
            screen: Some(Screen::Window {
                app: "Safari".into(),
                title: "Docs".into(),
            }),
            camera: Some(Device {
                id: "0x1".into(),
                name: "C920".into(),
            }),
            system_audio: true,
            ..Studio::default()
        };
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Studio>(&text).unwrap(), s);
        assert_eq!(serde_json::from_str::<Studio>("{}").unwrap(), Studio::default());
    }
}
