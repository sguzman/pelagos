//! Local-only credential storage and session tokens. Never reuse Jellyfin's database.
//! Passwords use Argon2id PHC strings, token secrets are random and only SHA-256
//! digests are stored. This is a foundational auth implementation, not policy parity.
use std::{
    collections::HashSet,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const DB_NAME: &str = "pelagos-auth.sqlite3";
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    pub id: String,
    pub name: String,
    pub server_id: String,
    pub policy: UserPolicy,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserPolicy {
    pub is_administrator: bool,
    pub is_disabled: bool,
    pub enable_content_downloading: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ClientMetadata {
    pub client: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub application_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionInfo {
    pub id: String,
    pub user_id: String,
    pub user_name: String,
    pub client: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub application_version: Option<String>,
    pub server_id: String,
    pub is_active: bool,
    pub supports_media_control: bool,
    pub supports_remote_control: bool,
    pub playable_media_types: Vec<String>,
    pub supported_commands: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Login {
    pub user: User,
    pub access_token: String,
    pub server_id: String,
    pub session_info: SessionInfo,
}

fn state_db(state_dir: &Path) -> Result<PathBuf> {
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

fn database(state_dir: &Path) -> Result<Connection> {
    let path = state_db(state_dir)?;
    let db = Connection::open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    db.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS pelagos_users(
             id TEXT PRIMARY KEY NOT NULL,
             username TEXT NOT NULL,
             username_key TEXT NOT NULL UNIQUE,
             password_hash TEXT NOT NULL,
             is_admin INTEGER NOT NULL CHECK(is_admin IN (0,1)),
             disabled INTEGER NOT NULL DEFAULT 0 CHECK(disabled IN (0,1))
         );
         CREATE TABLE IF NOT EXISTS pelagos_tokens(
             digest BLOB PRIMARY KEY NOT NULL,
             user_id TEXT NOT NULL REFERENCES pelagos_users(id) ON DELETE CASCADE,
             created_unix INTEGER NOT NULL DEFAULT (unixepoch())
         );
         CREATE TABLE IF NOT EXISTS pelagos_sessions(
             id TEXT PRIMARY KEY NOT NULL,
             digest BLOB NOT NULL UNIQUE REFERENCES pelagos_tokens(digest) ON DELETE CASCADE,
             client TEXT,
             device_id TEXT,
             device_name TEXT,
             application_version TEXT
         );
         CREATE TABLE IF NOT EXISTS pelagos_library_access(
             user_id TEXT NOT NULL REFERENCES pelagos_users(id) ON DELETE CASCADE,
             root TEXT NOT NULL,
             PRIMARY KEY(user_id, root)
         );
         CREATE TABLE IF NOT EXISTS pelagos_download_grants(
             user_id TEXT PRIMARY KEY NOT NULL REFERENCES pelagos_users(id) ON DELETE CASCADE
         );",
    )?;
    Ok(db)
}

fn valid_username(name: &str) -> bool {
    name.len() <= 128
        && !name.is_empty()
        && name.trim() == name
        && !name.chars().any(char::is_control)
}

fn validate_password(password: &str) -> Result<()> {
    if password.len() < 12 || password.len() > MAX_PASSWORD_BYTES {
        return Err("password must be 12 to 1024 bytes".into());
    }
    Ok(())
}

fn password_hash(password: &str) -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    let salt =
        SaltString::encode_b64(&bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(hash.to_string())
}

fn user_from_row(row: &rusqlite::Row<'_>, server_id: &str) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?,
        name: row.get(1)?,
        server_id: server_id.to_owned(),
        policy: UserPolicy {
            is_administrator: row.get::<_, bool>(2)?,
            is_disabled: row.get::<_, bool>(3)?,
            enable_content_downloading: row.get::<_, bool>(4)?,
        },
    })
}

/// Privileged list of minimal account DTOs. Never select credential hashes,
/// raw tokens or other secrets for public serialization.
pub fn list_users(state_dir: &Path, server_id: &str) -> Result<Vec<User>> {
    let db = database(state_dir)?;
    let mut stmt = db.prepare(
        "SELECT u.id,u.username,u.is_admin,u.disabled,
                (u.is_admin=1 OR EXISTS(
                   SELECT 1 FROM pelagos_download_grants d WHERE d.user_id=u.id
                ))
         FROM pelagos_users u ORDER BY u.username_key, u.id",
    )?;
    let rows = stmt.query_map([], |row| user_from_row(row, server_id))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Bootstrap through the CLI; no anonymous HTTP route may create an administrator.
pub fn create_user(state_dir: &Path, name: &str, password: &str) -> Result<User> {
    if !valid_username(name) {
        return Err("username must be 1-128 bytes, trimmed, without controls".into());
    }
    validate_password(password)?;
    let hash = password_hash(password)?;
    let mut db = database(state_dir)?;
    let tx = db.transaction()?;
    let existing: i64 = tx.query_row("SELECT COUNT(*) FROM pelagos_users", [], |r| r.get(0))?;
    let id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO pelagos_users(id, username, username_key, password_hash, is_admin)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![id, name, name.to_lowercase(), hash, existing == 0],
    )?;
    tx.commit()?;
    Ok(User {
        id,
        name: name.to_owned(),
        server_id: String::new(),
        policy: UserPolicy {
            is_administrator: existing == 0,
            is_disabled: false,
            enable_content_downloading: existing == 0,
        },
    })
}

fn digest_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

pub fn authenticate(
    state_dir: &Path,
    name: &str,
    password: &str,
    server_id: &str,
) -> Result<Option<Login>> {
    authenticate_with_client(
        state_dir,
        name,
        password,
        server_id,
        ClientMetadata::default(),
    )
}

pub fn authenticate_with_client(
    state_dir: &Path,
    name: &str,
    password: &str,
    server_id: &str,
    metadata: ClientMetadata,
) -> Result<Option<Login>> {
    if !valid_username(name) || password.len() > MAX_PASSWORD_BYTES {
        return Ok(None);
    }
    let mut db = database(state_dir)?;
    let record: Option<(String, String)> = db
        .query_row(
            "SELECT id, password_hash FROM pelagos_users WHERE username_key=?1 AND disabled=0",
            params![name.to_lowercase()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((id, stored_hash)) = record else {
        return Ok(None);
    };
    let parsed = PasswordHash::new(&stored_hash)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_err()
    {
        return Ok(None);
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let digest = digest_token(&token);
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO pelagos_tokens(digest, user_id) VALUES(?1, ?2)",
        params![digest.as_slice(), id],
    )?;
    let user = tx.query_row(
        "SELECT u.id,u.username,u.is_admin,u.disabled,
                (u.is_admin=1 OR EXISTS(
                   SELECT 1 FROM pelagos_download_grants d WHERE d.user_id=u.id
                ))
         FROM pelagos_users u WHERE u.id=?1",
        params![id],
        |row| user_from_row(row, server_id),
    )?;
    let session_id = Uuid::new_v4().simple().to_string();
    tx.execute(
        "INSERT INTO pelagos_sessions(
            id, digest, client, device_id, device_name, application_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            session_id,
            digest.as_slice(),
            metadata.client,
            metadata.device_id,
            metadata.device_name,
            metadata.application_version,
        ],
    )?;
    tx.commit()?;
    let session_info = SessionInfo {
        id: session_id,
        user_id: user.id.clone(),
        user_name: user.name.clone(),
        client: metadata.client,
        device_id: metadata.device_id,
        device_name: metadata.device_name,
        application_version: metadata.application_version,
        server_id: server_id.to_owned(),
        is_active: true,
        supports_media_control: false,
        supports_remote_control: false,
        playable_media_types: Vec::new(),
        supported_commands: Vec::new(),
    };
    Ok(Some(Login {
        user,
        access_token: token,
        server_id: server_id.to_owned(),
        session_info,
    }))
}

pub fn identify(state_dir: &Path, token: &str, server_id: &str) -> Result<Option<User>> {
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let db = database(state_dir)?;
    let digest = digest_token(token);
    let user = db
        .query_row(
            "SELECT u.id,u.username,u.is_admin,u.disabled,
                (u.is_admin=1 OR EXISTS(
                   SELECT 1 FROM pelagos_download_grants d WHERE d.user_id=u.id
                ))
         FROM pelagos_tokens t
             JOIN pelagos_users u ON u.id=t.user_id
             WHERE t.digest=?1 AND u.disabled=0",
            params![digest.as_slice()],
            |row| user_from_row(row, server_id),
        )
        .optional()?;
    Ok(user)
}

pub fn revoke(state_dir: &Path, token: &str) -> Result<bool> {
    if token.len() != 64 {
        return Ok(false);
    }
    let db = database(state_dir)?;
    let digest = digest_token(token);
    Ok(db.execute(
        "DELETE FROM pelagos_tokens WHERE digest=?1",
        params![digest.as_slice()],
    )? != 0)
}

/// Return currently valid session metadata, scoped to the requesting user.
pub fn list_sessions(
    state_dir: &Path,
    requester: &User,
    server_id: &str,
    device_id: Option<&str>,
) -> Result<Vec<SessionInfo>> {
    let db = database(state_dir)?;
    let mut statement = db.prepare(
        "SELECT s.id, u.id, u.username, s.client, s.device_id, s.device_name,
                s.application_version
         FROM pelagos_sessions s
         JOIN pelagos_tokens t ON t.digest=s.digest
         JOIN pelagos_users u ON u.id=t.user_id
         WHERE u.disabled=0
           AND (?1=1 OR u.id=?2)
           AND (?3 IS NULL OR s.device_id=?3)
         ORDER BY s.id",
    )?;
    let rows = statement.query_map(
        params![requester.policy.is_administrator, requester.id, device_id],
        |row| {
            Ok(SessionInfo {
                id: row.get(0)?,
                user_id: row.get(1)?,
                user_name: row.get(2)?,
                client: row.get(3)?,
                device_id: row.get(4)?,
                device_name: row.get(5)?,
                application_version: row.get(6)?,
                server_id: server_id.to_owned(),
                is_active: true,
                supports_media_control: false,
                supports_remote_control: false,
                playable_media_types: Vec::new(),
                supported_commands: Vec::new(),
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Persist a separate download permission. A readable library alone never
/// authorizes downloading; the administrator role always retains its implicit
/// local download grant. Changes apply to already-issued tokens immediately.
pub fn set_content_download_permission(
    state_dir: &Path,
    username: &str,
    enabled: bool,
) -> Result<bool> {
    if !valid_username(username) {
        return Err("invalid username".into());
    }
    let db = database(state_dir)?;
    let (user_id, is_admin): (String, bool) = db
        .query_row(
            "SELECT id,is_admin FROM pelagos_users WHERE username_key=?1 AND disabled=0",
            params![username.to_lowercase()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or("user does not exist or is disabled")?;
    if is_admin {
        if !enabled {
            return Err("administrator's implicit download permission cannot be revoked".into());
        }
        return Ok(false);
    }
    let changed = if enabled {
        db.execute(
            "INSERT OR IGNORE INTO pelagos_download_grants(user_id) VALUES (?1)",
            params![user_id],
        )?
    } else {
        db.execute(
            "DELETE FROM pelagos_download_grants WHERE user_id=?1",
            params![user_id],
        )?
    };
    Ok(changed != 0)
}

/// Local CLI administration: grant one explicitly named user read access to one
/// canonical library root. A newly created non-admin account has no library access.
pub fn grant_library(state_dir: &Path, username: &str, root: &Path) -> Result<bool> {
    let root = fs::canonicalize(root)?;
    if !root.is_dir() {
        return Err("library permission target must be a directory".into());
    }
    let name = root
        .to_str()
        .ok_or("library permission root must be UTF-8")?;
    let db = database(state_dir)?;
    let user_id: String = db
        .query_row(
            "SELECT id FROM pelagos_users WHERE username_key=?1 AND disabled=0",
            params![username.to_lowercase()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or("user does not exist or is disabled")?;
    let inserted = db.execute(
        "INSERT OR IGNORE INTO pelagos_library_access(user_id, root) VALUES (?1, ?2)",
        params![user_id, name],
    )?;
    Ok(inserted != 0)
}

/// Local CLI administration: revoke an existing grant. Does not alter media.
pub fn revoke_library(state_dir: &Path, username: &str, root: &Path) -> Result<bool> {
    let root = fs::canonicalize(root)?;
    let name = root
        .to_str()
        .ok_or("library permission root must be UTF-8")?;
    let db = database(state_dir)?;
    Ok(db.execute(
        "DELETE FROM pelagos_library_access
         WHERE user_id=(SELECT id FROM pelagos_users WHERE username_key=?1) AND root=?2",
        params![username.to_lowercase(), name],
    )? != 0)
}

/// None means an administrator may access every indexed root; Some(set) gives
/// the exact canonical roots explicitly permitted to a non-administrator.
pub fn visible_library_roots(state_dir: &Path, user: &User) -> Result<Option<HashSet<String>>> {
    if user.policy.is_disabled {
        return Ok(Some(HashSet::new()));
    }
    if user.policy.is_administrator {
        return Ok(None);
    }
    let db = database(state_dir)?;
    let mut stmt =
        db.prepare("SELECT root FROM pelagos_library_access WHERE user_id=?1 ORDER BY root")?;
    let results = stmt.query_map(params![user.id], |row| row.get(0))?;
    Ok(Some(results.collect::<rusqlite::Result<HashSet<_>>>()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_admin_only_for_first_user_and_persists_password_hash() {
        let dir = tempfile::tempdir().unwrap();
        let admin = create_user(dir.path(), "Admin", "very-strong-test-passphrase").unwrap();
        assert!(admin.policy.is_administrator);
        let second = create_user(dir.path(), "viewer", "another-long-test-passphrase").unwrap();
        assert!(!second.policy.is_administrator);
        assert!(create_user(dir.path(), "admin", "another-long-test-passphrase").is_err());
        let db = database(dir.path()).unwrap();
        let saved: String = db
            .query_row(
                "SELECT password_hash FROM pelagos_users WHERE id=?1",
                params![admin.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(saved.starts_with("$argon2id$"));
        assert!(!saved.contains("very-strong-test-passphrase"));
    }

    #[test]
    fn login_token_revoke_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        create_user(dir.path(), "viewer", "secure-test-passphrase").unwrap();
        assert!(
            authenticate(dir.path(), "viewer", "wrong-password", "server")
                .unwrap()
                .is_none()
        );
        let session = authenticate(dir.path(), "Viewer", "secure-test-passphrase", "server")
            .unwrap()
            .unwrap();
        assert_eq!(session.server_id, "server");
        assert_eq!(session.access_token.len(), 64);
        assert_eq!(
            identify(dir.path(), &session.access_token, "server")
                .unwrap()
                .unwrap()
                .name,
            "viewer"
        );
        assert!(identify(dir.path(), "invalid", "server").unwrap().is_none());
        let db = database(dir.path()).unwrap();
        let raw: Vec<u8> = db
            .query_row("SELECT digest FROM pelagos_tokens", [], |row| row.get(0))
            .unwrap();
        assert_ne!(raw, session.access_token.as_bytes());
        assert!(revoke(dir.path(), &session.access_token).unwrap());
        assert!(
            identify(dir.path(), &session.access_token, "server")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn clients_receive_persistent_session_metadata_with_token_revocation() {
        let dir = tempfile::tempdir().unwrap();
        create_user(dir.path(), "viewer", "secure-test-passphrase").unwrap();
        let metadata = ClientMetadata {
            client: Some("Jellyfin Android TV".to_owned()),
            device_id: Some("tv-device-1".to_owned()),
            device_name: Some("Living Room".to_owned()),
            application_version: Some("1.0.0".to_owned()),
        };
        let result = authenticate_with_client(
            dir.path(),
            "viewer",
            "secure-test-passphrase",
            "server",
            metadata,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.session_info.client.as_deref(),
            Some("Jellyfin Android TV")
        );
        assert_eq!(
            result.session_info.device_id.as_deref(),
            Some("tv-device-1")
        );
        assert_eq!(result.session_info.user_name, "viewer");
        assert_eq!(result.session_info.user_id, result.user.id);
        assert!(!result.session_info.supports_remote_control);
        let db = database(dir.path()).unwrap();
        let sessions: i64 = db
            .query_row("SELECT COUNT(*) FROM pelagos_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1);
        assert!(revoke(dir.path(), &result.access_token).unwrap());
        let sessions: i64 = db
            .query_row("SELECT COUNT(*) FROM pelagos_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 0);
    }

    #[test]
    fn session_listing_is_user_scoped_admin_visible_and_revocation_aware() {
        let dir = tempfile::tempdir().unwrap();
        let admin_user = create_user(dir.path(), "admin", "admin-test-passphrase").unwrap();
        let viewer_user = create_user(dir.path(), "viewer", "viewer-test-passphrase").unwrap();
        let admin = authenticate(dir.path(), "admin", "admin-test-passphrase", "server")
            .unwrap()
            .unwrap();
        let viewer = authenticate_with_client(
            dir.path(),
            "viewer",
            "viewer-test-passphrase",
            "server",
            ClientMetadata {
                device_id: Some("tv-1".to_owned()),
                ..ClientMetadata::default()
            },
        )
        .unwrap()
        .unwrap();
        let mine = list_sessions(dir.path(), &viewer_user, "server", None).unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].user_id, viewer_user.id);
        assert_eq!(
            list_sessions(dir.path(), &admin_user, "server", None)
                .unwrap()
                .len(),
            2
        );
        let filtered = list_sessions(dir.path(), &admin_user, "server", Some("tv-1")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, viewer.session_info.id);
        assert_eq!(
            list_sessions(dir.path(), &viewer_user, "server", Some("unknown"))
                .unwrap()
                .len(),
            0
        );
        assert!(revoke(dir.path(), &viewer.access_token).unwrap());
        assert_eq!(
            list_sessions(dir.path(), &admin_user, "server", None)
                .unwrap()
                .len(),
            1
        );
        assert!(revoke(dir.path(), &admin.access_token).unwrap());
    }

    #[test]
    fn library_grants_are_deny_by_default_and_can_be_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("Library");
        fs::create_dir(&media).unwrap();
        let admin = create_user(dir.path(), "admin", "admin-test-passphrase").unwrap();
        let viewer = create_user(dir.path(), "viewer", "viewer-test-passphrase").unwrap();
        assert!(visible_library_roots(dir.path(), &admin).unwrap().is_none());
        assert!(
            visible_library_roots(dir.path(), &viewer)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(grant_library(dir.path(), "viewer", &media).unwrap());
        assert!(!grant_library(dir.path(), "viewer", &media).unwrap());
        let canonical = fs::canonicalize(&media).unwrap();
        assert!(
            visible_library_roots(dir.path(), &viewer)
                .unwrap()
                .unwrap()
                .contains(canonical.to_str().unwrap())
        );
        assert!(!grant_library(dir.path(), "unknown", &media).is_ok());
        assert!(revoke_library(dir.path(), "viewer", &media).unwrap());
        assert!(
            visible_library_roots(dir.path(), &viewer)
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_names_and_weak_passwords_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(create_user(dir.path(), "../admin", "short").is_err());
        assert!(create_user(dir.path(), " user ", "secure-test-passphrase").is_err());
        assert!(create_user(dir.path(), "user", "short").is_err());
        assert!(create_user(dir.path(), "user", "secure-test-passphrase").is_ok());
    }

    #[test]
    fn disabled_users_cannot_login_or_use_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let user = create_user(dir.path(), "viewer", "secure-test-passphrase").unwrap();
        let session = authenticate(dir.path(), "viewer", "secure-test-passphrase", "server")
            .unwrap()
            .unwrap();
        let db = database(dir.path()).unwrap();
        db.execute(
            "UPDATE pelagos_users SET disabled=1 WHERE id=?1",
            params![user.id],
        )
        .unwrap();
        assert!(
            authenticate(dir.path(), "viewer", "secure-test-passphrase", "server")
                .unwrap()
                .is_none()
        );
        assert!(
            identify(dir.path(), &session.access_token, "server")
                .unwrap()
                .is_none()
        );
    }
}
