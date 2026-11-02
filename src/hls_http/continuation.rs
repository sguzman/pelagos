//! Explicit authenticated continuation of a bounded private PGS HLS excerpt.
//! Never exposes a complete long-VOD playlist or a background scheduler.
use super::*;

/// Plan the immediately adjacent half-open source-time interval, never
/// crossing the actual media EOF or the supported unwrapped PGS PTS ceiling.
fn next_interval(
    start_ticks: u64,
    duration_ticks: u64,
    source_end_ticks: u64,
) -> Result<Option<(u64, u64)>, StatusCode> {
    let next_start = start_ticks
        .checked_add(duration_ticks)
        .ok_or(StatusCode::BAD_REQUEST)?;
    let remaining = source_end_ticks.saturating_sub(next_start);
    let max_pts_remaining = MAX_PGS_START_END_TICKS.saturating_sub(next_start);
    let next_duration = duration_ticks.min(remaining).min(max_pts_remaining);
    Ok((next_duration >= 1_000_000).then_some((next_start, next_duration)))
}

pub(super) async fn video_next(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let publication = state
        .hls_registry()
        .lookup(&playlist_id, &item, "video", &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    let duration = publication
        .excerpt_duration_ticks
        .ok_or(StatusCode::NOT_FOUND)?;
    let _admission = state.hls_registry().acquire_continuation()?;
    let query = CacheLookup {
        item: &item,
        kind: "video",
        token: &token,
        source: &publication.source,
        size: publication.source_size,
        modified: publication.source_modified,
        target_seconds: publication.target_seconds,
        start_time_ticks: publication.start_time_ticks,
        excerpt_duration_ticks: publication.excerpt_duration_ticks,
        rendition_count: publication.rendition_count,
        subtitle_stream_index: publication.subtitle_stream_index,
        sidecar: publication.sidecar.as_ref(),
    };
    reauthorize_publication(&state, &headers, &query).await?;
    // The next interval is clamped to the actual probed end, not guessed
    // from a VOD playlist's ENDLIST marker or the previous segment lengths.
    let inspected = crate::probe::probe(
        &publication.source,
        state.probe_executable(),
        crate::probe::DEFAULT_TIMEOUT,
    )
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let source_seconds = inspected
        .format
        .as_ref()
        .and_then(|format| format["duration"].as_str())
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|value| value.is_finite() && (0.1..=86_400.0).contains(value))
        .ok_or(StatusCode::NOT_IMPLEMENTED)?;
    let source_end_ticks = (source_seconds * 10_000_000.0).floor() as u64;
    let Some((next_start, next_duration)) = next_interval(
        publication.start_time_ticks,
        duration,
        source_end_ticks,
    )? else {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
        return Ok(response);
    }

    // Re-enter the normal authenticated publication path. It independently
    // revalidates the token/grant/media revision, explicit PGS selection,
    // sidecar digest and FFmpeg outputs before issuing the new UUID.
    let request = PlaylistQuery {
        segment_length: Some(publication.target_seconds),
        start_time_ticks: Some(next_start as i64),
        pelagos_excerpt_duration_ticks: Some(next_duration as i64),
        renditions: Some(publication.rendition_count),
        media_source_id: Some(item.clone()),
        subtitle_stream_index: publication.subtitle_stream_index,
        subtitle_method: Some("Encode".to_owned()),
        expected_revision: Some(ExpectedRevision {
            source: publication.source.clone(),
            size: publication.source_size,
            modified: publication.source_modified,
            sidecar: publication.sidecar.clone(),
        }),
        ..PlaylistQuery::default()
    };
    generate_hls_playlist(
        state.clone(),
        item,
        "video",
        request,
        headers,
        publication.rendition_count > 1,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_window_planning_clamps_eof_pts_and_overflow() {
        assert_eq!(
            next_interval(6_050_000_000, 40_000_000, 6_100_000_000),
            Ok(Some((6_090_000_000, 10_000_000)))
        );
        assert_eq!(
            next_interval(6_090_000_000, 10_000_000, 6_100_000_000),
            Ok(None)
        );
        let pts_limit = MAX_PGS_START_END_TICKS;
        assert_eq!(
            next_interval(
                pts_limit - 2_000_000,
                1_000_000,
                pts_limit + 10_000_000,
            ),
            Ok(Some((pts_limit - 1_000_000, 1_000_000)))
        );
        assert_eq!(
            next_interval(pts_limit - 500_000, 200_000, pts_limit + 10_000_000),
            Ok(None)
        );
        assert_eq!(
            next_interval(u64::MAX, 1, u64::MAX),
            Err(StatusCode::BAD_REQUEST)
        );
    }
}
