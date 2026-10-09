//! Colors, after Stet Ink: an ink ground, paper-warm text, and one vermilion
//! accent reserved for where you are (the playhead and the selection).

use gpui::{Hsla, Rgba, rgb, rgba};

pub struct Theme;

#[allow(dead_code)]
impl Theme {
    pub fn ground() -> Rgba {
        rgb(0x141824)
    }
    pub fn mantle() -> Rgba {
        rgb(0x0d1018)
    }
    pub fn crust() -> Rgba {
        rgb(0x090b11)
    }
    /// Panels: the raised surfaces that sit on the crust.
    pub fn panel() -> Rgba {
        rgb(0x1a1f2d)
    }
    /// A panel's edge, a shade lighter than the panel so it reads on the crust.
    pub fn edge() -> Rgba {
        rgb(0x2c3244)
    }
    /// Track lanes inside the timeline panel.
    pub fn lane() -> Rgba {
        rgb(0x242a3a)
    }
    pub fn surface0() -> Rgba {
        rgb(0x2a2f40)
    }
    pub fn surface1() -> Rgba {
        rgb(0x3a4054)
    }
    pub fn surface2() -> Rgba {
        rgb(0x5c6173)
    }
    pub fn rule() -> Rgba {
        rgb(0x2a2f3f)
    }
    pub fn overlay() -> Rgba {
        rgb(0x9c978e)
    }
    pub fn muted() -> Rgba {
        rgb(0xb0aba2)
    }
    pub fn subtext() -> Rgba {
        rgb(0xd4cec3)
    }
    pub fn text() -> Rgba {
        rgb(0xf3eee4)
    }
    pub fn accent() -> Rgba {
        rgb(0xef6a55)
    }
    pub fn accent_soft() -> Rgba {
        rgba(0xef6a5530)
    }
    /// Clip bodies: picture clips and sound-only clips.
    pub fn clip() -> Rgba {
        rgb(0x363c52)
    }
    pub fn clip_audio() -> Rgba {
        rgb(0x1f3a36)
    }
    pub fn wave() -> Rgba {
        rgba(0x86cfc0b0)
    }
    pub fn error() -> Rgba {
        rgb(0xef6f5e)
    }
    pub fn scrim() -> Hsla {
        Hsla::from(rgba(0x090b11cc))
    }
}

pub const UI_FONT: &str = ".SystemUIFont";
pub const MONO_FONT: &str = "Menlo";
