//! Recognized pinned Jellyfin item/media filters shared across catalog and search.
use axum::http::StatusCode;

pub(crate) fn filter_types(value: Option<&str>) -> Result<Option<Vec<String>>, StatusCode> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = value
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parsed.iter().any(|term| {
        !matches!(
            term.as_str(),
            "Video"
                | "Audio"
                | "Photo"
                | "Folder"
                | "CollectionFolder"
                | "Episode"
                | "Season"
                | "Series"
        )
    }) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    Ok(Some(parsed))
}

pub(crate) fn media_types(value: Option<&str>) -> Result<Option<Vec<String>>, StatusCode> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = value
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parsed
        .iter()
        .any(|term| !matches!(term.as_str(), "Video" | "Audio" | "Photo"))
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    Ok(Some(parsed))
}
