//! Small, expiring token-scoped HLS preview leases, independent of the
//! three-entry LRU transcode cache. Leases keep completed private segment
//! roots alive while unrelated clients publish, never schedule new media.
use super::registry::matches_cache_query;
use super::*;
use std::time::{Duration, Instant};

const MAX_RETAINED_PREVIEWS: usize = 2;
const MIN_PREVIEW_LIFETIME_SECONDS: u64 = 180;
const MAX_PREVIEW_LIFETIME_SECONDS: u64 = 1_860;

/// Retain a complete preview for its advertised duration plus one minute,
/// clamped to three minutes minimum and 31 minutes maximum. Longer source
/// windows cannot outlive the bounded three-publication preview contract.
fn preview_lifetime(duration_ticks: u64) -> Duration {
    let seconds = duration_ticks
        .div_ceil(10_000_000)
        .saturating_add(60)
        .clamp(MIN_PREVIEW_LIFETIME_SECONDS, MAX_PREVIEW_LIFETIME_SECONDS);
    Duration::from_secs(seconds)
}

#[derive(Debug)]
pub(super) struct RetainedPreview {
    pub(super) id: String,
    pub(super) item: String,
    pub(super) digest: [u8; 32],
    pub(super) manifest: String,
    pub(super) start_ticks: u64,
    pub(super) duration_ticks: u64,
    pub(super) publications: Vec<Arc<Published>>,
    expires_at: Instant,
}

impl HlsRegistry {
    /// Expired leases drop their Arc-held segment roots once no in-flight
    /// HTTP response still references them. Sweep on new publications and
    /// every retention lookup, without a background cleanup worker.
    pub(super) fn prune_retained(&self) {
        let now = Instant::now();
        self.retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|lease| lease.expires_at > now);
    }

    /// A bounded lease retains at most three single-quality PGS excerpts.
    /// Repeating the same chain returns its original stable URI and expiry.
    /// Once both slots are occupied, new previews receive HTTP 429 rather
    /// than silently evicting another active client's retained segments.
    pub(super) fn retain_preview(
        &self,
        item: &str,
        token: &str,
        publications: &[Arc<Published>],
        manifest: String,
        duration_ticks: u64,
    ) -> Result<Arc<RetainedPreview>, StatusCode> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if !(2..=3).contains(&publications.len())
            || duration_ticks == 0
            || manifest.len() > MAX_MANIFEST
            || publications.iter().any(|publication| {
                publication.item != item
                    || publication.kind != "video"
                    || publication.digest != digest
                    || publication.rendition_count != 1
            })
        {
            return Err(StatusCode::NOT_FOUND);
        }
        let now = Instant::now();
        let mut leases = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        leases.retain(|lease| lease.expires_at > now);
        if let Some(existing) = leases.iter().find(|lease| {
            lease.item == item
                && lease.digest == digest
                && lease.publications.len() == publications.len()
                && lease
                    .publications
                    .iter()
                    .zip(publications)
                    .all(|(retained, current)| retained.id == current.id)
        }) {
            return Ok(existing.clone());
        }
        if leases.len() >= MAX_RETAINED_PREVIEWS {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let lease = Arc::new(RetainedPreview {
            id: Uuid::new_v4().simple().to_string(),
            item: item.to_owned(),
            digest,
            manifest,
            start_ticks: publications[0].start_time_ticks,
            duration_ticks,
            publications: publications.to_vec(),
            expires_at: now + preview_lifetime(duration_ticks),
        });
        leases.push_back(lease.clone());
        Ok(lease)
    }

    pub(super) fn retained_preview(
        &self,
        id: &str,
        item: &str,
        token: &str,
    ) -> Option<Arc<RetainedPreview>> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let now = Instant::now();
        let mut leases = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        leases.retain(|lease| lease.expires_at > now);
        leases
            .iter()
            .find(|lease| lease.id == id && lease.item == item && lease.digest == digest)
            .cloned()
    }

    /// Releasing a lease never revokes another client's preview. Its private
    /// TS files are dropped once no cache, lease or in-flight response owns
    /// the underlying publication Arcs.
    pub(super) fn release_retained(&self, id: &str, item: &str, token: &str) -> bool {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let removed = {
            let mut leases = self
                .retained
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            leases.retain(|lease| lease.expires_at > Instant::now());
            leases
                .iter()
                .position(|lease| lease.id == id && lease.item == item && lease.digest == digest)
                .and_then(|index| leases.remove(index))
        };
        removed.is_some()
    }

    pub(super) fn retained_publication(
        &self,
        id: &str,
        item: &str,
        kind: &str,
        digest: &[u8; 32],
    ) -> Option<Arc<Published>> {
        let now = Instant::now();
        let mut leases = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        leases.retain(|lease| lease.expires_at > now);
        leases
            .iter()
            .filter(|lease| lease.item == item && &lease.digest == digest)
            .flat_map(|lease| &lease.publications)
            .find(|entry| entry.id == id && entry.item == item && entry.kind == kind)
            .cloned()
    }

    pub(super) fn retained_cached(
        &self,
        query: &CacheLookup<'_>,
        digest: &[u8; 32],
    ) -> Option<Arc<Published>> {
        let now = Instant::now();
        let mut leases = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        leases.retain(|lease| lease.expires_at > now);
        leases
            .iter()
            .filter(|lease| lease.item == query.item && &lease.digest == digest)
            .flat_map(|lease| &lease.publications)
            .find(|entry| matches_cache_query(entry, query, digest))
            .cloned()
    }
}

pub(super) fn preview_response(lease: &RetainedPreview) -> Result<Response, StatusCode> {
    let mut response = Response::new(Body::from(lease.manifest.clone()));
    set_private_headers(
        &mut response,
        "application/vnd.apple.mpegurl",
        lease.manifest.len(),
    );
    set_excerpt_headers(&mut response, lease.start_ticks, Some(lease.duration_ticks));
    response
        .headers_mut()
        .insert("x-pelagos-stitched", HeaderValue::from_static("true"));
    let last = lease
        .publications
        .last()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    set_continuation_path(
        &mut response,
        &lease.item,
        &last.id,
        Some(lease.duration_ticks),
    );
    let path = format!("/Videos/{}/hls1/{}/retained.m3u8", lease.item, lease.id);
    response.headers_mut().insert(
        "x-pelagos-retained-path",
        HeaderValue::from_str(&path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok(response)
}

/// An immutable 180-second playlist URL. Per-request reauthorization is
/// mandatory; pinning the encoded TS bytes does not pin library permissions.
pub(super) async fn video_retained(
    State(state): State<Arc<AppState>>,
    RoutePath((item, lease_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let lease = state
        .hls_registry()
        .retained_preview(&lease_id, &item, &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    let _probe = state.hls_registry().acquire_track_probe()?;
    for publication in &lease.publications {
        reauthorize_publication(
            &state,
            &headers,
            &super::stitch::lookup(publication, &token),
        )
        .await?;
    }
    // Do not return a stale lease after asynchronous permission/revision
    // checks, even if it expired during the last sidecar read.
    if state
        .hls_registry()
        .retained_preview(&lease_id, &item, &token)
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    preview_response(&lease)
}

/// Release is a token-scoped control-plane action, not a request to delete
/// source media or global HLS cache entries.
pub(super) async fn video_release(
    State(state): State<Arc<AppState>>,
    RoutePath((item, lease_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers).await?;
    if !state
        .hls_registry()
        .release_retained(&lease_id, &item, &token)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(root: &Path, id: &str, start: u64) -> Arc<Published> {
        Arc::new(Published {
            id: id.to_owned(),
            item: "movie".to_owned(),
            kind: "video",
            digest: Sha256::digest(b"viewer-token").into(),
            source: PathBuf::from("/synthetic/movie.mkv"),
            source_size: 123,
            source_modified: None,
            target_seconds: 1,
            start_time_ticks: start,
            excerpt_duration_ticks: Some(10_000_000),
            media_sequence: start / 10_000_000,
            rendition_count: 1,
            subtitle_stream_index: Some(1),
            sidecar: None,
            private_root: root.join(id),
            main_playlist: String::new(),
            master_playlist: String::new(),
            media_playlist: String::new(),
            segments: Vec::new(),
            variants: Vec::new(),
        })
    }

    #[test]
    fn retention_lifetime_covers_bounded_hls_playback_without_unbounded_lease() {
        assert_eq!(preview_lifetime(10_000_000), Duration::from_secs(180));
        assert_eq!(preview_lifetime(600 * 10_000_000), Duration::from_secs(660));
        assert_eq!(
            preview_lifetime(1800 * 10_000_000),
            Duration::from_secs(1860)
        );
        assert_eq!(preview_lifetime(u64::MAX), Duration::from_secs(1860));
    }

    #[test]
    fn retained_roots_survive_lru_eviction_and_obey_token_and_capacity() {
        let root = tempfile::tempdir().unwrap();
        let registry = HlsRegistry::new(PathBuf::from("ffmpeg"));
        let first = fake(root.path(), "first", 0);
        let second = fake(root.path(), "second", 10_000_000);
        let third = fake(root.path(), "third", 20_000_000);
        for publication in [&first, &second, &third] {
            registry.publish(publication.clone());
        }
        let first_group = registry
            .retain_preview(
                "movie",
                "viewer-token",
                &[first.clone(), second.clone()],
                "#EXTM3U\n".to_owned(),
                20_000_000,
            )
            .unwrap();
        assert_eq!(
            registry
                .retain_preview(
                    "movie",
                    "viewer-token",
                    &[first.clone(), second.clone()],
                    "#EXTM3U\n".to_owned(),
                    20_000_000,
                )
                .unwrap()
                .id,
            first_group.id
        );
        let other_group = registry
            .retain_preview(
                "movie",
                "viewer-token",
                &[second.clone(), third.clone()],
                "#EXTM3U\n".to_owned(),
                20_000_000,
            )
            .unwrap();
        assert_ne!(first_group.id, other_group.id);
        assert_eq!(
            registry
                .retain_preview(
                    "movie",
                    "viewer-token",
                    &[first.clone(), second, third],
                    "#EXTM3U\n".to_owned(),
                    30_000_000,
                )
                .err(),
            Some(StatusCode::TOO_MANY_REQUESTS)
        );

        for position in 0..4 {
            registry.publish(fake(
                root.path(),
                &format!("unrelated-{position}"),
                1_000_000_000 + position,
            ));
        }
        assert!(
            registry
                .lookup("first", "movie", "video", "viewer-token")
                .is_some()
        );
        assert!(
            registry
                .lookup("third", "movie", "video", "viewer-token")
                .is_some()
        );
        assert!(
            registry
                .lookup("first", "movie", "video", "different-token")
                .is_none()
        );
        assert!(
            registry
                .retained_preview(&first_group.id, "movie", "different-token")
                .is_none()
        );
        assert!(
            registry
                .retained_preview(&first_group.id, "movie", "viewer-token")
                .is_some()
        );
        assert!(!registry.release_retained(&first_group.id, "movie", "different-token"));
        assert!(registry.release_retained(&first_group.id, "movie", "viewer-token"));
        assert!(!registry.release_retained(&first_group.id, "movie", "viewer-token"));
        assert!(
            registry
                .retained_preview(&first_group.id, "movie", "viewer-token")
                .is_none()
        );
        // The other preview shares the middle and retains the third, but not
        // the first publication after it has been evicted from the LRU.
        assert!(
            registry
                .lookup("first", "movie", "video", "viewer-token")
                .is_none()
        );
        assert!(
            registry
                .lookup("third", "movie", "video", "viewer-token")
                .is_some()
        );
    }
}
