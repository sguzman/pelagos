//! Pelagos-owned, per-user playback flags. No access to Jellyfin's database.
//! This is an initial Jellyfin v12.3 user-data subset, not a complete sync engine.
use std::{collections::{BTreeMap, HashMap}, path::Path, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::{app::AppState, auth_api, catalog_api};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemData {
    pub key: String,
    pub item_id: String,
    pub played: bool,
    pub play_count: i64,
    pub playback_position_ticks: i64,
    pub is_favorite: bool,
    pub played_percentage: Option<f64>,
    pub unplayed_item_count: Option<i64>,
    pub likes: Option<bool>,
    pub rating: Option<f64>,
    pub last_played_date: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum Flag {
    Played(bool),
    Favorite(bool),
}

fn open_db(state_dir: &Path) -> Result<Connection> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(state_dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join("pelagos-user-data.sqlite3");
    let db = Connection::open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    db.execute_batch(
        "PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS item_user_data (
            user_id TEXT NOT NULL,
            item_id TEXT NOT NULL,
            played INTEGER NOT NULL DEFAULT 0 CHECK(played IN (0,1)),
            play_count INTEGER NOT NULL DEFAULT 0 CHECK(play_count >= 0),
            position_ticks INTEGER NOT NULL DEFAULT 0 CHECK(position_ticks >= 0),
            favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0,1)),
            last_played_utc TEXT,
            PRIMARY KEY(user_id, item_id)
         );",
    )?;
    Ok(db)
}

fn user_item(item_id: &str) -> UserItemData {
    UserItemData {
        key: item_id.to_owned(),
        item_id: item_id.to_owned(),
        played: false,
        play_count: 0,
        playback_position_ticks: 0,
        is_favorite: false,
        played_percentage: None,
        unplayed_item_count: None,
        likes: None,
        rating: None,
        last_played_date: None,
    }
}

fn load(db: &Connection, user_id: &str, item_id: &str) -> Result<UserItemData> {
    let row = db
        .query_row(
            "SELECT played,play_count,position_ticks,favorite,last_played_utc
             FROM item_user_data WHERE user_id=?1 AND item_id=?2",
            params![user_id, item_id],
            |r| {
                Ok((
                    r.get::<_, bool>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    let mut data = user_item(item_id);
    if let Some((played, count, ticks, favorite, last_played)) = row {
        data.played = played;
        data.play_count = count;
        data.playback_position_ticks = ticks;
        data.is_favorite = favorite;
        data.last_played_date = last_played;
    }
    Ok(data)
}

pub fn read(state_dir: &Path, user_id: &str, item_id: &str) -> Result<UserItemData> {
    let db = open_db(state_dir)?;
    load(&db, user_id, item_id)
}

/// Read personal item states in bounded indexed queries instead of one
/// query per media item. The result preserves caller order, duplicate IDs and
/// synthesized default states for items with no saved user data.
pub fn read_many(
    state_dir: &Path,
    user_id: &str,
    item_ids: &[String],
) -> Result<Vec<UserItemData>> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    let db = open_db(state_dir)?;
    let mut saved = HashMap::new();
    // Stay below SQLite's historical 999-bind limit, including user_id.
    for chunk in item_ids.chunks(400) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT item_id,played,play_count,position_ticks,favorite,last_played_utc
             FROM item_user_data WHERE user_id=? AND item_id IN ({placeholders})"
        );
        let mut statement = db.prepare(&sql)?;
        let arguments = std::iter::once(user_id).chain(chunk.iter().map(String::as_str));
        let rows = statement.query_map(params_from_iter(arguments), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in rows {
            let (item_id, played, count, ticks, favorite, last_played) = row?;
            let mut data = user_item(&item_id);
            data.played = played;
            data.play_count = count;
            data.playback_position_ticks = ticks;
            data.is_favorite = favorite;
            data.last_played_date = last_played;
            saved.insert(item_id, data);
        }
    }
    Ok(item_ids
        .iter()
        .map(|id| saved.get(id).cloned().unwrap_or_else(|| user_item(id)))
        .collect())
}

fn set_flag(state_dir: &Path, user_id: &str, item_id: &str, flag: Flag) -> Result<UserItemData> {
    let mut db = open_db(state_dir)?;
    let tx = db.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO item_user_data(user_id,item_id) VALUES(?1,?2)",
        params![user_id, item_id],
    )?;
    match flag {
        Flag::Played(true) => {
            tx.execute(
                "UPDATE item_user_data SET
                 play_count = CASE WHEN played = 0 THEN play_count + 1 ELSE play_count END,
                 played = 1,
                 position_ticks = 0,
                 last_played_utc = strftime('%Y-%m-%dT%H:%M:%SZ','now')
                 WHERE user_id=?1 AND item_id=?2",
                params![user_id, item_id],
            )?;
        }
        Flag::Played(false) => {
            tx.execute(
                "UPDATE item_user_data SET played=0,position_ticks=0
                 WHERE user_id=?1 AND item_id=?2",
                params![user_id, item_id],
            )?;
        }
        Flag::Favorite(value) => {
            tx.execute(
                "UPDATE item_user_data SET favorite=?3
                 WHERE user_id=?1 AND item_id=?2",
                params![user_id, item_id, value],
            )?;
        }
    }
    tx.commit()?;
    read(state_dir, user_id, item_id)
}

/// Persist an authorized direct-play resume position without changing the
/// watched/favorite flags. Completion thresholds need full media runtimes.
pub fn save_position(state_dir: &Path, user_id: &str, item_id: &str, ticks: i64) -> Result<()> {
    if ticks < 0 {
        return Err("negative playback ticks are invalid".into());
    }
    let mut db = open_db(state_dir)?;
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO item_user_data(user_id,item_id,position_ticks,last_played_utc)
         VALUES(?1,?2,?3,
                CASE WHEN ?3 > 0 THEN strftime('%Y-%m-%dT%H:%M:%fZ','now')
                     ELSE NULL END)
         ON CONFLICT(user_id,item_id) DO UPDATE
         SET position_ticks=excluded.position_ticks,
             last_played_utc=CASE WHEN excluded.position_ticks > 0
                                  THEN strftime('%Y-%m-%dT%H:%M:%fZ','now')
                                  ELSE item_user_data.last_played_utc END",
        params![user_id, item_id, ticks],
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserDataQuery {
    user_id: Option<String>,
    date_played: Option<String>,
}

/// The pinned Jellyfin UserItems/{itemId}/UserData read contract. This
/// exposes only the caller's private state for an authorized catalog item.
async fn read_authorized(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: String,
    requested_user: Option<String>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if requested_user.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !catalog_api::accessible_nodes(state.clone(), headers)
        .await?
        .iter()
        .any(|item| item.id == item_id)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let user_id = user.id;
    let result = tokio::task::spawn_blocking(move || read(&dir, &user_id, &item_id))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(result))
}

/// Supported subset of the pinned Jellyfin UpdateUserItemDataDto contract.
/// Explicitly reject unsupported non-null fields instead of silently dropping
/// edits made by existing clients.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserDataUpdate {
    played: Option<bool>,
    is_favorite: Option<bool>,
    playback_position_ticks: Option<i64>,
    play_count: Option<i64>,
    key: Option<String>,
    item_id: Option<String>,
    rating: Option<serde_json::Value>,
    played_percentage: Option<serde_json::Value>,
    unplayed_item_count: Option<serde_json::Value>,
    likes: Option<serde_json::Value>,
    last_played_date: Option<String>,
    #[serde(flatten)]
    additional: BTreeMap<String, serde_json::Value>,
}

impl UserDataUpdate {
    fn validate(&self, item_id: &str) -> std::result::Result<(), StatusCode> {
        if self.rating.is_some()
            || self.played_percentage.is_some()
            || self.unplayed_item_count.is_some()
            || self.likes.is_some()
            || self.last_played_date.is_some()
            || self.additional.values().any(|value| !value.is_null())
        {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        if self.key.as_deref().is_some_and(|key| key != item_id)
            || self.item_id.as_deref().is_some_and(|id| id != item_id)
            || self.playback_position_ticks.is_some_and(|ticks| ticks < 0)
            || self.play_count.is_some_and(|count| count < 0)
            || self.played == Some(true)
                && self.playback_position_ticks.is_some_and(|ticks| ticks != 0)
            || self.played == Some(false)
                && self.playback_position_ticks.is_some_and(|ticks| ticks != 0)
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        if self.played.is_none()
            && self.is_favorite.is_none()
            && self.playback_position_ticks.is_none()
            && self.play_count.is_none()
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        Ok(())
    }
}

fn write_update(
    state_dir: &Path,
    user_id: &str,
    item_id: &str,
    update: UserDataUpdate,
) -> Result<UserItemData> {
    let mut db = open_db(state_dir)?;
    let tx = db.transaction()?;
    let prior = load(&tx, user_id, item_id)?;
    let played = update.played.unwrap_or(prior.played);
    let count = update.play_count.unwrap_or_else(|| {
        if played && !prior.played {
            prior.play_count.saturating_add(1)
        } else {
            prior.play_count
        }
    });
    let ticks = if update.played.is_some() {
        0
    } else {
        update
            .playback_position_ticks
            .unwrap_or(prior.playback_position_ticks)
    };
    let favorite = update.is_favorite.unwrap_or(prior.is_favorite);
    let newly_played = played && !prior.played;
    let new_activity = newly_played
        || update
            .playback_position_ticks
            .is_some_and(|ticks| ticks > 0);
    tx.execute(
        "INSERT OR IGNORE INTO item_user_data(user_id,item_id) VALUES(?1,?2)",
        params![user_id, item_id],
    )?;
    tx.execute(
        "UPDATE item_user_data
         SET played=?3,play_count=?4,position_ticks=?5,favorite=?6,
             last_played_utc=CASE WHEN ?7 THEN strftime('%Y-%m-%dT%H:%M:%fZ','now')
                                  ELSE last_played_utc END
         WHERE user_id=?1 AND item_id=?2",
        params![
            user_id,
            item_id,
            played,
            count,
            ticks,
            favorite,
            new_activity
        ],
    )?;
    tx.commit()?;
    read(state_dir, user_id, item_id)
}

async fn update_authorized(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: String,
    requested_user: Option<String>,
    update: UserDataUpdate,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if requested_user.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !catalog_api::accessible_nodes(state.clone(), headers)
        .await?
        .iter()
        .any(|item| item.id == item_id)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    update.validate(&item_id)?;
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let user_id = user.id;
    let updated =
        tokio::task::spawn_blocking(move || write_update(&dir, &user_id, &item_id, update))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(updated))
}

async fn update_item(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<UserDataReadQuery>,
    Json(update): Json<UserDataUpdate>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    update_authorized(state, headers, item_id, query.user_id, update).await
}

async fn update_item_legacy(
    State(state): State<Arc<AppState>>,
    RoutePath((user_id, item_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Json(update): Json<UserDataUpdate>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    update_authorized(state, headers, item_id, Some(user_id), update).await
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserDataReadQuery {
    user_id: Option<String>,
}

async fn read_item(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<UserDataReadQuery>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    read_authorized(state, headers, item_id, query.user_id).await
}

async fn read_item_legacy(
    State(state): State<Arc<AppState>>,
    RoutePath((user_id, item_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    read_authorized(state, headers, item_id, Some(user_id)).await
}

async fn change(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: String,
    query: UserDataQuery,
    flag: Flag,
    legacy_user: Option<String>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id)
        || legacy_user.as_deref().is_some_and(|id| id != user.id)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if query.date_played.is_some() {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    // Authorize the indexed node before creating persistent personal state.
    if !catalog_api::accessible_nodes(state.clone(), headers)
        .await?
        .iter()
        .any(|item| item.id == item_id)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let user_id = user.id;
    let result = tokio::task::spawn_blocking(move || set_flag(&dir, &user_id, &item_id, flag))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(result))
}

async fn played(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    change(state, headers, item_id, query, Flag::Played(true), None).await
}

async fn unplayed(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    change(state, headers, item_id, query, Flag::Played(false), None).await
}

async fn favorite(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    change(state, headers, item_id, query, Flag::Favorite(true), None).await
}

async fn unfavorite(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    change(state, headers, item_id, query, Flag::Favorite(false), None).await
}

async fn legacy(
    State(state): State<Arc<AppState>>,
    RoutePath((user_id, item_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<UserDataQuery>,
    operation: Flag,
) -> std::result::Result<Json<UserItemData>, StatusCode> {
    change(state, headers, item_id, query, operation, Some(user_id)).await
}

macro_rules! legacy_handler {
    ($name:ident, $flag:expr) => {
        async fn $name(
            State(state): State<Arc<AppState>>,
            RoutePath((user_id, item_id)): RoutePath<(String, String)>,
            headers: HeaderMap,
            Query(query): Query<UserDataQuery>,
        ) -> std::result::Result<Json<UserItemData>, StatusCode> {
            legacy(
                State(state),
                RoutePath((user_id, item_id)),
                headers,
                Query(query),
                $flag,
            )
            .await
        }
    };
}
legacy_handler!(played_legacy, Flag::Played(true));
legacy_handler!(unplayed_legacy, Flag::Played(false));
legacy_handler!(favorite_legacy, Flag::Favorite(true));
legacy_handler!(unfavorite_legacy, Flag::Favorite(false));

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/UserItems/{item_id}/UserData",
            get(read_item).post(update_item),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/UserData",
            get(read_item_legacy).post(update_item_legacy),
        )
        .route("/UserPlayedItems/{item_id}", post(played).delete(unplayed))
        .route(
            "/Users/{user_id}/PlayedItems/{item_id}",
            post(played_legacy).delete(unplayed_legacy),
        )
        .route(
            "/UserFavoriteItems/{item_id}",
            post(favorite).delete(unfavorite),
        )
        .route(
            "/Users/{user_id}/FavoriteItems/{item_id}",
            post(favorite_legacy).delete(unfavorite_legacy),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_position_persists_without_marking_watched() {
        let dir = tempfile::tempdir().unwrap();
        save_position(dir.path(), "user", "video", 12_340_000).unwrap();
        assert_eq!(
            read(dir.path(), "user", "video")
                .unwrap()
                .playback_position_ticks,
            12_340_000
        );
        assert!(!read(dir.path(), "user", "video").unwrap().played);
        assert_eq!(
            read(dir.path(), "other", "video")
                .unwrap()
                .playback_position_ticks,
            0
        );
        assert!(save_position(dir.path(), "user", "video", -1).is_err());
        assert_eq!(
            read(dir.path(), "user", "video")
                .unwrap()
                .playback_position_ticks,
            12_340_000
        );
        set_flag(dir.path(), "user", "video", Flag::Played(true)).unwrap();
        assert_eq!(
            read(dir.path(), "user", "video")
                .unwrap()
                .playback_position_ticks,
            0
        );
    }

    #[test]
    fn flags_persist_separately_per_user_and_play_count_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let a = set_flag(dir.path(), "a", "item", Flag::Played(true)).unwrap();
        assert!(a.played);
        assert_eq!(a.play_count, 1);
        assert_eq!(read(dir.path(), "a", "item").unwrap().play_count, 1);
        assert_eq!(
            set_flag(dir.path(), "a", "item", Flag::Played(true))
                .unwrap()
                .play_count,
            1
        );
        assert!(
            set_flag(dir.path(), "a", "item", Flag::Favorite(true))
                .unwrap()
                .is_favorite
        );
        assert_eq!(read(dir.path(), "b", "item").unwrap().play_count, 0);
        assert!(!read(dir.path(), "b", "item").unwrap().is_favorite);
        let b = set_flag(dir.path(), "a", "item", Flag::Played(false)).unwrap();
        assert!(!b.played);
        assert_eq!(b.play_count, 1);
        assert_eq!(
            set_flag(dir.path(), "a", "item", Flag::Played(true))
                .unwrap()
                .play_count,
            2
        );
    }
}
