//! Read-only Jellyfin-shaped item queries from Pelagos' private SQLite catalog.
//! This is not yet full BaseItemDto or Jellyfin's library hierarchy.
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    app::AppState,
    artwork, auth_api, catalog, naming,
    query_filters::{filter_types, media_types},
    user_data,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub media_type: Option<String>,
    pub is_folder: bool,
    pub server_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<user_data::UserItemData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tags: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult {
    items: Vec<ItemDto>,
    total_record_count: usize,
    start_index: usize,
}

#[derive(Default, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemQuery {
    start_index: Option<usize>,
    limit: Option<usize>,
    search_term: Option<String>,
    parent_id: Option<String>,
    recursive: Option<bool>,
    include_item_types: Option<String>,
    exclude_item_types: Option<String>,
    media_types: Option<String>,
    is_movie: Option<bool>,
    is_series: Option<bool>,
    is_played: Option<bool>,
    is_favorite: Option<bool>,
    user_id: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    enable_images: Option<bool>,
    enable_user_data: Option<bool>,
    enable_total_record_count: Option<bool>,
    fields: Option<String>,
    image_type_limit: Option<usize>,
    enable_image_types: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items", get(items))
        .route("/Items/Counts", get(item_counts))
        .route("/Items/Root", get(items_root))
        .route("/Users/{user_id}/Items/Root", get(items_root_legacy))
        .route("/Items/{item_id}", get(item))
        .route("/Items/{item_id}/Ancestors", get(ancestors))
        .route("/Users/{user_id}/Items", get(user_items))
        .route("/Users/{user_id}/Items/{item_id}", get(user_item))
        .route("/UserViews", get(user_views))
        .route("/Library/MediaFolders", get(library_media_folders))
        .route("/Library/PhysicalPaths", get(library_physical_paths))
        .route("/Users/{user_id}/Views", get(user_views_legacy))
}

/// Subset of the Jellyfin v12.3 ItemCounts response. Unsupported item
/// classifications remain zero, not guessed from generic Audio/Video.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ItemCounts {
    movie_count: usize,
    series_count: usize,
    episode_count: usize,
    artist_count: usize,
    program_count: usize,
    trailer_count: usize,
    song_count: usize,
    album_count: usize,
    music_video_count: usize,
    box_set_count: usize,
    book_count: usize,
    item_count: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemCountsQuery {
    user_id: Option<String>,
    is_favorite: Option<bool>,
}

async fn item_counts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemCountsQuery>,
) -> Result<Json<ItemCounts>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let mut nodes = accessible_nodes(state.clone(), headers).await?;
    if let Some(favorite) = query.is_favorite {
        let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let ids = nodes.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        let user_id = user.id;
        let user_items =
            tokio::task::spawn_blocking(move || user_data::read_many(&dir, &user_id, &ids))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        nodes = nodes
            .into_iter()
            .zip(user_items)
            .filter_map(|(item, data)| (data.is_favorite == favorite).then_some(item))
            .collect();
    }
    let count = |kind: &str| nodes.iter().filter(|item| item.item_type == kind).count();
    let item_count = nodes
        .iter()
        .filter(|item| {
            matches!(
                item.item_type.as_str(),
                "Movie" | "Episode" | "Video" | "Audio" | "Photo"
            )
        })
        .count();
    Ok(Json(ItemCounts {
        movie_count: count("Movie"),
        series_count: count("Series"),
        episode_count: count("Episode"),
        artist_count: 0,
        program_count: 0,
        trailer_count: 0,
        song_count: 0,
        album_count: 0,
        music_video_count: 0,
        box_set_count: 0,
        book_count: 0,
        item_count,
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AncestorsQuery {
    user_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RootQuery {
    user_id: Option<String>,
}

/// Same virtual root ID across authenticated clients and daemon restarts;
/// unrelated to any physical library path or the caller's personal token.
fn virtual_root_id(server_id: &str) -> String {
    stable_id(server_id, "pelagos:virtual-user-root:v1")
}

fn virtual_root(server_id: &str) -> ItemDto {
    ItemDto {
        id: virtual_root_id(server_id),
        name: "Media".to_owned(),
        item_type: "Folder".to_owned(),
        media_type: None,
        is_folder: true,
        server_id: server_id.to_owned(),
        parent_id: None,
        size: None,
        container: None,
        series_name: None,
        series_id: None,
        season_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        child_count: None,
        user_data: None,
        image_tags: None,
    }
}

async fn root_for_user(
    state: Arc<AppState>,
    headers: HeaderMap,
    query_user: Option<String>,
    path_user: Option<String>,
) -> Result<Json<ItemDto>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query_user.as_deref().is_some_and(|id| id != user.id)
        || path_user.as_deref().is_some_and(|id| id != user.id)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let mut root = virtual_root(state.server_id());
    attach_user_data(&state, headers, std::slice::from_mut(&mut root)).await?;
    Ok(Json(root))
}

async fn items_root(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<RootQuery>,
) -> Result<Json<ItemDto>, StatusCode> {
    root_for_user(state, headers, query.user_id, None).await
}

async fn items_root_legacy(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<ItemDto>, StatusCode> {
    root_for_user(state, headers, None, Some(user_id)).await
}

/// Pinned Jellyfin v12.3 ancestor order is immediate parent first.
/// Resolve only from the authenticated user's accessible graph, so neither
/// ancestor IDs nor artwork leak across ungranted library roots.
async fn ancestors(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<AncestorsQuery>,
) -> Result<Json<Vec<ItemDto>>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let nodes = accessible_nodes(state.clone(), headers.clone()).await?;
    let by_id: HashMap<String, ItemDto> = nodes
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut next = by_id
        .get(&item_id)
        .ok_or(StatusCode::NOT_FOUND)?
        .parent_id
        .clone();
    let mut seen = HashSet::new();
    let mut parents = Vec::new();
    while let Some(id) = next {
        if !seen.insert(id.clone()) {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        let parent = by_id.get(&id).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        next = parent.parent_id.clone();
        parents.push(parent.clone());
    }
    attach_user_data(&state, headers, &mut parents).await?;
    Ok(Json(parents))
}

pub(crate) fn stable_id(root: &str, name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(root.as_bytes());
    digest.update([0]);
    digest.update(name.as_bytes());
    let mut bytes: [u8; 32] = digest.finalize().into();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_slice(&bytes[..16])
        .expect("SHA-256 output is at least 16 bytes")
        .simple()
        .to_string()
}

fn map_item(
    file: catalog::CatalogFile,
    server_id: &str,
    parent_id: String,
    primary_tag: Option<String>,
) -> ItemDto {
    let name = Path::new(&file.relative_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(file.relative_path.clone());
    let episode = if file.kind == "video" {
        naming::episode_hint(Path::new(&file.relative_path))
    } else {
        None
    };
    let movie = if file.kind == "video" && episode.is_none() {
        naming::movie_hint(Path::new(&file.relative_path))
    } else {
        None
    };
    let (item_type, media_type) = match file.kind.as_str() {
        "video" if episode.is_some() => ("Episode", "Video"),
        "video" if movie.is_some() => ("Movie", "Video"),
        "video" => ("Video", "Video"),
        "audio" => ("Audio", "Audio"),
        "image" => ("Photo", "Photo"),
        _ => ("Unknown", "Unknown"),
    };
    let name = episode
        .as_ref()
        .and_then(|hint| hint.episode_title.clone())
        .or_else(|| movie.as_ref().map(|hint| hint.title.clone()))
        .unwrap_or(name);
    let series_id = episode.as_ref().and_then(|_| {
        Path::new(&file.relative_path)
            .parent()
            .and_then(Path::parent)
            .map(|path| stable_id(&file.root, &format!("folder:{}", path.display())))
    });
    let season_id = episode.as_ref().map(|_| parent_id.clone());
    ItemDto {
        id: stable_id(&file.root, &file.relative_path),
        name,
        item_type: item_type.to_owned(),
        media_type: Some(media_type.to_owned()),
        is_folder: false,
        server_id: server_id.to_owned(),
        parent_id: Some(parent_id),
        size: Some(file.size_bytes),
        container: Path::new(&file.relative_path)
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase()),
        series_name: episode.as_ref().map(|hint| hint.series_name.clone()),
        series_id,
        season_id,
        index_number: episode.as_ref().map(|hint| hint.episode),
        parent_index_number: episode.as_ref().map(|hint| hint.season),
        production_year: movie.map(|hint| hint.production_year),
        child_count: None,
        user_data: None,
        image_tags: primary_tag.map(|tag| [("Primary".to_owned(), tag)].into()),
    }
}

fn root_item(root: &str, server_id: &str, image_tag: Option<String>) -> ItemDto {
    ItemDto {
        id: stable_id(root, ""),
        name: Path::new(root)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Library".to_owned()),
        item_type: "CollectionFolder".to_owned(),
        media_type: None,
        is_folder: true,
        server_id: server_id.to_owned(),
        parent_id: Some(virtual_root_id(server_id)),
        size: None,
        container: None,
        series_name: None,
        series_id: None,
        season_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        child_count: None,
        user_data: None,
        image_tags: image_tag.map(|tag| [("Primary".to_owned(), tag)].into()),
    }
}

/// Build a virtual directory hierarchy from indexed filenames. No filesystem writes,
/// metadata guesses, or symlink traversal occur here.
fn catalog_nodes(
    files: Vec<catalog::CatalogFile>,
    roots: Vec<String>,
    server_id: &str,
) -> Vec<ItemDto> {
    let mut result = vec![virtual_root(server_id)];
    let mut seen = HashSet::new();
    let mut series_folders = HashSet::new();
    let mut season_folders = HashMap::new();
    for file in &files {
        if file.kind != "video" {
            continue;
        }
        let path = Path::new(&file.relative_path);
        let Some(hint) = naming::episode_hint(path) else {
            continue;
        };
        let Some(season_dir) = path.parent() else {
            continue;
        };
        let Some(series_dir) = season_dir.parent() else {
            continue;
        };
        series_folders.insert((file.root.clone(), series_dir.to_path_buf()));
        season_folders.insert((file.root.clone(), season_dir.to_path_buf()), hint.season);
    }
    let indexed_images = files
        .iter()
        .filter(|f| f.kind == "image")
        .cloned()
        .collect::<Vec<_>>();
    let media_sidecars = artwork::sidecar_keys(&files);
    for root in roots {
        let root_id = stable_id(&root, "");
        if seen.insert(root_id) {
            let image_tag = artwork::primary_for_folder(&indexed_images, &root, Path::new(""))
                .and_then(artwork::image_tag);
            result.push(root_item(&root, server_id, image_tag));
        }
    }
    for file in files {
        if file.kind == "image"
            && media_sidecars.contains(&(file.root.clone(), file.relative_path.clone()))
        {
            continue;
        }
        let root = file.root.clone();
        let root_id = stable_id(&root, "");
        if seen.insert(root_id.clone()) {
            let image_tag = artwork::primary_for_folder(&indexed_images, &root, Path::new(""))
                .and_then(artwork::image_tag);
            result.push(root_item(&root, server_id, image_tag));
        }
        let mut parent_id = root_id;
        let mut folder = PathBuf::new();
        if let Some(directory) = Path::new(&file.relative_path).parent() {
            for component in directory.components() {
                folder.push(component.as_os_str());
                let path = folder.to_string_lossy();
                let folder_id = stable_id(&root, &format!("folder:{path}"));
                if seen.insert(folder_id.clone()) {
                    let index_number = season_folders.get(&(root.clone(), folder.clone())).copied();
                    let folder_type = if index_number.is_some() {
                        "Season"
                    } else if series_folders.contains(&(root.clone(), folder.clone())) {
                        "Series"
                    } else {
                        "Folder"
                    };
                    result.push(ItemDto {
                        id: folder_id.clone(),
                        name: component.as_os_str().to_string_lossy().into_owned(),
                        item_type: folder_type.to_owned(),
                        media_type: None,
                        is_folder: true,
                        server_id: server_id.to_owned(),
                        parent_id: Some(parent_id.clone()),
                        size: None,
                        container: None,
                        series_name: None,
                        series_id: index_number.and_then(|_| {
                            folder.parent().map(|series| {
                                stable_id(&root, &format!("folder:{}", series.display()))
                            })
                        }),
                        season_id: None,
                        index_number,
                        parent_index_number: None,
                        production_year: None,
                        child_count: None,
                        user_data: None,
                        image_tags: artwork::primary_for_folder(&indexed_images, &root, &folder)
                            .and_then(artwork::image_tag)
                            .map(|tag| [("Primary".to_owned(), tag)].into()),
                    });
                }
                parent_id = folder_id;
            }
        }
        let primary =
            artwork::primary_for_file(&indexed_images, &file).and_then(artwork::image_tag);
        result.push(map_item(file, server_id, parent_id, primary));
    }
    result
}

fn is_descendant(id: &str, parent: &str, parents: &HashMap<String, Option<String>>) -> bool {
    let mut current = parents.get(id).and_then(|p| p.as_deref());
    for _ in 0..1024 {
        match current {
            Some(candidate) if candidate == parent => return true,
            Some(candidate) => current = parents.get(candidate).and_then(|p| p.as_deref()),
            None => return false,
        }
    }
    false
}

/// One shared permission boundary for item browsing and Jellyfin search.
/// No item may become discoverable merely because a different API calls it.
pub(crate) async fn accessible_nodes(
    state: Arc<AppState>,
    headers: HeaderMap,
) -> Result<Vec<ItemDto>, StatusCode> {
    let (state_dir, allowed) = auth_api::library_scope(state.clone(), headers).await?;
    let server_id = state.server_id().to_owned();
    let (mut files, mut roots) = tokio::task::spawn_blocking(move || {
        let files = catalog::all_files(&state_dir)?;
        let roots = catalog::library_roots(&state_dir)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((files, roots))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(granted) = allowed {
        files.retain(|file| granted.contains(&file.root));
        roots.retain(|root| granted.contains(root));
    }
    Ok(catalog_nodes(files, roots, &server_id))
}

async fn query_items(
    state: Arc<AppState>,
    headers: HeaderMap,
    query: ItemQuery,
) -> Result<Json<QueryResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if query.user_id.as_deref().is_some_and(|id| id != user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    if query
        .sort_by
        .as_deref()
        .is_some_and(|key| key != "SortName")
        || query
            .sort_order
            .as_deref()
            .is_some_and(|order| !matches!(order, "Ascending" | "Descending"))
        || query.enable_total_record_count == Some(false)
        || query.fields.is_some()
        || query.image_type_limit.is_some()
        || query.enable_image_types.is_some()
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let mut all = accessible_nodes(state.clone(), headers.clone()).await?;
    match query.parent_id.as_deref() {
        Some(parent) => {
            let parents: HashMap<String, Option<String>> = all
                .iter()
                .map(|item| (item.id.clone(), item.parent_id.clone()))
                .collect();
            if !all.iter().any(|item| item.id == parent && item.is_folder) {
                return Err(StatusCode::NOT_FOUND);
            }
            let recursive = query.recursive.unwrap_or(false);
            all.retain(|item| {
                if recursive {
                    !item.is_folder && is_descendant(&item.id, parent, &parents)
                } else {
                    item.parent_id.as_deref() == Some(parent)
                }
            });
        }
        None => all.retain(|item| {
            !item.is_folder
                || (query.is_series == Some(true) && item.item_type == "Series")
                || query
                    .include_item_types
                    .as_deref()
                    .is_some_and(|types| types.split(',').any(|kind| kind.trim() == item.item_type))
        }),
    }
    let included = filter_types(query.include_item_types.as_deref())?;
    let excluded = filter_types(query.exclude_item_types.as_deref())?;
    let media = media_types(query.media_types.as_deref())?;
    if let Some(search) = query.search_term.filter(|s| !s.is_empty()) {
        let search = search.to_lowercase();
        all.retain(|item| item.name.to_lowercase().contains(&search));
    }
    all.retain(|item| {
        included
            .as_ref()
            .is_none_or(|types| types.contains(&item.item_type))
            && excluded
                .as_ref()
                .is_none_or(|types| !types.contains(&item.item_type))
            && media.as_ref().is_none_or(|types| {
                item.media_type
                    .as_ref()
                    .is_some_and(|kind| types.contains(kind))
            })
    });
    if let Some(is_movie) = query.is_movie {
        all.retain(|item| (item.item_type == "Movie") == is_movie);
    }
    if let Some(is_series) = query.is_series {
        all.retain(|item| (item.item_type == "Series") == is_series);
    }
    if query.is_played.is_some() || query.is_favorite.is_some() {
        let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
        let user_id = user.id.clone();
        let ids = all.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        let personal =
            tokio::task::spawn_blocking(move || user_data::read_many(&dir, &user_id, &ids))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        all = all
            .into_iter()
            .zip(personal)
            .filter_map(|(item, data)| {
                (query.is_played.is_none_or(|value| data.played == value)
                    && query
                        .is_favorite
                        .is_none_or(|value| data.is_favorite == value))
                .then_some(item)
            })
            .collect();
    }
    all.sort_by(|a, b| {
        let order = if query.sort_by.as_deref() == Some("SortName") {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        } else {
            a.name.cmp(&b.name)
        };
        order.then_with(|| a.id.cmp(&b.id))
    });
    if query.sort_order.as_deref() == Some("Descending") {
        all.reverse();
    }
    let total_record_count = all.len();
    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(200);
    let mut page: Vec<ItemDto> = all.into_iter().skip(start_index).take(limit).collect();
    if query.enable_images == Some(false) {
        for item in &mut page {
            item.image_tags = None;
        }
    }
    if query.enable_user_data != Some(false) {
        attach_user_data(&state, headers, &mut page).await?;
    }
    Ok(Json(QueryResult {
        items: page,
        total_record_count,
        start_index,
    }))
}

async fn items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemQuery>,
) -> Result<Json<QueryResult>, StatusCode> {
    query_items(state, headers, query).await
}

async fn user_items(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
    Query(query): Query<ItemQuery>,
) -> Result<Json<QueryResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if user.id != user_id {
        return Err(StatusCode::FORBIDDEN);
    }
    query_items(state, headers, query).await
}

/// Attach the correct authenticated user's persistent state, not an admin's
/// view of another user's preferences.
async fn attach_user_data(
    state: &Arc<AppState>,
    headers: HeaderMap,
    items: &mut [ItemDto],
) -> Result<(), StatusCode> {
    if items.is_empty() {
        return Ok(());
    }
    let (user, _) = auth_api::authenticated(state.clone(), headers).await?;
    let state_dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let ids = items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
    let personal =
        tokio::task::spawn_blocking(move || user_data::read_many(&state_dir, &user.id, &ids))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for (item, data) in items.iter_mut().zip(personal) {
        item.user_data = Some(data);
    }
    Ok(())
}

async fn item(
    State(state): State<Arc<AppState>>,
    RoutePath(item_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<ItemDto>, StatusCode> {
    let mut found = accessible_nodes(state.clone(), headers.clone())
        .await?
        .into_iter()
        .find(|item| item.id == item_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    attach_user_data(&state, headers, std::slice::from_mut(&mut found)).await?;
    Ok(Json(found))
}

async fn user_item(
    State(state): State<Arc<AppState>>,
    RoutePath((user_id, item_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ItemDto>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if user.id != user_id {
        return Err(StatusCode::FORBIDDEN);
    }
    item(State(state), RoutePath(item_id), headers).await
}

/// Pinned Jellyfin v12.3 administrator-only library root paths.
/// Unlike ordinary user DTOs, this explicitly privileged endpoint returns
/// configured on-disk source paths only after administrator authentication.
async fn library_physical_paths(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers).await?;
    if !user.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    let dir = state.auth_dir().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let mut roots = tokio::task::spawn_blocking(move || catalog::library_roots(&dir))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    roots.sort();
    roots.dedup();
    Ok(Json(roots))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaFoldersQuery {
    is_hidden: Option<bool>,
}

/// Administrator-only subset of Jellyfin v12.3 /Library/MediaFolders.
/// Pelagos currently has no hidden/disabled library configuration; the
/// indexed roots are visible and this endpoint never exposes physical paths.
async fn library_media_folders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MediaFoldersQuery>,
) -> Result<Json<QueryResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if !user.policy.is_administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    let mut items = if query.is_hidden == Some(true) {
        Vec::new()
    } else {
        accessible_nodes(state, headers)
            .await?
            .into_iter()
            .filter(|item| item.item_type == "CollectionFolder")
            .collect::<Vec<_>>()
    };
    items.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(Json(QueryResult {
        total_record_count: items.len(),
        items,
        start_index: 0,
    }))
}

async fn user_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<QueryResult>, StatusCode> {
    let items = accessible_nodes(state, headers)
        .await?
        .into_iter()
        .filter(|item| item.item_type == "CollectionFolder")
        .collect::<Vec<_>>();
    Ok(Json(QueryResult {
        total_record_count: items.len(),
        items,
        start_index: 0,
    }))
}

async fn user_views_legacy(
    State(state): State<Arc<AppState>>,
    RoutePath(user_id): RoutePath<String>,
    headers: HeaderMap,
) -> Result<Json<QueryResult>, StatusCode> {
    let (user, _) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    if user.id != user_id {
        return Err(StatusCode::FORBIDDEN);
    }
    user_views(State(state), headers).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_folder_hierarchy_is_stable_and_selectable() {
        let files = vec![
            catalog::CatalogFile {
                root: "/media/movies".to_owned(),
                relative_path: "Series/Season 1/Episode.mkv".to_owned(),
                kind: "video".to_owned(),
                size_bytes: 7,
            },
            catalog::CatalogFile {
                root: "/media/movies".to_owned(),
                relative_path: "Standalone.mp4".to_owned(),
                kind: "video".to_owned(),
                size_bytes: 5,
            },
        ];
        let tree = catalog_nodes(files, vec!["/media/movies".to_owned()], "server");
        assert_eq!(tree.len(), 6);
        let root = tree
            .iter()
            .find(|item| item.item_type == "CollectionFolder")
            .unwrap();
        let virtual_root = tree.iter().find(|item| item.name == "Media").unwrap();
        assert_eq!(root.parent_id.as_deref(), Some(virtual_root.id.as_str()));
        let series = tree.iter().find(|item| item.name == "Series").unwrap();
        let episode = tree.iter().find(|item| item.name == "Episode").unwrap();
        assert_eq!(series.parent_id.as_deref(), Some(root.id.as_str()));
        assert!(is_descendant(
            &episode.id,
            &root.id,
            &tree
                .iter()
                .map(|item| (item.id.clone(), item.parent_id.clone()))
                .collect()
        ));
    }

    #[test]
    fn deterministic_path_id_with_no_filenames_leaked() {
        let first = stable_id("/media/movies", "A/movie.mp4");
        assert_eq!(first, stable_id("/media/movies", "A/movie.mp4"));
        assert_ne!(first, stable_id("/media/music", "A/movie.mp4"));
        assert_eq!(first.len(), 32);
    }
}
