//! Authorized HLS media playlist and MPEG-TS segment delivery, including
//! token, source/sidecar revision, byte-range and HEAD revalidation.
use super::*;
use crate::streaming;
use axum::http::header::{ACCEPT_RANGES, CONTENT_RANGE, ETAG, IF_RANGE, RANGE};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

pub(super) async fn serve_segment(
    state: Arc<AppState>,
    item: String,
    playlist_id: String,
    name: String,
    headers: HeaderMap,
    expected_kind: &'static str,
    head_only: bool,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let publication = state
        .hls_registry()
        .lookup(&playlist_id, &item, expected_kind, &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    // Reauthorize every segment, including library grants and confined source.
    let (_, scope) = auth_api::library_scope(state.clone(), headers.clone()).await?;
    let (source, kind) = playback::media_file(&state, item, scope).await?;
    if kind != expected_kind || source != publication.source {
        return Err(StatusCode::NOT_FOUND);
    }
    // A published HLS segment must not survive replacement or modification
    // of the source file. Regeneration will create a new publication key.
    let metadata = tokio::fs::metadata(&source)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if metadata.len() != publication.source_size
        || metadata.modified().ok() != publication.source_modified
    {
        return Err(StatusCode::NOT_FOUND);
    }
    if let Some(sidecar) = publication.sidecar.as_ref() {
        let bytes = read_selected_sidecar(&source, &sidecar.filename)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        if digest != sidecar.digest {
            return Err(StatusCode::NOT_FOUND);
        }
    }

    let (name, media_playlist, segments, etag_variant) =
        if let Some((variant_name, leaf_name)) = name.split_once('/') {
            let variant = publication
                .variants
                .iter()
                .find(|variant| variant.name == variant_name)
                .ok_or(StatusCode::NOT_FOUND)?;
            (
                leaf_name,
                &variant.media_playlist,
                &variant.segments,
                variant_name,
            )
        } else {
            if !publication.variants.is_empty() {
                return Err(StatusCode::NOT_FOUND);
            }
            (
                name.as_str(),
                &publication.media_playlist,
                &publication.segments,
                "",
            )
        };
    if name == "index.m3u8" {
        let text = media_playlist;
        let mut response = Response::new(if head_only {
            Body::empty()
        } else {
            Body::from(text.clone())
        });
        set_private_headers(&mut response, "application/vnd.apple.mpegurl", text.len());
        set_excerpt_headers(
            &mut response,
            publication.start_time_ticks,
            publication.excerpt_duration_ticks,
        );
        set_continuation_path(
            &mut response,
            &publication.item,
            &publication.id,
            publication.excerpt_duration_ticks,
        );
        return Ok(response);
    }
    let segment_number = name.strip_suffix(".ts").ok_or(StatusCode::NOT_FOUND)?;
    if segment_number.is_empty()
        || segment_number.len() > 2
        || (segment_number.starts_with('0') && segment_number.len() > 1)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let index = segment_number
        .parse::<usize>()
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let (path, size) = segments.get(index).ok_or(StatusCode::NOT_FOUND)?;
    if *size == 0 || *size > MAX_SEGMENT_BYTES {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if file
        .metadata()
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?
        .len()
        != *size
    {
        return Err(StatusCode::NOT_FOUND);
    }
    // A publication UUID plus its bounded segment index and fixed byte
    // count uniquely identifies this immutable, token-scoped MPEG-TS part.
    // A client may resume only when its If-Range strong validator matches.
    let etag = format!("\"hls-{playlist_id}-{etag_variant}-{index}-{size}\"");
    let apply_range = headers.get(IF_RANGE).is_none_or(|candidate| {
        candidate
            .to_str()
            .ok()
            .is_some_and(|candidate| candidate == etag)
    });
    let byte_range = if apply_range {
        headers.get(RANGE)
    } else {
        None
    };
    let byte_range = if let Some(value) = byte_range {
        match value
            .to_str()
            .ok()
            .and_then(|value| streaming::parse_range(value, *size))
        {
            Some(range) => Some(range),
            None => {
                let mut response = Response::new(Body::empty());
                *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                set_private_headers(&mut response, "video/mp2t", 0);
                response.headers_mut().insert(
                    ETAG,
                    HeaderValue::from_str(&etag).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                );
                response
                    .headers_mut()
                    .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
                response.headers_mut().insert(
                    CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{size}"))
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
                );
                return Ok(response);
            }
        }
    } else {
        None
    };
    let start = byte_range.as_ref().map_or(0, |range| range.start);
    let end = byte_range.as_ref().map_or(*size - 1, |range| range.end);
    let length = end - start + 1;
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let body = if head_only {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(file.take(length)))
    };
    let mut response = Response::new(body);
    let response_length = usize::try_from(length).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    set_private_headers(&mut response, "video/mp2t", response_length);
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&etag).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if byte_range.is_some() {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}"))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        );
    }
    Ok(response)
}
