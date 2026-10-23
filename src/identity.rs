use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Persistent identity across daemon restarts, without sharing Jellyfin's database.
pub fn load_or_create(dir: &Path) -> io::Result<String> {
    fs::create_dir_all(dir)?;
    let file_path = dir.join("server-id");
    if file_path.exists() {
        return read_id(&file_path);
    }

    let id = Uuid::new_v4().to_string();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&file_path) {
        Ok(mut file) => {
            writeln!(file, "{id}")?;
            file.sync_all()?;
            Ok(id)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read_id(&file_path),
        Err(error) => Err(error),
    }
}

fn read_id(path: &Path) -> io::Result<String> {
    let content = fs::read_to_string(path)?;
    let id = Uuid::parse_str(content.trim())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(id.to_string())
}

/// Respect XDG_STATE_HOME, with a per-user fallback. Never touch Jellyfin's state.
pub fn default_state_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(path).join("pelagos");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(home).join(".local/state/pelagos");
    }
    PathBuf::from(".pelagos-state")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create(dir.path()).unwrap();
        assert_eq!(first, load_or_create(dir.path()).unwrap());
        assert!(Uuid::parse_str(&first).is_ok());
    }

    #[test]
    fn malformed_identity_is_rejected_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("server-id"), "invalid").unwrap();
        assert_eq!(
            load_or_create(dir.path()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
