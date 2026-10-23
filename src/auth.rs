//! Local-only credential storage and session tokens. Never reuse Jellyfin's database.
//! Passwords use Argon2id PHC strings, token secrets are random and only SHA-256
//! digests are stored. This is a foundational auth implementation, not policy parity.
use std::{
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Login {
    pub user: User,
    pub access_token: String,
    pub server_id: String,
    pub session_info: Option<serde_json::Value>,
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
    getrandom::fill(&mut bytes)?;
    let salt = SaltString::encode_b64(&bytes)?;
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt)?;
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
        },
    })
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
    let parsed = PasswordHash::new(&stored_hash)?;
    if Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_err()
    {
        return Ok(None);
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let digest = digest_token(&token);
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO pelagos_tokens(digest, user_id) VALUES(?1, ?2)",
        params![digest.as_slice(), id],
    )?;
    let user = tx.query_row(
        "SELECT id,username,is_admin,disabled FROM pelagos_users WHERE id=?1",
        params![id],
        |row| user_from_row(row, server_id),
    )?;
    tx.commit()?;
    Ok(Some(Login {
        user,
        access_token: token,
        server_id: server_id.to_owned(),
        session_info: None,
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
            "SELECT u.id,u.username,u.is_admin,u.disabled FROM pelagos_tokens t
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
