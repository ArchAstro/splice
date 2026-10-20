//! A small line-icon set, drawn for Splice on a 24-unit grid and served to
//! gpui as embedded SVG assets (`icons/<name>.svg`). gpui paints an SVG as a
//! mask, so every icon takes the text color it is given.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

const STROKE: &str = r#"fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round""#;

fn icon(name: &str) -> Option<&'static str> {
    Some(match name {
        "play" => {
            r#"<path d="M7.5 4.8v14.4a.8.8 0 0 0 1.2.7l11.6-7.2a.8.8 0 0 0 0-1.4L8.7 4.1a.8.8 0 0 0-1.2.7z" fill="black" stroke="none"/>"#
        }
        "pause" => {
            r#"<rect x="6" y="4.5" width="4.2" height="15" rx="1.2" fill="black" stroke="none"/><rect x="13.8" y="4.5" width="4.2" height="15" rx="1.2" fill="black" stroke="none"/>"#
        }
        "start" => {
            r#"<path d="M6 5v14"/><path d="M18 5.8v12.4a.6.6 0 0 1-.95.5L9.2 12.5a.6.6 0 0 1 0-1l7.85-6.2a.6.6 0 0 1 .95.5z" fill="black"/>"#
        }
        "end" => {
            r#"<path d="M18 5v14"/><path d="M6 5.8v12.4a.6.6 0 0 0 .95.5l7.85-6.2a.6.6 0 0 0 0-1L6.95 5.3a.6.6 0 0 0-.95.5z" fill="black"/>"#
        }
        "undo" => r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>"#,
        "redo" => r#"<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>"#,
        "split" => {
            r#"<circle cx="6" cy="6" r="2.6"/><circle cx="6" cy="18" r="2.6"/><path d="M20 4 8.2 15.8"/><path d="M14.5 14.5 20 20"/><path d="M8.2 8.2 11.5 11.5"/>"#
        }
        "trash" => {
            r#"<path d="M4 6.5h16"/><path d="M9.5 6.5V4.5h5v2"/><path d="M6.2 6.5 7 19a2 2 0 0 0 2 1.8h6a2 2 0 0 0 2-1.8l.8-12.5"/><path d="M10 10.5v6M14 10.5v6"/>"#
        }
        "ripple" => {
            r#"<path d="M3.5 5v14M20.5 5v14"/><path d="M7 12h3.5M13.5 12H17"/><path d="m7.5 9 3 3-3 3"/><path d="m16.5 9-3 3 3 3"/>"#
        }
        "copy" => {
            r#"<rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2.2"/><path d="M15.5 8.5V6.2A2.2 2.2 0 0 0 13.3 4H6.2A2.2 2.2 0 0 0 4 6.2v7.1a2.2 2.2 0 0 0 2.2 2.2h2.3"/>"#
        }
        "zoom-in" => {
            r#"<circle cx="10.5" cy="10.5" r="6.5"/><path d="m20 20-4.6-4.6"/><path d="M7.8 10.5h5.4M10.5 7.8v5.4"/>"#
        }
        "zoom-out" => r#"<circle cx="10.5" cy="10.5" r="6.5"/><path d="m20 20-4.6-4.6"/><path d="M7.8 10.5h5.4"/>"#,
        "fit" => r#"<path d="M3.5 12h17"/><path d="m7 8.5-3.5 3.5L7 15.5"/><path d="m17 8.5 3.5 3.5-3.5 3.5"/>"#,
        "eye" => {
            r#"<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.8"/>"#
        }
        "eye-off" => {
            r#"<path d="M6.5 6.9C3.9 8.6 2.5 12 2.5 12S6 18.5 12 18.5c1.9 0 3.5-.6 4.9-1.5"/><path d="M10 5.7c.6-.1 1.3-.2 2-.2 6 0 9.5 6.5 9.5 6.5s-.7 1.3-2 2.8"/><path d="M3.5 3.5l17 17"/>"#
        }
        "volume" => {
            r#"<path d="M11 5.5 6.5 9H3.5v6h3l4.5 3.5z"/><path d="M15.3 9a4.2 4.2 0 0 1 0 6"/><path d="M18.2 6.2a8.2 8.2 0 0 1 0 11.6"/>"#
        }
        "volume-off" => r#"<path d="M11 5.5 6.5 9H3.5v6h3l4.5 3.5z"/><path d="m15.5 9.5 5 5M20.5 9.5l-5 5"/>"#,
        "plus" => r#"<path d="M12 5v14M5 12h14"/>"#,
        "export" => {
            r#"<path d="M12 14.5V3.5"/><path d="m7.5 8 4.5-4.5L16.5 8"/><path d="M5 14v4.5A2 2 0 0 0 7 20.5h10a2 2 0 0 0 2-2V14"/>"#
        }
        "history" => {
            r#"<path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1L3.5 8.5"/><path d="M3.5 3.5v5h5"/><path d="M12 7.5V12l3 2"/>"#
        }
        "command" => {
            r#"<path d="M15 6.5v11a2.5 2.5 0 1 0 2.5-2.5h-11A2.5 2.5 0 1 0 9 17.5v-11A2.5 2.5 0 1 0 6.5 9h11A2.5 2.5 0 1 0 15 6.5"/>"#
        }
        "film" => {
            r#"<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M7.5 4v16M16.5 4v16M3 9h4.5M3 15h4.5M16.5 9H21M16.5 15H21"/>"#
        }
        "music" => {
            r#"<path d="M9 17.5V5.5l10.5-2v12"/><circle cx="6.5" cy="17.5" r="2.5"/><circle cx="17" cy="15.5" r="2.5"/>"#
        }
        "image" => {
            r#"<rect x="3.5" y="3.5" width="17" height="17" rx="2.5"/><circle cx="9" cy="9" r="1.8"/><path d="m20.5 15-4.8-4.8L5.5 20.5"/>"#
        }
        "sidebar" => {
            r#"<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M9.5 4.5v15"/><path d="M5.8 8h1.6M5.8 11h1.6" />"#
        }
        "sidebar-off" => r#"<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M9.5 4.5v15"/>"#,
        "message" => {
            r#"<path d="M20.5 11.5a8 8 0 0 1-11.8 7L3.5 20l1.6-4.6A8 8 0 1 1 20.5 11.5z"/><path d="M8.5 11.5h.01M12 11.5h.01M15.5 11.5h.01" stroke-width="2.4"/>"#
        }
        "align-left" => r#"<path d="M4 6h16M4 10h10M4 14h16M4 18h10"/>"#,
        "align-center" => r#"<path d="M4 6h16M7 10h10M4 14h16M7 18h10"/>"#,
        "align-right" => r#"<path d="M4 6h16M10 10h10M4 14h16M10 18h10"/>"#,
        "score" => {
            r#"<path d="M3.5 7h17M3.5 11h17M3.5 15h17"/><path d="M14.5 15.5V5l4-1"/><ellipse cx="12.4" cy="16" rx="2.4" ry="1.9" fill="black"/><path d="M6.5 19h4"/>"#
        }
        "scene" => {
            r#"<circle cx="7.5" cy="8" r="3.5"/><rect x="13" y="4.5" width="7" height="7" rx="1.5"/><path d="M8 13.5 12 20H4z"/><path d="M14 17.5h6"/>"#
        }
        "close" => r#"<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>"#,
        "minus" => r#"<path d="M5 12h14"/>"#,
        "chevron" => r#"<path d="m7 9.5 5 5 5-5"/>"#,
        "chevron-right" => r#"<path d="m9.5 7 5 5-5 5"/>"#,
        "text" => r#"<path d="M5 6.5V5h14v1.5"/><path d="M12 5v14"/><path d="M9 19h6"/>"#,
        "swatch" => {
            r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 3.5v17" /><path d="M12 3.5a8.5 8.5 0 0 1 0 17z" fill="black"/>"#
        }
        "layers" => {
            r#"<path d="m12 3.5 9 4.5-9 4.5-9-4.5z"/><path d="m3 12 9 4.5 9-4.5"/><path d="m3 16 9 4.5 9-4.5"/>"#
        }
        _ => return None,
    })
}

pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")) else {
            return Ok(None);
        };
        Ok(icon(name).map(|body| {
            let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" {STROKE}>{body}</svg>"#);
            Cow::Owned(svg.into_bytes())
        }))
    }

    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

pub fn path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_load() {
        for name in ["play", "pause", "split", "eye-off", "volume-off", "export"] {
            let bytes = Icons.load(&format!("icons/{name}.svg")).unwrap().unwrap();
            assert!(std::str::from_utf8(&bytes).unwrap().starts_with("<svg"));
        }
        assert!(Icons.load("icons/nope.svg").unwrap().is_none());
    }
}
