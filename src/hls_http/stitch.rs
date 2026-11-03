//! Explicit, token-scoped two/three-window HLS preview built only from completed
//! private PGS publications. Does not encode, retain an unbounded chain or
//! advertise a partial source as a complete Jellyfin VOD.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StitchQuery {
    next: String,
    after_next: Option<String>,
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

/// Accept only the exact bounded, independently encoded media-playlist
/// grammar produced by Pelagos. A plausible EXTINF count is insufficient:
/// duplicate headers, unexpected tags or misplaced ENDLIST may otherwise be
/// silently carried into an authenticated stitched publication.
fn verified_fragment(entry: &Published) -> Result<(u32, Vec<&str>), StatusCode> {
    let body = &entry.media_playlist;
    if body.len() > MAX_MANIFEST || !body.ends_with('\n') || entry.segments.is_empty() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut lines = body.lines();
    if lines.next() != Some("#EXTM3U") || lines.next() != Some("#EXT-X-VERSION:3") {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let target = lines
        .next()
        .and_then(|line| line.strip_prefix("#EXT-X-TARGETDURATION:"))
        .and_then(|raw| raw.parse::<u32>().ok())
        .filter(|seconds| (1..=600).contains(seconds))
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let expected_sequence = format!("#EXT-X-MEDIA-SEQUENCE:{}", entry.media_sequence);
    if lines.next() != Some(expected_sequence.as_str()) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if entry.media_sequence != 0 {
        let expected = format!("#EXT-X-DISCONTINUITY-SEQUENCE:{}", entry.media_sequence);
        if lines.next() != Some(expected.as_str()) {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
    if lines.next() != Some("#EXT-X-PLAYLIST-TYPE:VOD") {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let mut pieces = Vec::with_capacity(entry.segments.len());
    for index in 0..entry.segments.len() {
        if index > 0 && lines.next() != Some("#EXT-X-DISCONTINUITY") {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        let cue = lines.next().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let (number, title) = cue
            .strip_prefix("#EXTINF:")
            .and_then(|raw| raw.split_once(','))
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let duration = number
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && (0.001..=600.0).contains(seconds))
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        if !title.is_empty() || duration.ceil() > f64::from(target) {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        let expected_path = format!("{index}.ts");
        if lines.next() != Some(expected_path.as_str()) {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        pieces.push(cue);
    }
    if lines.next() != Some("#EXT-X-ENDLIST") || lines.next().is_some() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok((target, pieces))
}

/// Read-only composition of two or three completed adjacent private HLS
/// excerpts. No segment is generated here, and three is the hard cap imposed
/// by the token-scoped publication registry.
fn make_preview_chain(
    item: &str,
    publications: &[&Published],
) -> Result<(String, u64), StatusCode> {
    if !(2..=3).contains(&publications.len()) {
        return Err(StatusCode::NOT_FOUND);
    }
    for pair in publications.windows(2) {
        validate_pair(pair[0], pair[1])?;
    }
    let fragments = publications
        .iter()
        .map(|publication| verified_fragment(publication))
        .collect::<Result<Vec<_>, _>>()?;
    let target = fragments
        .iter()
        .map(|(target, _)| *target)
        .max()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let duration = publications
        .iter()
        .try_fold(0_u64, |total, publication| {
            total.checked_add(publication.excerpt_duration_ticks?)
        })
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let first = publications[0];
    let mut playlist = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{}\n#EXT-X-DISCONTINUITY-SEQUENCE:{}\n#EXT-X-PLAYLIST-TYPE:VOD\n",
        first.media_sequence, first.media_sequence
    );
    let mut written = 0_usize;
    for (publication, (_, cues)) in publications.iter().zip(&fragments) {
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

#[cfg(test)]
fn make_preview(
    item: &str,
    first: &Published,
    next: &Published,
) -> Result<(String, u64), StatusCode> {
    make_preview_chain(item, &[first, next])
}

/// Revalidate every retained publication immediately before exposing a
/// stitched manifest. Automatic one-step advance and manual read-only stitch
/// must enforce identical token, grant and source-revision constraints.
async fn render_stitched(
    state: &Arc<AppState>,
    item: &str,
    headers: &HeaderMap,
    token: &str,
    publications: &[Arc<Published>],
) -> Result<Response, StatusCode> {
    if !(2..=3).contains(&publications.len()) {
        return Err(StatusCode::NOT_FOUND);
    }
    for pair in publications.windows(2) {
        validate_pair(&pair[0], &pair[1])?;
    }
    let _probe = state.hls_registry().acquire_track_probe()?;
    for publication in publications {
        reauthorize_publication(state, headers, &lookup(publication, token)).await?;
    }
    let borrowed = publications.iter().map(Arc::as_ref).collect::<Vec<_>>();
    let (playlist, duration) = make_preview_chain(item, &borrowed)?;
    let mut response = Response::new(Body::from(playlist.clone()));
    set_private_headers(
        &mut response,
        "application/vnd.apple.mpegurl",
        playlist.len(),
    );
    set_excerpt_headers(
        &mut response,
        publications[0].start_time_ticks,
        Some(duration),
    );
    response
        .headers_mut()
        .insert("x-pelagos-stitched", HeaderValue::from_static("true"));
    let last = publications
        .last()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    set_continuation_path(&mut response, item, &last.id, Some(duration));
    Ok(response)
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
    let third = query
        .after_next
        .as_deref()
        .map(|id| {
            registry
                .lookup(id, &item, "video", &token)
                .ok_or(StatusCode::NOT_FOUND)
        })
        .transpose()?;
    let mut publications = vec![first, next];
    if let Some(third) = third {
        publications.push(third);
    }
    render_stitched(&state, &item, &headers, &token, &publications).await
}

/// Extract only Pelagos's own trusted continuation UUID from an internal
/// response. Never interpret a client-supplied URL as a publication ID.
fn continuation_publication_id<'a>(path: &'a str, item: &str) -> Option<&'a str> {
    let prefix = format!("/Videos/{item}/hls1/");
    let id = path.strip_prefix(&prefix)?.strip_suffix("/next.m3u8")?;
    (id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(id)
}

/// Explicit one-step composition: publish exactly one adjacent bounded PGS
/// excerpt using the existing authorization/FFmpeg admission path, then
/// stitch it to the current window. No background job, open-ended playlist
/// or automatic HLS reload is implied.
pub(super) async fn video_advance(
    State(state): State<Arc<AppState>>,
    RoutePath((item, playlist_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let first = state
        .hls_registry()
        .lookup(&playlist_id, &item, "video", &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    if first.rendition_count != 1 || first.excerpt_duration_ticks.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let next_response = super::continuation::video_next(
        State(state.clone()),
        RoutePath((item.clone(), playlist_id)),
        headers.clone(),
    )
    .await?;
    if next_response.status() == StatusCode::NO_CONTENT {
        return Ok(next_response);
    }
    if next_response.status() != StatusCode::OK {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let path = next_response
        .headers()
        .get("x-pelagos-next-path")
        .and_then(|header| header.to_str().ok())
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let next_id =
        continuation_publication_id(path, &item).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let next = state
        .hls_registry()
        .lookup(next_id, &item, "video", &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    render_stitched(&state, &item, &headers, &token, &[first, next]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        root: &Path,
        id: &str,
        start: u64,
        duration: u64,
        media_sequence: u64,
        count: usize,
    ) -> Published {
        let mut media_playlist = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{media_sequence}\n"
        );
        if media_sequence != 0 {
            media_playlist.push_str(&format!("#EXT-X-DISCONTINUITY-SEQUENCE:{media_sequence}\n"));
        }
        media_playlist.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
        let segments = (0..count)
            .map(|index| {
                if index != 0 {
                    media_playlist.push_str("#EXT-X-DISCONTINUITY\n");
                }
                media_playlist.push_str(&format!("#EXTINF:2.000,\n{index}.ts\n"));
                (root.join(format!("fake-{id}-{index}.ts")), 376_u64)
            })
            .collect();
        media_playlist.push_str("#EXT-X-ENDLIST\n");
        Published {
            id: id.to_owned(),
            item: "synthetic-item".to_owned(),
            kind: "video",
            digest: [7; 32],
            source: PathBuf::from("/synthetic/feature.mkv"),
            source_size: 123,
            source_modified: None,
            target_seconds: 2,
            start_time_ticks: start,
            excerpt_duration_ticks: Some(duration),
            media_sequence,
            rendition_count: 1,
            subtitle_stream_index: Some(1),
            sidecar: Some(SidecarRevision {
                filename: "feature.es.sup".to_owned(),
                digest: [9; 32],
            }),
            private_root: root.join(format!("preview-{id}")),
            main_playlist: media_playlist.clone(),
            master_playlist: String::new(),
            media_playlist,
            segments,
            variants: Vec::new(),
        }
    }

    #[test]
    fn preview_rejects_noncanonical_fragment_headers_and_segments() {
        let temp = tempfile::tempdir().unwrap();
        let mut first = sample(temp.path(), "first", 100_000_000, 40_000_000, 0, 2);
        let next = sample(temp.path(), "next", 140_000_000, 20_000_000, 2, 1);
        let pristine = first.media_playlist.clone();
        assert!(make_preview("synthetic-item", &first, &next).is_ok());

        let malformed = [
            pristine.replace("#EXT-X-VERSION:3\n", ""),
            pristine.replace("#EXT-X-VERSION:3\n", "#EXT-X-VERSION:3\n#EXT-X-VERSION:3\n"),
            pristine.replace(
                "#EXT-X-MEDIA-SEQUENCE:0\n",
                "#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MEDIA-SEQUENCE:99\n",
            ),
            pristine.replace(
                "#EXT-X-PLAYLIST-TYPE:VOD\n",
                "#EXT-X-DISCONTINUITY-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n",
            ),
            pristine.replace("#EXT-X-TARGETDURATION:2\n", "#EXT-X-TARGETDURATION:1\n"),
            pristine.replace("#EXT-X-DISCONTINUITY\n", ""),
            pristine.replace(
                "#EXTINF:2.000,\n1.ts\n",
                "#EXT-X-DISCONTINUITY\n#EXTINF:2.000,\n0.ts\n",
            ),
            pristine.replace(
                "#EXTINF:2.000,\n1.ts\n",
                "#EXTINF:2.000,\n1.ts\n#EXT-X-KEY:METHOD=NONE\n",
            ),
            pristine.replace("#EXT-X-ENDLIST\n", ""),
            pristine.replace("#EXT-X-ENDLIST\n", "#EXT-X-ENDLIST\n0.ts\n"),
            pristine.trim_end_matches('\n').to_owned(),
        ];
        for (index, fragment) in malformed.into_iter().enumerate() {
            assert_ne!(
                fragment, pristine,
                "malformed fixture {index} had no effect"
            );
            first.media_playlist = fragment;
            assert_eq!(
                make_preview("synthetic-item", &first, &next),
                Err(StatusCode::INTERNAL_SERVER_ERROR),
                "accepted malformed fixture {index}"
            );
        }

        // A later, nonzero media-sequence fragment must carry one matching
        // discontinuity-sequence header in the canonical header position.
        let mut later = sample(temp.path(), "later", 140_000_000, 20_000_000, 2, 1);
        let late_original = later.media_playlist.clone();
        for fragment in [
            late_original.replace("#EXT-X-DISCONTINUITY-SEQUENCE:2\n", ""),
            late_original.replace(
                "#EXT-X-DISCONTINUITY-SEQUENCE:2\n",
                "#EXT-X-DISCONTINUITY-SEQUENCE:2\n#EXT-X-DISCONTINUITY-SEQUENCE:2\n",
            ),
        ] {
            later.media_playlist = fragment;
            assert_eq!(
                make_preview(
                    "synthetic-item",
                    &sample(temp.path(), "early", 100_000_000, 40_000_000, 0, 2),
                    &later
                ),
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            );
        }
    }

    #[test]
    fn preview_chain_requires_three_adjacent_publications_and_monotonic_segments() {
        let temp = tempfile::tempdir().unwrap();
        let first = sample(temp.path(), "first", 100_000_000, 40_000_000, 0, 2);
        let middle = sample(temp.path(), "middle", 140_000_000, 20_000_000, 2, 1);
        let mut third = sample(temp.path(), "third", 160_000_000, 20_000_000, 3, 2);
        let (manifest, duration) =
            make_preview_chain("synthetic-item", &[&first, &middle, &third]).unwrap();
        assert_eq!(duration, 80_000_000);
        assert_eq!(manifest.matches("#EXTINF:").count(), 5);
        assert_eq!(manifest.matches("#EXT-X-DISCONTINUITY\n").count(), 4);
        assert!(manifest.contains("/Videos/synthetic-item/hls1/third/1.ts\n"));
        assert!(manifest.ends_with("#EXT-X-ENDLIST\n"));

        third.media_sequence += 1;
        assert_eq!(
            make_preview_chain("synthetic-item", &[&first, &middle, &third]),
            Err(StatusCode::NOT_FOUND)
        );
        third.media_sequence -= 1;
        third.start_time_ticks += 1;
        assert_eq!(
            make_preview_chain("synthetic-item", &[&first, &middle, &third]),
            Err(StatusCode::NOT_FOUND)
        );
        third.start_time_ticks -= 1;

        third.media_playlist = third.media_playlist.replace("#EXT-X-ENDLIST\n", "");
        assert_eq!(
            make_preview_chain("synthetic-item", &[&first, &middle, &third]),
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        );
        assert_eq!(
            make_preview_chain("synthetic-item", &[&first]),
            Err(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            make_preview_chain("synthetic-item", &[&first, &middle, &third, &first]),
            Err(StatusCode::NOT_FOUND)
        );
    }

    #[test]
    fn continuation_path_is_only_accepted_for_exact_internal_uuid() {
        let item = "synthetic-item";
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            continuation_publication_id(
                "/Videos/synthetic-item/hls1/0123456789abcdef0123456789abcdef/next.m3u8",
                item
            ),
            Some(id)
        );
        for path in [
            "/Videos/other-item/hls1/0123456789abcdef0123456789abcdef/next.m3u8",
            "/Videos/synthetic-item/hls1/not-a-uuid/next.m3u8",
            "/Videos/synthetic-item/hls1/0123456789abcdef0123456789abcdef/other.m3u8",
            "/Videos/synthetic-item/hls1/0123456789abcdef0123456789abcdeg/next.m3u8",
            "/Videos/synthetic-item/hls1/0123456789abcdef0123456789abcdef/next.m3u8/extra",
        ] {
            assert_eq!(continuation_publication_id(path, item), None);
        }
    }

    #[test]
    fn preview_pair_requires_exact_published_timeline_and_shared_grant() {
        let temp = tempfile::tempdir().unwrap();
        let first = sample(temp.path(), "first", 100_000_000, 40_000_000, 0, 2);
        let mut next = sample(temp.path(), "next", 140_000_000, 20_000_000, 2, 1);
        let (manifest, duration) = make_preview("synthetic-item", &first, &next).unwrap();
        assert_eq!(duration, 60_000_000);
        assert_eq!(manifest.matches("#EXTINF:").count(), 3);
        assert_eq!(manifest.matches("#EXT-X-DISCONTINUITY\n").count(), 2);
        assert!(manifest.contains("/Videos/synthetic-item/hls1/first/0.ts"));
        assert!(manifest.contains("/Videos/synthetic-item/hls1/next/0.ts"));
        assert!(manifest.contains("#EXT-X-DISCONTINUITY-SEQUENCE:0\n"));
        assert_eq!(validate_pair(&next, &first), Err(StatusCode::NOT_FOUND));

        next.start_time_ticks += 1;
        assert_eq!(validate_pair(&first, &next), Err(StatusCode::NOT_FOUND));
        next.start_time_ticks -= 1;

        next.media_sequence += 1;
        assert_eq!(validate_pair(&first, &next), Err(StatusCode::NOT_FOUND));
        next.media_sequence -= 1;

        next.digest = [0; 32];
        assert_eq!(validate_pair(&first, &next), Err(StatusCode::NOT_FOUND));
        next.digest = [7; 32];

        next.sidecar.as_mut().unwrap().digest = [0; 32];
        assert_eq!(validate_pair(&first, &next), Err(StatusCode::NOT_FOUND));
        next.sidecar.as_mut().unwrap().digest = [9; 32];

        next.rendition_count = 2;
        assert_eq!(validate_pair(&first, &next), Err(StatusCode::NOT_FOUND));
        next.rendition_count = 1;

        let mut reordered = sample(temp.path(), "swapped", 100_000_000, 40_000_000, 0, 2);
        reordered.media_playlist = reordered
            .media_playlist
            .replace("0.ts", "placeholder.ts")
            .replace("1.ts", "0.ts")
            .replace("placeholder.ts", "1.ts");
        assert_eq!(
            make_preview("synthetic-item", &reordered, &next),
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        );

        next.media_playlist = "untrusted fragment".to_owned();
        assert_eq!(
            make_preview("synthetic-item", &first, &next),
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        );
    }
}
