//! The open document and the app's memory of recent ones.
//!
//! A document always has a file. New projects start as "Untitled N.splice" in
//! ~/Movies/Splice and are autosaved there from the first edit; Save As moves
//! them wherever you like. Every autosave and save is a version in the file.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use splice_core::history::History;
use splice_core::model::Project;
use splice_core::store::{EXTENSION, Store, Version, VersionKind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

pub struct Dirs {
    /// Recent-files state.
    pub data: PathBuf,
    /// Media extracted from project files.
    pub cache: PathBuf,
    /// Where untitled projects live.
    pub projects: PathBuf,
}

pub fn dirs() -> &'static Dirs {
    static DIRS: OnceLock<Dirs> = OnceLock::new();
    DIRS.get_or_init(|| {
        let base = directories::ProjectDirs::from("", "", "Splice");
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        let data = base
            .as_ref()
            .map_or_else(|| home.join(".splice"), |b| b.data_dir().to_path_buf());
        let cache = base
            .as_ref()
            .map_or_else(|| home.join(".splice/cache"), |b| b.cache_dir().to_path_buf());
        let projects = directories::UserDirs::new()
            .and_then(|u| u.video_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| home.join("Movies"))
            .join("Splice");
        for dir in [&data, &cache.join("media"), &projects] {
            let _ = std::fs::create_dir_all(dir);
        }
        Dirs { data, cache, projects }
    })
}

pub struct Document {
    pub store: Store,
    pub project: Project,
    pub history: History,
    /// Edited since the last write to disk.
    dirty: bool,
    /// Edited since the last explicit save (what the title shows).
    pub edited: bool,
    pub last_write: Option<Instant>,
    /// Counts changes, so a reader can tell whether the project moved on.
    pub revision: u64,
}

impl Document {
    pub fn new_untitled() -> Result<Document> {
        let dir = &dirs().projects;
        let path = (1..)
            .map(|n| dir.join(format!("Untitled {n}.{EXTENSION}")))
            .find(|p| !p.exists())
            .expect("a free name");
        let store = Store::open(&path)?;
        let project = Project::default();
        store.save_version(&project, VersionKind::Auto, None)?;
        Ok(Document::with(store, project))
    }

    pub fn open(path: &Path) -> Result<Document> {
        let store = Store::open(path)?;
        let project = store.latest()?.unwrap_or_default();
        Ok(Document::with(store, project))
    }

    fn with(store: Store, project: Project) -> Document {
        Document {
            store,
            project,
            history: History::default(),
            dirty: false,
            edited: false,
            last_write: None,
            revision: 0,
        }
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub fn name(&self) -> String {
        self.path()
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".into())
    }

    pub fn is_untitled(&self) -> bool {
        self.path().parent() == Some(dirs().projects.as_path()) && self.name().starts_with("Untitled")
    }

    /// Applies an edit; records undo when it changed anything.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut Project) -> R) -> R {
        let before = self.project.clone();
        let result = f(&mut self.project);
        self.commit(before);
        result
    }

    /// Records undo for changes already made to `project` (drags edit live).
    pub fn commit(&mut self, before: Project) {
        if before != self.project {
            self.history.record(before);
            self.touch();
        }
    }

    /// Marks the project changed without an undo step (a render finishing).
    pub fn touch(&mut self) {
        self.revision += 1;
        self.dirty = true;
        self.edited = true;
    }

    pub fn undo(&mut self) -> bool {
        let changed = self.history.undo(&mut self.project);
        if changed {
            self.touch();
        }
        changed
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn redo(&mut self) -> bool {
        let changed = self.history.redo(&mut self.project);
        if changed {
            self.touch();
        }
        changed
    }

    /// Writes an autosave version if anything changed.
    pub fn autosave(&mut self) -> Result<bool> {
        if !self.dirty {
            return Ok(false);
        }
        self.store.save_version(&self.project, VersionKind::Auto, None)?;
        self.dirty = false;
        self.last_write = Some(Instant::now());
        Ok(true)
    }

    /// Writes a version you chose to keep.
    pub fn save(&mut self, label: Option<&str>) -> Result<()> {
        self.store.save_version(&self.project, VersionKind::Save, label)?;
        self.dirty = false;
        self.edited = false;
        self.last_write = Some(Instant::now());
        // Bytes no kept version uses (old renders, replaced media) go.
        if let Err(e) = self.store.collect_garbage() {
            eprintln!("splice: could not tidy {}: {e}", self.path().display());
        }
        Ok(())
    }

    /// Saves a copy at `dest` and continues there. An untitled original is removed.
    pub fn save_as(&mut self, dest: &Path) -> Result<()> {
        self.autosave()?;
        let old = self.path().to_path_buf();
        let was_untitled = self.is_untitled();
        self.store.copy_to(dest)?;
        self.store = Store::open(dest)?;
        self.save(None)?;
        if was_untitled && old != dest {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    }

    pub fn versions(&self) -> Vec<Version> {
        self.store.versions().unwrap_or_default()
    }

    /// Brings back an earlier version as an undoable edit.
    pub fn restore(&mut self, id: i64) -> Result<()> {
        let project = self.store.load_version(id)?;
        let before = std::mem::replace(&mut self.project, project);
        self.commit(before);
        Ok(())
    }

    /// Final write before the document goes away. An untitled project nobody
    /// touched is deleted rather than left as clutter.
    pub fn finish(&mut self) {
        let path = self.path().to_path_buf();
        if self.is_untitled() && self.project.is_empty() && self.project.media.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        if let Err(e) = self.autosave() {
            eprintln!("splice: could not save {}: {e}", path.display());
        }
    }
}

/// Recently opened projects, newest first.
#[derive(Default, Serialize, Deserialize)]
pub struct Recent {
    pub files: Vec<PathBuf>,
}

impl Recent {
    fn path() -> PathBuf {
        dirs().data.join("recent.json")
    }

    pub fn load() -> Recent {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn push(path: &Path) {
        let mut recent = Recent::load();
        recent.files.retain(|p| p != path && p.exists());
        recent.files.insert(0, path.to_path_buf());
        recent.files.truncate(12);
        if let Ok(json) = serde_json::to_vec_pretty(&recent) {
            let _ = std::fs::write(Self::path(), json);
        }
    }

    pub fn last() -> Option<PathBuf> {
        Recent::load().files.into_iter().find(|p| p.exists())
    }
}

/// How the window's panels are arranged, remembered between launches.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub bin_width: f32,
    pub bin_open: bool,
    pub timeline_height: f32,
    pub inspector_open: bool,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            bin_width: 248.0,
            bin_open: true,
            timeline_height: 300.0,
            inspector_open: true,
        }
    }
}

impl Layout {
    fn path() -> PathBuf {
        dirs().data.join("layout.json")
    }

    pub fn load() -> Layout {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn store(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(Self::path(), json);
        }
    }
}
