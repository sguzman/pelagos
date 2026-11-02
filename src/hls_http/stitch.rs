//! Explicit, token-scoped two-window HLS preview built only from completed
//! private PGS publications. Does not encode, retain an unbounded chain or
//! advertise a partial source as a complete Jellyfin VOD.

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StitchQuery {
    next: String,
}

fn lookup<'a>(entry: &'a Published, token: &'a str) -> CacheLookup<'a> {
    CacheLookup {
        item: &entry.item,
        kind: entry.kind,
        token,
        source: &entry.source,
        size: entry.source_size,
        modified: entry.source_modified,
        target_seconds: entry.target_seconds,
        start_time_ticks: entry.start_time_ticks,
        excerpt_duration_ticks: entry.excerpt_duration_ticks,
        media_sequence: entry.media_sequence,
        rendition_count: entry.rendition_count,
        subtitle_stream_index: entry.subtitle_stream_index,
        sidecar: entry.sidecar.as_ref(),
    }
}

/// Only two *already generated* single-quality excerpts can be stitched. The
/// next UUID must denote an exactly adjacent publication, not a new source.
fn validate_pair(first: &Published, next: &Published) -> Result<(), StatusCode> {
    let duration = first.excerpt_duration_ticks.ok_or(StatusCode::NOT_FOUND)?;
    let end = first
        .start_time_ticks
        .checked_add(duration)
        .ok_or(StatusCode::NOT_FOUND)?;
    let sequence_end = first
        .media_sequence
        .checked_add(first.segments.len() as u64)
        .ok_or(StatusCode::NOT_FOUND)?;
    if first.kind != "video"
        || next.kind != "video"
        || first.rendition_count != 1
        || next.rendition_count != 1
        || next.excerpt_duration_ticks.is_none()
        || first.segments.is_empty()
        || next.segments.is_empty()
        || first.segments.len() > 64
        || next.segments.len() > 64
        || first.id == next.id
        || first.item != next.item
        || first.digest != next.digest
        || first.source != next.source
        || first.source_size != next.source_size
        || first.source_modified != next.source_modified
        || first.sidecar != next.sidecar
        || first.subtitle_stream_index != next.subtitle_stream_index
        || first.target_seconds != next.target_seconds
        || next.start_time_ticks != end
        || next.media_sequence != sequence_end
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(())
}

fn verified_fragment(entry: &Published) -> Result<(u32, Vec<&str>), StatusCode> {
    let body = &entry.media_playlist;
    if body.len() > MAX_MANIFEST || !body.starts_with("#EXTM3U\n") {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let expected_sequence = format!("#EXT-X-MEDIA-SEQUENCE:{}", entry.media_sequence);
    if body
        .lines()
        .filter(|line| *line == expected_sequence)
        .count()
        != 1
    {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let target = body
        .lines()
        .find_map(|line| line.strip_prefix("#EXT-X-TARGETDURATION:"))
        .and_then(|raw| raw.parse::<u32>().ok())
        .filter(|seconds| (1..=600).contains(seconds))
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let pieces = body
        .lines()
        .filter(|line| line.starts_with("#EXTINF:"))
        .collect::<Vec<_>>();
    if pieces.len() != entry.segments.len()
        || pieces.is_empty()
        || body.matches("#EXT-X-DISCONTINUITY\n").count() != pieces.len() - 1
    {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    for (index, line) in pieces.iter().enumerate() {
        let (number, title) = line
            .strip_prefix("#EXTINF:")
            .and_then(|raw| raw.split_once(','))
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        if !title.is_empty()
            || number.parse::<f64>().ok().is_none_or(|duration| {
                !duration.is_finite() || !(0.001..=600.0).contains(&duration)
            })
            || !body.lines().any(|entry| entry == format!("{index}.ts"))
        {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    Ok((target, pieces))
}

fn make_preview(
    item: &str,
    first: &Published,
    next: &Published,
) -> Result<(String, u64), StatusCode> {
    validate_pair(first, next)?;
    let (target_a, cues_a) = verified_fragment(first)?;
    let (target_b, cues_b) = verified_fragment(next)?;
    let duration = first
        .excerpt_duration_ticks
        .and_then(|a| next.excerpt_duration_ticks?.checked_add(a))
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut playlist = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:{}\n#EXT-X-DISCONTINUITY-SEQUENCE:{}\n#EXT-X-PLAYLIST-TYPE:VOD\n",
        target_a.max(target_b),
        first.media_sequence,
        first.media_sequence
    );
    let mut written = 0_usize;
    for (publication, cues) in [(first, cues_a), (next, cues_b)] {
        for (index, cue) in cues.iter().enumerate() {
            if written > 0 {
                playlist.push_str("#EXT-X-DISCONTINUITY\n");
            }
            playlist.push_str(cue);
            playlist.push('\n');
            playlist.push_str(&format!(
                "/Videos/{item}/hls1/{}/{index}.ts\n",
                publication.id
            ));
            written += 1;
        }
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    if playlist.len() > MAX_MANIFEST {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok((playlist, duration))
}

pub(super) async fn video_stitch(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist_id)): RoutePath<(String, String)>,
    Query(query): Query<StitchQuery>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let registry = state.hls_registry();
    let first = registry
        .lookup(&playlist_id, &item, "video", &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    let next = registry
        .lookup(&query.next, &item, "video", &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    validate_pair(&first, &next)?;
    reauthorize_publication(&state, &headers, &lookup(&first, &token)).await?;
    reauthorize_publication(&state, &headers, &lookup(&next, &token)).await?;
    let (playlist, duration) = make_preview(&item, &first, &next)?;
    let mut response = Response::new(Body::from(playlist.clone()));
    set_private_headers(
        &mut response,
        "application/vnd.apple.mpegurl",
        playlist.len(),
    );
    set_excerpt_headers(&mut response, first.start_time_ticks, Some(duration));
    response
        .headers_mut()
        .insert("x-pelagos-stitched", HeaderValue::from_static("true"));
    set_continuation_path(&mut response, &item, &next.id, Some(duration));
    Ok(response)
}
