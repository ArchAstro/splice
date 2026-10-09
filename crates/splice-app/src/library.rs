//! Runtime assets for each piece of media, keyed by content hash: where a
//! readable copy lives, its bin thumbnail, filmstrip, still (for images), PCM
//! and waveform peaks. Built on background threads; nothing here is saved.

use crate::ffmpeg::{self, Pcm};
use gpui::RenderImage;
use splice_core::model::{Media, MediaKind};
use splice_core::time::{SECOND, Time};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Width of one waveform bucket.
pub const PEAK_BUCKET: Time = SECOND / 100;

#[derive(Default)]
pub struct Assets {
    pub path: Option<PathBuf>,
    pub thumb: Option<Arc<RenderImage>>,
    pub strip: Vec<(Time, Arc<RenderImage>)>,
    pub still: Option<Arc<RenderImage>>,
    pub pcm: Option<Arc<Pcm>>,
    pub peaks: Option<Arc<Vec<u8>>>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Library {
    pub assets: HashMap<String, Assets>,
}

impl Library {
    pub fn get(&self, hash: &str) -> Option<&Assets> {
        self.assets.get(hash)
    }

    pub fn path(&self, hash: &str) -> Option<&Path> {
        self.assets.get(hash).and_then(|a| a.path.as_deref())
    }

    pub fn entry(&mut self, hash: &str) -> &mut Assets {
        self.assets.entry(hash.to_string()).or_default()
    }

    pub fn clear(&mut self) {
        self.assets.clear();
    }

    pub fn apply(&mut self, hash: &str, built: Built) {
        let assets = self.entry(hash);
        assets.thumb = built.thumb;
        assets.strip = built.strip;
        assets.still = built.still;
        assets.pcm = built.pcm.map(Arc::new);
        assets.peaks = built.peaks.map(Arc::new);
        assets.error = built.error;
        assets.loading = false;
    }

    /// Every image the library holds, for releasing GPU memory on close.
    pub fn images(&self) -> Vec<Arc<RenderImage>> {
        self.assets
            .values()
            .flat_map(|a| {
                a.thumb
                    .iter()
                    .chain(a.still.iter())
                    .chain(a.strip.iter().map(|(_, i)| i))
                    .cloned()
            })
            .collect()
    }
}

pub struct Built {
    pub thumb: Option<Arc<RenderImage>>,
    pub strip: Vec<(Time, Arc<RenderImage>)>,
    pub still: Option<Arc<RenderImage>>,
    pub pcm: Option<Pcm>,
    pub peaks: Option<Vec<u8>>,
    pub error: Option<String>,
}

/// Decodes everything the editor shows for `media`. Blocking; run it off the main thread.
pub fn build(media: &Media, path: &Path, rate: u32) -> Built {
    let mut built = Built {
        thumb: None,
        strip: Vec::new(),
        still: None,
        pcm: None,
        peaks: None,
        error: None,
    };
    let mut error = None;
    let mut note = |e: anyhow::Error| {
        error.get_or_insert(e.to_string());
    };
    if media.is_visual() {
        let (w, h) = ffmpeg::fit(media.width, media.height, 192, 108);
        let at = if media.kind == MediaKind::Video {
            (media.duration / 3).min(SECOND)
        } else {
            0
        };
        match ffmpeg::still(path, at, w, h) {
            Ok(image) => built.thumb = Some(image),
            Err(e) => note(e),
        }
    }
    match media.kind {
        MediaKind::Video => {
            let (w, h) = ffmpeg::fit(media.width, media.height, 96, 54);
            match ffmpeg::filmstrip(path, media.duration, w, h) {
                Ok(strip) => built.strip = strip,
                Err(e) => note(e),
            }
        }
        MediaKind::Image => {
            let (w, h) = ffmpeg::fit(media.width, media.height, 2048, 2048);
            match ffmpeg::still(path, 0, w, h) {
                Ok(image) => built.still = Some(image),
                Err(e) => note(e),
            }
        }
        MediaKind::Audio => {}
    }
    if media.has_audio {
        match ffmpeg::decode_audio(path, rate) {
            Ok(pcm) => {
                built.peaks = Some(ffmpeg::peaks(&pcm, PEAK_BUCKET));
                built.pcm = Some(pcm);
            }
            Err(e) => note(e),
        }
    }
    built.error = error;
    built
}

/// Where extracted media lives between sessions.
pub fn cache_path(cache_dir: &Path, media: &Media) -> PathBuf {
    let ext = if media.ext.is_empty() { "bin" } else { &media.ext };
    cache_dir.join("media").join(format!("{}.{ext}", media.hash))
}
