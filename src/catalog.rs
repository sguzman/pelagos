//! Private SQLite media index. Indexing never writes to user media or a Jellyfin database.
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::media::{self, MediaKind};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const DB_NAME: &str = "pelagos.sqlite3";
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Serialize)]
pub struct IndexedItem {
    pub relative_path: String,
    pub kind: String,
    pub size_bytes: i64,
}

#[derive(Debug, Serialize)]
pub struct CatalogSummary {
    pub library_root: String,
    pub indexed: usize,
    pub stale_removed: usize,
    pub database: PathBuf,
}

fn path_text(path: &Path) -> Result<&str> {
    Ok(path
        .to_str()
        .ok_or("non-UTF-8 paths are not supported by the initial SQLite catalog")?)
}

fn canonical_root(root: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(root)?;
    if !canonical.is_dir() {
        return Err("media root must be a directory".into());
    }
    path_text(&canonical)?;
    Ok(canonical)
}

fn database_path(state_dir: &Path) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(state_dir)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(state_dir)?;
    Ok(state_dir.join(DB_NAME))
}

fn connect(state_dir: &Path) -> Result<Connection> {
    let path = database_path(state_dir)?;
    let db = Connection::open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(format!("catalog schema version {version} is newer than supported").into());
    }
    if version == 0 {
        db.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS media_files (
                library_root TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('video', 'audio', 'image')),
                size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
                last_seen_at INTEGER NOT NULL DEFAULT (unixepoch()),
                PRIMARY KEY (library_root, relative_path)
             );
             PRAGMA user_version=1;
             COMMIT;",
        )?;
    }
    Ok(db)
}

fn media_kind(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::Image => "image",
    }
}

/// Scans completely before beginning a transaction. If the read-only walk fails,
/// no existing catalog rows can be deleted or replaced.
pub fn index(state_dir: &Path, root: &Path) -> Result<CatalogSummary> {
    let root = canonical_root(root)?;
    let scan = media::scan(&root)?;
    let mut db = connect(state_dir)?;
    let tx = db.transaction()?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS pelagos_seen(path TEXT PRIMARY KEY);
         DELETE FROM pelagos_seen;",
    )?;
    {
        let mut put = tx.prepare(
            "INSERT INTO media_files (library_root, relative_path, kind, size_bytes)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(library_root, relative_path) DO UPDATE SET
                kind = excluded.kind,
                size_bytes = excluded.size_bytes,
                last_seen_at = unixepoch()",
        )?;
        let mut seen = tx.prepare("INSERT INTO pelagos_seen (path) VALUES (?1)")?;
        for file in &scan.files {
            let relative = path_text(&file.path)?;
            seen.execute(params![relative])?;
            let bytes = i64::try_from(file.size_bytes)?;
            put.execute(params![
                path_text(&root)?,
                relative,
                media_kind(file.kind),
                bytes
            ])?;
        }
    }
    let stale_removed = tx.execute(
        "DELETE FROM media_files
         WHERE library_root=?1 AND NOT EXISTS
         (SELECT 1 FROM pelagos_seen WHERE path = media_files.relative_path)",
        params![path_text(&root)?],
    )?;
    tx.commit()?;

    Ok(CatalogSummary {
        library_root: path_text(&root)?.to_string(),
        indexed: scan.files.len(),
        stale_removed,
        database: state_dir.join(DB_NAME),
    })
}

/// Return an indexed library without rescanning or modifying source media.
/// A library root must still exist; stale records are available only after a successful index.
pub fn list(state_dir: &Path, root: &Path) -> Result<Vec<IndexedItem>> {
    let root = canonical_root(root)?;
    let db = connect(state_dir)?;
    let mut query = db.prepare(
        "SELECT relative_path, kind, size_bytes
         FROM media_files WHERE library_root=?1 ORDER BY relative_path",
    )?;
    let rows = query.query_map(params![path_text(&root)?], |row| {
        Ok(IndexedItem {
            relative_path: row.get(0)?,
            kind: row.get(1)?,
            size_bytes: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_incremental_and_cleans_only_stale_rows() {
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let movies = fixture.path().join("movies");
        let music = fixture.path().join("music");
        fs::create_dir_all(&movies).unwrap();
        fs::create_dir_all(&music).unwrap();
        fs::write(movies.join("A.mp4"), b"first").unwrap();
        fs::write(movies.join("ignored.txt"), b"text").unwrap();
        fs::write(music.join("track.flac"), b"sound").unwrap();

        let initial = index(&state, &movies).unwrap();
        assert_eq!(initial.indexed, 1);
        assert_eq!(initial.stale_removed, 0);
        assert_eq!(index(&state, &music).unwrap().indexed, 1);
        assert_eq!(list(&state, &movies).unwrap()[0].size_bytes, 5);

        fs::remove_file(movies.join("A.mp4")).unwrap();
        fs::write(movies.join("B.mkv"), b"second-file").unwrap();
        let rescanned = index(&state, &movies).unwrap();
        assert_eq!(rescanned.indexed, 1);
        assert_eq!(rescanned.stale_removed, 1);
        assert_eq!(list(&state, &movies).unwrap()[0].relative_path, "B.mkv");
        assert_eq!(list(&state, &music).unwrap()[0].relative_path, "track.flac");
        assert_eq!(fs::read(movies.join("B.mkv")).unwrap(), b"second-file");
    }

    #[test]
    fn invalid_root_does_not_erase_database_rows() {
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let source = fixture.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("Movie.mp4"), b"movie").unwrap();
        index(&state, &source).unwrap();
        assert!(index(&state, &fixture.path().join("missing")).is_err());
        assert_eq!(list(&state, &source).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_other_library_is_not_indexed() {
        use std::os::unix::fs::symlink;
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let source = fixture.path().join("media");
        let outside = fixture.path().join("outside");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("private.mp4"), b"other").unwrap();
        symlink(&outside, source.join("linked")).unwrap();
        assert_eq!(index(&state, &source).unwrap().indexed, 0);
        assert!(list(&state, &source).unwrap().is_empty());
    }

    #[test]
    fn future_schema_is_rejected_without_migration() {
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let db = connect(&state).unwrap();
        db.execute_batch("PRAGMA user_version=1000;").unwrap();
        assert!(connect(&state).is_err());
    }
}
