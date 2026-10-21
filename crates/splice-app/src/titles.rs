//! Drawing titles and colours. Text is shaped and rasterized here with the
//! system's own fonts (cosmic-text: font discovery, shaping, fallback), so
//! the picture the viewer shows is the very picture export overlays.

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight};
use splice_core::export::{Rendered, Resolve};
use splice_core::look::Mask;
use splice_core::model::{Align, Clip, Generator, Media, Title};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();

fn fonts() -> &'static Mutex<Fonts> {
    FONTS.get_or_init(|| {
        Mutex::new(Fonts {
            system: FontSystem::new(),
            cache: SwashCache::new(),
        })
    })
}

/// Loads the system's fonts in the background, so the first title is quick.
pub fn warm() {
    std::thread::spawn(|| {
        let _ = fonts();
    });
}

/// Font families installed on this machine, sorted.
pub fn families() -> Vec<String> {
    let fonts = fonts().lock().unwrap();
    let mut names: Vec<String> = fonts
        .system
        .db()
        .faces()
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// Straight-alpha RGBA pixels.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    fn new(width: u32, height: u32) -> Picture {
        Picture {
            width,
            height,
            rgba: vec![0; (width * height * 4) as usize],
        }
    }

    /// Paints `color` over the pixel at (x, y), `coverage` 0 to 1.
    fn blend(&mut self, x: i32, y: i32, color: [u8; 4], coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        let src_a = color[3] as f32 / 255.0 * coverage;
        if src_a <= 0.0 {
            return;
        }
        let dst_a = self.rgba[i + 3] as f32 / 255.0;
        let out_a = src_a + dst_a * (1.0 - src_a);
        for (c, &s) in color.iter().take(3).enumerate() {
            let d = self.rgba[i + c] as f32;
            self.rgba[i + c] = ((s as f32 * src_a + d * dst_a * (1.0 - src_a)) / out_a.max(1e-6)).round() as u8;
        }
        self.rgba[i + 3] = (out_a * 255.0).round() as u8;
    }

    fn apply_mask(&mut self, mask: &Mask) {
        let (w, h) = (self.width as f32, self.height as f32);
        for y in 0..self.height {
            for x in 0..self.width {
                let a = mask.alpha((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
                let i = ((y * self.width + x) * 4 + 3) as usize;
                self.rgba[i] = (self.rgba[i] as f32 * a).round() as u8;
            }
        }
    }

    /// BGRA, as gpui wants it.
    pub fn bgra(&self) -> Vec<u8> {
        let mut out = self.rgba.clone();
        for px in out.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        out
    }

    pub fn save_png(&self, path: &Path) -> anyhow::Result<()> {
        let image = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .ok_or_else(|| anyhow::anyhow!("bad picture"))?;
        let partial = path.with_extension("partial.png");
        image.save(&partial)?;
        std::fs::rename(&partial, path)?;
        Ok(())
    }
}

fn rgba(color: u32) -> [u8; 4] {
    color.to_be_bytes()
}

/// Draws a title or colour for a `canvas`-sized frame, mask applied.
pub fn draw(generator: &Generator, mask: Option<&Mask>, canvas: (u32, u32)) -> Picture {
    let mut picture = match generator {
        Generator::Color { color } => {
            let mut p = Picture::new(canvas.0.max(2), canvas.1.max(2));
            let c = rgba(*color);
            for px in p.rgba.as_chunks_mut::<4>().0 {
                px.copy_from_slice(&c);
            }
            p
        }
        Generator::Text(title) => draw_title(title, canvas),
    };
    if let Some(mask) = mask {
        picture.apply_mask(mask);
    }
    picture
}

fn draw_title(title: &Title, canvas: (u32, u32)) -> Picture {
    let size = (title.size * canvas.1 as f32).clamp(4.0, 2000.0);
    let line = size * 1.18;
    let text = if title.text.trim().is_empty() {
        " "
    } else {
        title.text.as_str()
    };
    let mut guard = fonts().lock().unwrap();
    let Fonts { system, cache } = &mut *guard;

    let attrs = Attrs::new()
        .family(Family::Name(&title.font))
        .weight(if title.bold { Weight::BOLD } else { Weight::NORMAL })
        .style(if title.italic { Style::Italic } else { Style::Normal });
    let align = match title.align {
        Align::Left => cosmic_text::Align::Left,
        Align::Center => cosmic_text::Align::Center,
        Align::Right => cosmic_text::Align::Right,
    };
    let mut buffer = Buffer::new(system, Metrics::new(size, line));
    buffer.set_size(None, None);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(system, false);
    let widest = buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0f32, f32::max)
        .max(1.0);
    // Lay out again at the widest line's width, so alignment has room to act.
    buffer.set_size(Some(widest.ceil()), None);
    buffer.set_text(text, &attrs, Shaping::Advanced, Some(align));
    buffer.shape_until_scroll(system, false);
    let height = buffer
        .layout_runs()
        .map(|run| run.line_top + run.line_height)
        .fold(0.0f32, f32::max)
        .max(line);

    let pad = if title.background.is_some() {
        size * 0.42
    } else {
        size * 0.16
    };
    let shadow = if title.shadow { size * 0.06 } else { 0.0 };
    let width = (widest + pad * 2.0 + shadow * 2.0).ceil() as u32;
    let full_height = (height + pad * 2.0 + shadow * 2.0).ceil() as u32;
    let mut picture = Picture::new(width.max(2), full_height.max(2));
    let origin = (pad + shadow, pad + shadow);

    if let Some(background) = title.background {
        let (x0, y0) = (shadow, shadow);
        let (x1, y1) = (width as f32 - shadow, full_height as f32 - shadow);
        let radius = size * 0.22;
        let c = rgba(background);
        for y in 0..picture.height {
            for x in 0..picture.width {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                // Distance outside a rounded rectangle, for a smooth edge.
                let dx = (x0 + radius - px).max(px - (x1 - radius)).max(0.0);
                let dy = (y0 + radius - py).max(py - (y1 - radius)).max(0.0);
                let outside = (dx * dx + dy * dy).sqrt() - radius;
                let coverage = (0.5 - outside).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    picture.blend(x as i32, y as i32, c, coverage);
                }
            }
        }
    }

    if title.shadow {
        // The letters in soft black, a little down and right, blurred.
        let (w, h) = (picture.width as usize, picture.height as usize);
        let mut alpha = vec![0f32; w * h];
        let offset = (size * 0.03, size * 0.045);
        buffer.draw(system, cache, Color::rgba(0, 0, 0, 255), |x, y, gw, gh, color| {
            let a = color.a() as f32 / 255.0;
            for yy in 0..gh as i32 {
                for xx in 0..gw as i32 {
                    let px = (x + xx) as f32 + origin.0 + offset.0;
                    let py = (y + yy) as f32 + origin.1 + offset.1;
                    let (px, py) = (px as i32, py as i32);
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let v = &mut alpha[py as usize * w + px as usize];
                        *v = (*v + a * (1.0 - *v)).min(1.0);
                    }
                }
            }
        });
        let radius = (size * 0.035).ceil().max(1.0) as usize;
        blur(&mut alpha, w, h, radius);
        blur(&mut alpha, w, h, radius);
        for y in 0..h {
            for x in 0..w {
                let a = alpha[y * w + x] * 0.55;
                if a > 0.003 {
                    picture.blend(x as i32, y as i32, [0, 0, 0, 255], a);
                }
            }
        }
    }

    let c = rgba(title.color);
    buffer.draw(
        system,
        cache,
        Color::rgba(c[0], c[1], c[2], c[3]),
        |x, y, gw, gh, color| {
            let coverage = color.a() as f32 / 255.0;
            let paint = [color.r(), color.g(), color.b(), 255];
            for yy in 0..gh as i32 {
                for xx in 0..gw as i32 {
                    picture.blend(x + xx + origin.0 as i32, y + yy + origin.1 as i32, paint, coverage);
                }
            }
        },
    );
    picture
}

/// Text alone, tightly drawn: for scenes. Lines wrap at `wrap` pixels.
#[allow(clippy::too_many_arguments)]
pub fn text_picture(
    text: &str,
    font: &str,
    size: f32,
    bold: bool,
    italic: bool,
    color: [u8; 4],
    align: Align,
    wrap: Option<f32>,
) -> Picture {
    let size = size.clamp(2.0, 2000.0);
    let line = size * 1.2;
    let text = if text.is_empty() { " " } else { text };
    let mut guard = fonts().lock().unwrap();
    let Fonts { system, cache } = &mut *guard;
    let attrs = Attrs::new()
        .family(Family::Name(font))
        .weight(if bold { Weight::BOLD } else { Weight::NORMAL })
        .style(if italic { Style::Italic } else { Style::Normal });
    let align = match align {
        Align::Left => cosmic_text::Align::Left,
        Align::Center => cosmic_text::Align::Center,
        Align::Right => cosmic_text::Align::Right,
    };
    let mut buffer = Buffer::new(system, Metrics::new(size, line));
    buffer.set_size(wrap, None);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(system, false);
    let widest = buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0f32, f32::max)
        .max(1.0);
    buffer.set_size(Some(widest.ceil()), None);
    buffer.set_text(text, &attrs, Shaping::Advanced, Some(align));
    buffer.shape_until_scroll(system, false);
    let height = buffer
        .layout_runs()
        .map(|run| run.line_top + run.line_height)
        .fold(0.0f32, f32::max)
        .max(line);
    let pad = (size * 0.1).ceil();
    let mut picture = Picture::new((widest + pad * 2.0).ceil() as u32, (height + pad * 2.0).ceil() as u32);
    buffer.draw(
        system,
        cache,
        Color::rgba(color[0], color[1], color[2], 255),
        |x, y, gw, gh, c| {
            let coverage = c.a() as f32 / 255.0;
            let paint = [c.r(), c.g(), c.b(), color[3]];
            for yy in 0..gh as i32 {
                for xx in 0..gw as i32 {
                    picture.blend(x + xx + pad as i32, y + yy + pad as i32, paint, coverage);
                }
            }
        },
    );
    picture
}

/// A box blur of `radius` over a `w` x `h` grid, in place.
fn blur(values: &mut [f32], w: usize, h: usize, radius: usize) {
    let mut tmp = vec![0f32; values.len()];
    let span = (2 * radius + 1) as f32;
    for y in 0..h {
        let row = &values[y * w..(y + 1) * w];
        let mut sum: f32 = (0..=radius).map(|i| row[i.min(w - 1)]).sum::<f32>() + row[0] * radius as f32;
        for x in 0..w {
            tmp[y * w + x] = sum / span;
            let add = row[(x + radius + 1).min(w - 1)];
            let drop = row[x.saturating_sub(radius)];
            sum += add - drop;
        }
    }
    for x in 0..w {
        let mut sum: f32 = (0..=radius).map(|i| tmp[i.min(h - 1) * w + x]).sum::<f32>() + tmp[x] * radius as f32;
        for y in 0..h {
            values[y * w + x] = sum / span;
            let add = tmp[(y + radius + 1).min(h - 1) * w + x];
            let drop = tmp[y.saturating_sub(radius) * w + x];
            sum += add - drop;
        }
    }
}

/// A stable key for a drawing: what is drawn, its mask and the canvas.
pub fn key(generator: &Generator, mask: Option<&Mask>, canvas: (u32, u32)) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(generator).unwrap_or_default().hash(&mut hasher);
    serde_json::to_string(&mask).unwrap_or_default().hash(&mut hasher);
    canvas.hash(&mut hasher);
    hasher.finish()
}

/// The drawing as a PNG in `dir` (made once and kept), with its size.
pub fn png(
    dir: &Path,
    generator: &Generator,
    mask: Option<&Mask>,
    canvas: (u32, u32),
) -> anyhow::Result<(PathBuf, u32, u32)> {
    let path = dir.join(format!("{:016x}.png", key(generator, mask, canvas)));
    if let Ok((w, h)) = image::image_dimensions(&path) {
        return Ok((path, w, h));
    }
    std::fs::create_dir_all(dir)?;
    let picture = draw(generator, mask, canvas);
    picture.save_png(&path)?;
    Ok((path, picture.width, picture.height))
}

/// Drawn titles and colours for the viewer, kept while they are on screen.
#[derive(Default)]
pub struct Drawings {
    map: std::collections::HashMap<u64, Drawn>,
    frame: u64,
}

struct Drawn {
    image: std::sync::Arc<gpui::RenderImage>,
    size: (u32, u32),
    used: u64,
}

impl Drawings {
    /// The picture for a title or colour, drawing it if it is new.
    pub fn get(
        &mut self,
        generator: &Generator,
        mask: Option<&Mask>,
        canvas: (u32, u32),
    ) -> Option<(std::sync::Arc<gpui::RenderImage>, (u32, u32))> {
        let key = key(generator, mask, canvas);
        if !self.map.contains_key(&key) {
            let picture = draw(generator, mask, canvas);
            let image = crate::ffmpeg::render_image(picture.width, picture.height, picture.bgra())?;
            self.map.insert(
                key,
                Drawn {
                    image,
                    size: (picture.width, picture.height),
                    used: self.frame,
                },
            );
        }
        let drawn = self.map.get_mut(&key)?;
        drawn.used = self.frame;
        Some((drawn.image.clone(), drawn.size))
    }

    /// Ends a frame: pictures unused for a while are handed back for release.
    pub fn sweep(&mut self) -> Vec<std::sync::Arc<gpui::RenderImage>> {
        self.frame += 1;
        let frame = self.frame;
        let stale: Vec<u64> = self
            .map
            .iter()
            .filter(|(_, d)| frame - d.used > 90)
            .map(|(k, _)| *k)
            .collect();
        stale
            .into_iter()
            .filter_map(|k| self.map.remove(&k))
            .map(|d| d.image)
            .collect()
    }
}

/// Where export finds pictures: media files through `media`, and titles and
/// colours drawn into `dir`.
pub struct Resolver<F> {
    pub media: F,
    pub dir: PathBuf,
    pub canvas: (u32, u32),
}

impl<F: Fn(&Media) -> Option<PathBuf>> Resolve for Resolver<F> {
    fn media(&self, media: &Media) -> Option<PathBuf> {
        (self.media)(media)
    }

    fn generated(&self, clip: &Clip, generator: &Generator) -> Option<Rendered> {
        match png(&self.dir, generator, clip.look.mask.as_ref(), self.canvas) {
            Ok((path, width, height)) => Some(Rendered { path, width, height }),
            Err(e) => {
                eprintln!("splice: could not draw {}: {e}", generator.name());
                None
            }
        }
    }
}

/// Where drawn titles and colours are kept for export.
pub fn drawn_dir() -> PathBuf {
    crate::session::dirs().cache.join("drawn")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_text_with_a_system_font() {
        let title = Title {
            text: "Hello\nSplice".into(),
            shadow: false,
            ..Title::default()
        };
        let p = draw(&Generator::Text(title.clone()), None, (1920, 1080));
        assert!(p.width > 100 && p.height > 150, "{}x{}", p.width, p.height);
        let inked = p.rgba.as_chunks::<4>().0.iter().filter(|px| px[3] > 128).count();
        assert!(inked > 1000, "the letters cover pixels");

        let boxed = Title {
            background: Some(0x000000cc),
            ..title
        };
        let b = draw(&Generator::Text(boxed), None, (1920, 1080));
        assert!(b.width > p.width, "a box adds padding");
        assert_eq!(
            b.rgba[(b.width / 2 * 4 + 3) as usize],
            0xcc,
            "the box is behind the top edge's middle"
        );
    }

    #[test]
    fn colours_fill_and_masks_cut() {
        let color = Generator::Color { color: 0xef6a55ff };
        let p = draw(&color, Some(&Mask::default()), (64, 36));
        assert_eq!(
            &p.rgba[0..4],
            &[0xef, 0x6a, 0x55, 0],
            "the corner is outside the ellipse"
        );
        let middle = ((18 * 64 + 32) * 4) as usize;
        assert_eq!(p.rgba[middle + 3], 255);
        assert_ne!(
            key(&color, None, (64, 36)),
            key(&color, Some(&Mask::default()), (64, 36))
        );
    }

    #[test]
    fn lists_fonts() {
        assert!(families().len() > 5);
    }
}
