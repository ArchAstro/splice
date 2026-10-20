//! The `.splice` project file: one SQLite database holding the media bytes and
//! every saved version of the timeline.
//!
//! - `media` / `media_chunks`: imported files, content-addressed by BLAKE3
//!   hash and split into chunks (SQLite caps a single value near 1 GB).
//! - `versions`: project JSON snapshots. Autosaves and explicit saves are both
//!   rows; opening a file loads the newest. Old autosaves are thinned out,
//!   explicit saves are kept.
//! - `meta`: the format marker and version, for migrations.
//!
//! The rollback journal (not WAL) keeps the project a single file on disk.

use crate::model::Project;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const EXTENSION: &str = "splice";
/// Bump with a step in `upgrade` when the tables change.
pub const FORMAT_VERSION: i64 = 1;
/// "SPLC" — lets `file(1)` and us tell a project from any other SQLite file.
const APPLICATION_ID: i64 = 0x5350_4c43;
const CHUNK: usize = 32 * 1024 * 1024;
/// Autosaves kept beyond the explicit saves.
const KEEP_AUTOSAVES: i64 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionKind {
    Auto,
    Save,
}

impl VersionKind {
    fn as_str(self) -> &'static str {
        match self {
            VersionKind::Auto => "auto",
            VersionKind::Save => "save",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Version {
    pub id: i64,
    pub created: SystemTime,
    pub kind: VersionKind,
    pub label: Option<String>,
}

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    /// Opens a project file, creating it when missing.
    pub fn open(path: &Path) -> Result<Store> {
        let fresh = !path.exists();
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "DELETE")?;
        let store = Store {
            conn,
            path: path.to_path_buf(),
        };
        if fresh {
            store.create()?;
        } else {
            store.check()?;
        }
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn create(&self) -> Result<()> {
        self.conn.pragma_update(None, "application_id", APPLICATION_ID)?;
        self.conn.execute_batch(
            "BEGIN;
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE media(hash TEXT PRIMARY KEY, ext TEXT NOT NULL, size INTEGER NOT NULL,
                                complete INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE media_chunks(hash TEXT NOT NULL, idx INTEGER NOT NULL, data BLOB NOT NULL,
                                       PRIMARY KEY(hash, idx));
             CREATE TABLE versions(id INTEGER PRIMARY KEY AUTOINCREMENT, created_ms INTEGER NOT NULL,
                                   kind TEXT NOT NULL, label TEXT, project TEXT NOT NULL);
             COMMIT;",
        )?;
        self.conn.execute("INSERT INTO meta VALUES ('format', 'splice')", [])?;
        self.conn.execute(
            "INSERT INTO meta VALUES ('format_version', ?1)",
            [FORMAT_VERSION.to_string()],
        )?;
        Ok(())
    }

    fn check(&self) -> Result<()> {
        let app_id: i64 = self.conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        if app_id != APPLICATION_ID {
            bail!("{} is not a Splice project", self.path.display());
        }
        let version: i64 = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'format_version'", [], |r| {
                r.get::<_, String>(0)
            })?
            .parse()?;
        if version > FORMAT_VERSION {
            bail!(
                "{} was saved by a newer Splice (file format {version})",
                self.path.display()
            );
        }
        self.upgrade(version)
    }

    fn upgrade(&self, from: i64) -> Result<()> {
        // Future steps: `if from < 2 { ALTER TABLE ... }`, then record the new version.
        if from < FORMAT_VERSION {
            self.conn.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'format_version'",
                [FORMAT_VERSION.to_string()],
            )?;
        }
        Ok(())
    }

    // Versions.

    pub fn save_version(&self, project: &Project, kind: VersionKind, label: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO versions(created_ms, kind, label, project) VALUES (?1, ?2, ?3, ?4)",
            params![now_ms(), kind.as_str(), label, project.to_json()],
        )?;
        let id = self.conn.last_insert_rowid();
        if kind == VersionKind::Auto {
            self.conn.execute(
                "DELETE FROM versions WHERE kind = 'auto' AND id NOT IN
                   (SELECT id FROM versions WHERE kind = 'auto' ORDER BY id DESC LIMIT ?1)",
                [KEEP_AUTOSAVES],
            )?;
        }
        Ok(id)
    }

    pub fn latest(&self) -> Result<Option<Project>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT project FROM versions ORDER BY id DESC LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?;
        json.map(|j| Project::from_json(&j)).transpose()
    }

    pub fn load_version(&self, id: i64) -> Result<Project> {
        let json: String = self
            .conn
            .query_row("SELECT project FROM versions WHERE id = ?1", [id], |r| r.get(0))?;
        Project::from_json(&json)
    }

    /// Newest first.
    pub fn versions(&self) -> Result<Vec<Version>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, created_ms, kind, label FROM versions ORDER BY id DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(Version {
                id: r.get(0)?,
                created: UNIX_EPOCH + Duration::from_millis(r.get::<_, i64>(1)? as u64),
                kind: if r.get::<_, String>(2)? == "save" {
                    VersionKind::Save
                } else {
                    VersionKind::Auto
                },
                label: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // Media.

    pub fn has_media(&self, hash: &str) -> Result<bool> {
        let complete: Option<i64> = self
            .conn
            .query_row("SELECT complete FROM media WHERE hash = ?1", [hash], |r| r.get(0))
            .optional()?;
        Ok(complete == Some(1))
    }

    /// Copies a file into the project. Each chunk commits on its own so a long
    /// import never holds the write lock against autosave.
    pub fn put_media(&self, hash: &str, ext: &str, source: &Path) -> Result<()> {
        if self.has_media(hash)? {
            return Ok(());
        }
        let mut file = File::open(source).with_context(|| format!("reading {}", source.display()))?;
        let size = file.metadata()?.len() as i64;
        self.conn.execute("DELETE FROM media_chunks WHERE hash = ?1", [hash])?;
        self.conn.execute(
            "INSERT OR REPLACE INTO media(hash, ext, size, complete) VALUES (?1, ?2, ?3, 0)",
            params![hash, ext, size],
        )?;
        let mut buffer = vec![0u8; CHUNK];
        let mut idx = 0i64;
        loop {
            let n = read_full(&mut file, &mut buffer)?;
            if n == 0 {
                break;
            }
            self.conn.execute(
                "INSERT INTO media_chunks(hash, idx, data) VALUES (?1, ?2, ?3)",
                params![hash, idx, &buffer[..n]],
            )?;
            idx += 1;
        }
        self.conn
            .execute("UPDATE media SET complete = 1 WHERE hash = ?1", [hash])?;
        Ok(())
    }

    /// Writes embedded media out to `dest` (via a temporary file, so a reader
    /// never sees half of it).
    pub fn extract_media(&self, hash: &str, dest: &Path) -> Result<()> {
        if !self.has_media(hash)? {
            bail!("media {hash} is not in this project");
        }
        let partial = dest.with_extension("partial");
        {
            let mut out = File::create(&partial)?;
            let mut stmt = self
                .conn
                .prepare("SELECT data FROM media_chunks WHERE hash = ?1 ORDER BY idx")?;
            let mut rows = stmt.query([hash])?;
            while let Some(row) = rows.next()? {
                let data = row.get_ref(0)?.as_blob()?;
                out.write_all(data)?;
            }
            out.sync_all()?;
        }
        std::fs::rename(&partial, dest)?;
        Ok(())
    }

    /// Writes a compact copy of the whole project to `dest` (Save As).
    pub fn copy_to(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        self.conn.execute("VACUUM INTO ?1", [dest.to_string_lossy()])?;
        Ok(())
    }

    /// Drops media no saved version refers to.
    pub fn collect_garbage(&self) -> Result<usize> {
        let mut referenced = std::collections::HashSet::new();
        let mut stmt = self.conn.prepare("SELECT project FROM versions")?;
        for json in stmt.query_map([], |r| r.get::<_, String>(0))? {
            if let Ok(project) = Project::from_json(&json?) {
                referenced.extend(project.media.into_iter().map(|m| m.hash));
            }
        }
        let hashes: Vec<String> = self
            .conn
            .prepare("SELECT hash FROM media")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut removed = 0;
        for hash in hashes.into_iter().filter(|h| !referenced.contains(h)) {
            self.conn.execute("DELETE FROM media_chunks WHERE hash = ?1", [&hash])?;
            self.conn.execute("DELETE FROM media WHERE hash = ?1", [&hash])?;
            removed += 1;
        }
        Ok(removed)
    }
}

/// BLAKE3 of a file's bytes, hex.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_mmap(path)
        .with_context(|| format!("hashing {}", path.display()))?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn read_full(file: &mut File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Media, MediaId, MediaKind};
    use crate::time::SECOND;

    #[test]
    fn versions_and_media_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.splice");
        let source = dir.path().join("clip.bin");
        let bytes: Vec<u8> = (0..(CHUNK + 1000)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &bytes).unwrap();
        let hash = hash_file(&source).unwrap();

        let mut project = Project::default();
        let media = project.add_media(Media {
            id: MediaId(0),
            name: "clip".into(),
            hash: hash.clone(),
            ext: "bin".into(),
            kind: MediaKind::Video,
            duration: 3 * SECOND,
            width: 640,
            height: 360,
            has_audio: false,
            source: source.clone(),
            recipe: None,
        });
        project.append_clip(media);

        {
            let store = Store::open(&path).unwrap();
            store.put_media(&hash, "bin", &source).unwrap();
            store.save_version(&project, VersionKind::Save, Some("first")).unwrap();
            project.settings.fps = 24;
            store.save_version(&project, VersionKind::Auto, None).unwrap();
        }

        let store = Store::open(&path).unwrap();
        assert_eq!(store.latest().unwrap().unwrap(), project);
        let versions = store.versions().unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[1].kind, VersionKind::Save);
        assert_eq!(store.load_version(versions[1].id).unwrap().settings.fps, 30);

        let out = dir.path().join("out.bin");
        store.extract_media(&hash, &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), bytes);

        let copy = dir.path().join("b.splice");
        store.copy_to(&copy).unwrap();
        assert!(Store::open(&copy).unwrap().has_media(&hash).unwrap());
        assert_eq!(store.collect_garbage().unwrap(), 0);
    }

    #[test]
    fn autosaves_are_thinned_saves_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("p.splice")).unwrap();
        let project = Project::default();
        store.save_version(&project, VersionKind::Save, None).unwrap();
        for _ in 0..(KEEP_AUTOSAVES + 10) {
            store.save_version(&project, VersionKind::Auto, None).unwrap();
        }
        let versions = store.versions().unwrap();
        assert_eq!(versions.len() as i64, KEEP_AUTOSAVES + 1);
        assert!(versions.iter().any(|v| v.kind == VersionKind::Save));
    }

    #[test]
    fn rejects_foreign_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE t(x)")
            .unwrap();
        assert!(Store::open(&path).is_err());
    }
}
