//! Read-only, versioned Pelagos startup configuration. No user media,
//! Jellyfin settings or existing config files are modified by this reader.
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    net::IpAddr,
    path::{Path, PathBuf},
};

use serde::Deserialize;

const MAX_CONFIG_BYTES: u64 = 16 * 1024;
const CURRENT_SCHEMA: u32 = 1;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(default)]
    schema_version: Option<u32>,
    #[serde(default)]
    bind: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    discover: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub bind: String,
    pub port: u16,
    pub discover: bool,
}

/// Preserve historic CLI defaults, even if no configuration file exists.
pub fn default_path_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(root) = xdg.filter(|value| !value.is_empty())
        && Path::new(root).is_absolute()
    {
        return Some(Path::new(root).join("pelagos/config.json"));
    }
    home.filter(|value| !value.is_empty())
        .filter(|value| Path::new(value).is_absolute())
        .map(|root| Path::new(root).join(".config/pelagos/config.json"))
}

/// Missing implicit XDG configuration is fine. An explicitly named file must
/// exist and be valid. Version 0 (unversioned JSON) is migrated only in memory.
pub fn load(explicit: Option<&Path>) -> io::Result<FileConfig> {
    let fallback = default_path_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    );
    let Some(path) = explicit.or(fallback.as_deref()) else {
        return Ok(FileConfig::default());
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if explicit.is_none() && error.kind() == io::ErrorKind::NotFound => {
            return Ok(FileConfig::default());
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Pelagos config exceeds 16 KiB",
        ));
    }
    let config: FileConfig = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if config.schema_version.unwrap_or(0) > CURRENT_SCHEMA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported future Pelagos configuration schema",
        ));
    }
    Ok(config)
}

/// Flags win over file settings. The explicit disable flag can override a
/// file opting into LAN discovery; otherwise discovery defaults to off.
pub fn resolve(
    config: FileConfig,
    bind: Option<String>,
    port: Option<u16>,
    discover: bool,
    no_discover: bool,
) -> io::Result<RuntimeConfig> {
    let bind = bind
        .or(config.bind)
        .unwrap_or_else(|| "127.0.0.1".to_owned());
    let parsed: IpAddr = bind.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "bind address must be an IP literal",
        )
    })?;
    let port = port.or(config.port).unwrap_or(8097);
    let discover = if no_discover {
        false
    } else if discover {
        true
    } else {
        config.discover.unwrap_or(false)
    };
    if discover && parsed.is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "discovery requires an explicit LAN bind",
        ));
    }
    Ok(RuntimeConfig {
        bind,
        port,
        discover,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_lookup_is_absolute_and_defaults_are_loopback() {
        assert_eq!(
            default_path_from(Some(OsStr::new("/config")), Some(OsStr::new("/home/test")),)
                .unwrap(),
            PathBuf::from("/config/pelagos/config.json")
        );
        assert_eq!(
            default_path_from(Some(OsStr::new("relative")), Some(OsStr::new("/home/test")))
                .unwrap(),
            PathBuf::from("/home/test/.config/pelagos/config.json")
        );
        assert_eq!(
            resolve(FileConfig::default(), None, None, false, false).unwrap(),
            RuntimeConfig {
                bind: "127.0.0.1".to_owned(),
                port: 8097,
                discover: false
            }
        );
    }

    #[test]
    fn ipv6_bind_is_validated_and_constructed_without_string_concatenation() {
        let parsed = resolve(
            FileConfig::default(),
            Some("::1".to_owned()),
            Some(8097),
            false,
            false,
        )
        .unwrap();
        let address = std::net::SocketAddr::new(parsed.bind.parse().unwrap(), parsed.port);
        assert_eq!(address.to_string(), "[::1]:8097");
    }

    #[test]
    fn legacy_json_is_migrated_in_memory_and_cli_override_is_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"bind":"0.0.0.0","port":8098,"discover":true}"#).unwrap();
        let config = load(Some(&path)).unwrap();
        assert_eq!(
            resolve(config, None, None, false, false).unwrap(),
            RuntimeConfig {
                bind: "0.0.0.0".to_owned(),
                port: 8098,
                discover: true
            }
        );
        let config = load(Some(&path)).unwrap();
        let effective = resolve(
            config,
            Some("127.0.0.1".to_owned()),
            Some(8099),
            false,
            true,
        )
        .unwrap();
        assert_eq!(effective.bind, "127.0.0.1");
        assert_eq!(effective.port, 8099);
        assert!(!effective.discover);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"port\":8098")
        );
    }

    #[test]
    fn invalid_inputs_fail_closed_without_rewriting_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for body in [
            r#"{"schema_version":2}"#,
            r#"{"schema_version":1,"unexpected":true}"#,
            r#"{"schema_version":1,"port":"8097"}"#,
            "{",
        ] {
            std::fs::write(&path, body).unwrap();
            assert!(load(Some(&path)).is_err(), "{body}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        }
        std::fs::write(&path, "x".repeat(16 * 1024 + 1)).unwrap();
        assert!(load(Some(&path)).is_err());
        assert!(load(Some(&dir.path().join("absent.json"))).is_err());
        assert!(
            resolve(
                FileConfig::default(),
                Some("bad.host".to_owned()),
                None,
                false,
                false
            )
            .is_err()
        );
        assert!(resolve(FileConfig::default(), None, None, true, false).is_err());
    }
}
