//! Small, read-only media inventory. This is NOT yet Jellyfin's library model.
use std::{
    io,
    path::{Path, PathBuf},
};

use serde::Serialize;
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct MediaFile {
    /// Relative to the requested library root: no absolute filenames in output.
    pub path: PathBuf,
    pub size_bytes: u64,
    pub kind: MediaKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub root: String,
    pub file_count: usize,
    pub files: Vec<MediaFile>,
}

fn media_kind(path: &Path) -> Option<MediaKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "mkv" | "mp4" | "m4v" | "mov" | "avi" | "webm" | "ts" | "mpg" | "mpeg" => {
            Some(MediaKind::Video)
        }
        "mp3" | "flac" | "wav" | "m4a" | "opus" | "ogg" | "aac" => Some(MediaKind::Audio),
        "jpg" | "jpeg" | "png" | "webp" | "gif" => Some(MediaKind::Image),
        _ => None,
    }
}

/// Enumerates supported files in a directory without following symlinks.
///
/// No file is opened or altered; this is an initial inventory, not FFprobe metadata.
pub fn scan(root: &Path) -> io::Result<ScanReport> {
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "library root must be an existing directory",
        ));
    }

    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry.map_err(io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        if let Some(kind) = media_kind(entry.path()) {
            let relative = entry.path().strip_prefix(root).map_err(io::Error::other)?;
            files.push(MediaFile {
                path: relative.to_path_buf(),
                size_bytes: entry.metadata().map_err(io::Error::other)?.len(),
                kind,
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ScanReport {
        root: root.display().to_string(),
        file_count: files.len(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn scanning_is_read_only_and_filters_by_type() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Movies")).unwrap();
        fs::write(dir.path().join("Movies/Example.MP4"), b"synthetic-media").unwrap();
        fs::write(dir.path().join("Movies/notes.txt"), b"notes").unwrap();
        fs::write(dir.path().join("cover.PNG"), b"synthetic-image").unwrap();
        let report = scan(dir.path()).unwrap();
        assert_eq!(report.file_count, 2);
        assert_eq!(report.files[0].path, PathBuf::from("Movies/Example.MP4"));
        assert_eq!(report.files[0].kind, MediaKind::Video);
        assert_eq!(report.files[0].size_bytes, 15);
        assert_eq!(report.files[1].kind, MediaKind::Image);
        assert_eq!(
            fs::read(dir.path().join("Movies/Example.MP4")).unwrap(),
            b"synthetic-media"
        );
    }

    #[test]
    fn invalid_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            scan(&dir.path().join("missing")).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_traversed() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.mp4"), b"not-indexed").unwrap();
        symlink(outside.path(), dir.path().join("linked")).unwrap();
        assert!(scan(dir.path()).unwrap().files.is_empty());
    }
}
