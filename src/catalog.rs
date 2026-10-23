//! Private SQLite media index. Indexing never writes to user media or a Jellyfin database.
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::media::{self, MediaKind};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const DB_NAME: &str = "pelagos.sqlite3";
const SCHEMA_VERSION: i64 = 4;

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
    if version < 2 {
        db.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS libraries (
                 root TEXT PRIMARY KEY NOT NULL,
                 created_unix INTEGER NOT NULL DEFAULT (unixepoch())
             );
             INSERT OR IGNORE INTO libraries(root)
             SELECT DISTINCT library_root FROM media_files;
             PRAGMA user_version=2;
             COMMIT;",
        )?;
    }
    if version < 3 {
        db.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE media_files ADD COLUMN first_indexed_unix INTEGER NOT NULL DEFAULT 0;
             UPDATE media_files SET first_indexed_unix=last_seen_at;
             CREATE INDEX IF NOT EXISTS idx_media_files_first_indexed
                 ON media_files(first_indexed_unix DESC, library_root, relative_path);
             PRAGMA user_version=3;
             COMMIT;",
        )?;
    }
    if version < 4 {
        db.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE libraries ADD COLUMN display_name TEXT;
             PRAGMA user_version=4;
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
    tx.execute(
        "INSERT OR IGNORE INTO libraries(root) VALUES (?1)",
        params![path_text(&root)?],
    )?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS pelagos_seen(path TEXT PRIMARY KEY);
         DELETE FROM pelagos_seen;",
    )?;
    {
        let mut put = tx.prepare(
            "INSERT INTO media_files
                (library_root, relative_path, kind, size_bytes, first_indexed_unix)
             VALUES (?1, ?2, ?3, ?4, unixepoch())
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

/// Atomically rescan *only* previously registered roots into private SQLite.
/// Preflight and enumerate every root before changing any indexed rows. If an
/// offline directory or replaced symlink fails validation, all old rows remain
/// intact instead of being silently deleted or redirected elsewhere.
pub fn refresh_registered(state_dir: &Path) -> Result<usize> {
    let registered = library_roots(state_dir)?;
    let mut reports = Vec::with_capacity(registered.len());
    for saved in registered {
        let root = PathBuf::from(&saved);
        if fs::canonicalize(&root)? != root {
            return Err("registered media root has been redirected".into());
        }
        let scanned = media::scan(&root)?;
        reports.push((saved, scanned));
    }
    let mut db = connect(state_dir)?;
    let tx = db.transaction()?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS pelagos_refresh_seen(path TEXT PRIMARY KEY);",
    )?;
    for (root, scan) in &reports {
        tx.execute("DELETE FROM pelagos_refresh_seen", [])?;
        {
            let mut put = tx.prepare(
                "INSERT INTO media_files
                    (library_root, relative_path, kind, size_bytes, first_indexed_unix)
                 VALUES (?1, ?2, ?3, ?4, unixepoch())
                 ON CONFLICT(library_root, relative_path) DO UPDATE SET
                    kind=excluded.kind,
                    size_bytes=excluded.size_bytes,
                    last_seen_at=unixepoch()",
            )?;
            let mut seen = tx.prepare("INSERT INTO pelagos_refresh_seen (path) VALUES (?1)")?;
            for file in &scan.files {
                let relative = path_text(&file.path)?;
                seen.execute(params![relative])?;
                put.execute(params![
                    root,
                    relative,
                    media_kind(file.kind),
                    i64::try_from(file.size_bytes)?,
                ])?;
            }
        }
        tx.execute(
            "DELETE FROM media_files
             WHERE library_root=?1 AND NOT EXISTS
             (SELECT 1 FROM pelagos_refresh_seen WHERE path=media_files.relative_path)",
            params![root],
        )?;
    }
    tx.commit()?;
    Ok(reports.len())
}

/// Result of unregistering a virtual library by its unique display basename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryRemoval {
    Removed,
    Missing,
    Ambiguous,
}

/// Remove only Pelagos catalog records and explicit user-library grants.
/// ATTACH keeps both private SQLite files in one rollback-journal transaction
/// so a deleted root cannot regain stale grants when later registered again.
/// Source media files are never deleted, even for an offline library root.
pub fn unregister_by_name(state_dir: &Path, name: &str) -> Result<LibraryRemoval> {
    let mut db = connect(state_dir)?;
    let auth_file = state_dir.join("pelagos-auth.sqlite3");
    if !auth_file.is_file() {
        return Err("authentication database is unavailable".into());
    }
    db.execute(
        "ATTACH DATABASE ?1 AS permission_store",
        params![path_text(&auth_file)?],
    )?;
    let tx = db.transaction()?;
    let matching = {
        let mut stmt = tx.prepare("SELECT root, display_name FROM libraries ORDER BY root")?;
        let all = stmt
            .query_map([], |row| {
                let root: String = row.get(0)?;
                let label: Option<String> = row.get(1)?;
                Ok((root, label))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        all.into_iter()
            .filter(|(root, label)| {
                label.as_deref().unwrap_or_else(|| {
                    Path::new(root)
                        .file_name()
                        .and_then(|part| part.to_str())
                        .unwrap_or("Library")
                }) == name
            })
            .map(|(root, _)| root)
            .collect::<Vec<_>>()
    };
    let root = match matching.as_slice() {
        [] => return Ok(LibraryRemoval::Missing),
        [only] => only,
        _ => return Ok(LibraryRemoval::Ambiguous),
    };
    tx.execute(
        "DELETE FROM permission_store.pelagos_library_access WHERE root=?1",
        params![root],
    )?;
    tx.execute(
        "DELETE FROM media_files WHERE library_root=?1",
        params![root],
    )?;
    tx.execute("DELETE FROM libraries WHERE root=?1", params![root])?;
    tx.commit()?;
    Ok(LibraryRemoval::Removed)
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

#[derive(Debug, Clone)]
pub struct CatalogFile {
    pub root: String,
    pub relative_path: String,
    pub kind: String,
    pub size_bytes: i64,
}

/// Read-only aggregate of Pelagos' catalog across explicitly indexed library roots.
pub fn all_files(state_dir: &Path) -> Result<Vec<CatalogFile>> {
    let db = connect(state_dir)?;
    let mut query = db.prepare(
        "SELECT library_root, relative_path, kind, size_bytes
         FROM media_files ORDER BY library_root, relative_path",
    )?;
    let rows = query.query_map([], |row| {
        Ok(CatalogFile {
            root: row.get(0)?,
            relative_path: row.get(1)?,
            kind: row.get(2)?,
            size_bytes: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Stable newest-first ordering by when Pelagos first indexed each file,
/// not its last library rescan. v1/v2 imports preserve only the best known
/// historical last-seen timestamp until a true first-indexed value exists.
pub fn latest_file_keys(state_dir: &Path) -> Result<Vec<(String, String)>> {
    let db = connect(state_dir)?;
    let mut query = db.prepare(
        "SELECT library_root, relative_path FROM media_files
         ORDER BY first_indexed_unix DESC, library_root ASC, relative_path ASC",
    )?;
    let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub root: String,
    pub name: String,
}

/// Durable virtual display names; never change the library root or item IDs.
pub fn library_entries(state_dir: &Path) -> Result<Vec<LibraryEntry>> {
    let db = connect(state_dir)?;
    let mut query = db.prepare("SELECT root, display_name FROM libraries ORDER BY root")?;
    let rows = query.query_map([], |row| {
        let root: String = row.get(0)?;
        let custom: Option<String> = row.get(1)?;
        let name = custom.unwrap_or_else(|| {
            Path::new(&root)
                .file_name()
                .map(|leaf| leaf.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Library".to_owned())
        });
        Ok(LibraryEntry { root, name })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryRename {
    Renamed,
    Missing,
    Ambiguous,
    Conflict,
}

/// Rename virtual-library display metadata, never the media directory itself.
/// Resolve both names under an immediate transaction to avoid duplicate labels.
pub fn rename_by_name(
    state_dir: &Path,
    current_name: &str,
    new_name: &str,
) -> Result<LibraryRename> {
    let mut db = connect(state_dir)?;
    let tx = db.transaction()?;
    let entries = {
        let mut query = tx.prepare("SELECT root, display_name FROM libraries ORDER BY root")?;
        query
            .query_map([], |row| {
                let root: String = row.get(0)?;
                let custom: Option<String> = row.get(1)?;
                let name = custom.unwrap_or_else(|| {
                    Path::new(&root)
                        .file_name()
                        .map(|leaf| leaf.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Library".to_owned())
                });
                Ok(LibraryEntry { root, name })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let found = entries
        .iter()
        .filter(|entry| entry.name.eq_ignore_ascii_case(current_name))
        .collect::<Vec<_>>();
    let chosen = match found.as_slice() {
        [] => return Ok(LibraryRename::Missing),
        [one] => *one,
        _ => return Ok(LibraryRename::Ambiguous),
    };
    if entries
        .iter()
        .any(|entry| entry.root != chosen.root && entry.name.eq_ignore_ascii_case(new_name))
    {
        return Ok(LibraryRename::Conflict);
    }
    tx.execute(
        "UPDATE libraries SET display_name=?1 WHERE root=?2",
        params![new_name, chosen.root],
    )?;
    tx.commit()?;
    Ok(LibraryRename::Renamed)
}

pub fn library_roots(state_dir: &Path) -> Result<Vec<String>> {
    let db = connect(state_dir)?;
    let mut query = db.prepare("SELECT root FROM libraries ORDER BY root")?;
    let rows = query.query_map([], |row| row.get(0))?;
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
    fn empty_libraries_remain_registered_after_clean_rescans() {
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let library = fixture.path().join("Movies");
        fs::create_dir(&library).unwrap();
        assert_eq!(index(&state, &library).unwrap().indexed, 0);
        let name = path_text(&fs::canonicalize(&library).unwrap())
            .unwrap()
            .to_owned();
        assert_eq!(library_roots(&state).unwrap(), vec![name.clone()]);
        fs::write(library.join("Film.mp4"), b"video").unwrap();
        assert_eq!(index(&state, &library).unwrap().indexed, 1);
        fs::remove_file(library.join("Film.mp4")).unwrap();
        assert_eq!(index(&state, &library).unwrap().stale_removed, 1);
        assert!(list(&state, &library).unwrap().is_empty());
        assert_eq!(library_roots(&state).unwrap(), vec![name]);
    }

    #[test]
    fn v1_catalog_migrates_existing_roots_without_dropping_files() {
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let path = database_path(&state).unwrap();
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE media_files (
                library_root TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                kind TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL DEFAULT (unixepoch()),
                PRIMARY KEY (library_root, relative_path)
            );
            INSERT INTO media_files(library_root, relative_path, kind, size_bytes)
            VALUES('/existing', 'A.mp4', 'video', 14);
            PRAGMA user_version=1;",
        )
        .unwrap();
        drop(db);
        let first = connect(&state).unwrap();
        let version: i64 = first
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        assert_eq!(library_roots(&state).unwrap(), vec!["/existing".to_owned()]);
        assert_eq!(all_files(&state).unwrap()[0].relative_path, "A.mp4");
        assert_eq!(library_roots(&state).unwrap().len(), 1);
    }

    #[test]
    fn first_indexed_timestamps_survive_rescans_and_reset_only_after_readdition() {
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let movies = fixture.path().join("Movies");
        fs::create_dir(&movies).unwrap();
        fs::write(movies.join("Alpha.mp4"), b"original").unwrap();
        index(&state, &movies).unwrap();
        let root = path_text(&fs::canonicalize(&movies).unwrap())
            .unwrap()
            .to_owned();
        let db = connect(&state).unwrap();
        db.execute(
            "UPDATE media_files SET first_indexed_unix=100 WHERE relative_path='Alpha.mp4'",
            [],
        )
        .unwrap();
        fs::write(movies.join("Beta.mkv"), b"later").unwrap();
        index(&state, &movies).unwrap();
        let keys = latest_file_keys(&state).unwrap();
        assert_eq!(
            keys,
            vec![
                (root.clone(), "Beta.mkv".to_owned()),
                (root.clone(), "Alpha.mp4".to_owned())
            ]
        );
        let preserved: i64 = connect(&state)
            .unwrap()
            .query_row(
                "SELECT first_indexed_unix FROM media_files
                 WHERE library_root=?1 AND relative_path='Alpha.mp4'",
                params![root],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, 100);

        fs::remove_file(movies.join("Alpha.mp4")).unwrap();
        assert_eq!(index(&state, &movies).unwrap().stale_removed, 1);
        fs::write(movies.join("Alpha.mp4"), b"new").unwrap();
        index(&state, &movies).unwrap();
        let readded: i64 = connect(&state)
            .unwrap()
            .query_row(
                "SELECT first_indexed_unix FROM media_files WHERE relative_path='Alpha.mp4'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(readded > 100);
    }

    #[test]
    fn v2_catalog_migrates_legacy_last_seen_without_reassigning_on_rescan() {
        let fixture = tempfile::tempdir().unwrap();
        let state = fixture.path().join("state");
        let root = fixture.path().join("Library");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("Legacy.mkv"), b"video").unwrap();
        let canonical = path_text(&fs::canonicalize(&root).unwrap())
            .unwrap()
            .to_owned();
        let path = database_path(&state).unwrap();
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE media_files (
                library_root TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                kind TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL DEFAULT (unixepoch()),
                PRIMARY KEY (library_root, relative_path)
             );
             CREATE TABLE libraries(root TEXT PRIMARY KEY NOT NULL);
             PRAGMA user_version=2;",
        )
        .unwrap();
        db.execute(
            "INSERT INTO media_files(library_root,relative_path,kind,size_bytes,last_seen_at)
             VALUES(?1,'Legacy.mkv','video',5,1234)",
            params![canonical],
        )
        .unwrap();
        db.execute("INSERT INTO libraries(root) VALUES(?1)", params![canonical])
            .unwrap();
        drop(db);
        assert_eq!(latest_file_keys(&state).unwrap().len(), 1);
        let prior: i64 = connect(&state)
            .unwrap()
            .query_row(
                "SELECT first_indexed_unix FROM media_files WHERE relative_path='Legacy.mkv'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(prior, 1234);
        index(&state, &root).unwrap();
        let after: i64 = connect(&state)
            .unwrap()
            .query_row(
                "SELECT first_indexed_unix FROM media_files WHERE relative_path='Legacy.mkv'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, prior);
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
