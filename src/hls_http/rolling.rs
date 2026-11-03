//! Experimental, opt-in on-demand HLS sliding session. One active session,
//! at most three completed PGS publications, no background transcode worker.
//! It does not imply full Jellyfin Smart TV compatibility.
use super::*;
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

const MAX_ROLLING_SESSIONS: usize = 1;
const ROLLING_LIFETIME: Duration = Duration::from_secs(1_860);

#[derive(Debug)]
pub(super) struct RollingSession {
    id: String,
    item: String,
    digest: [u8; 32],
    expires_at: Instant,
    /// Prevent two playlist reloads from encoding the same successor.
    advance: tokio::sync::Mutex<()>,
    window: Mutex<RollingWindow>,
}

#[derive(Debug)]
struct RollingWindow {
    publications: VecDeque<Arc<Published>>,
    eof: bool,
    next_due: Instant,
}

#[derive(Clone)]
struct Snapshot {
    publications: Vec<Arc<Published>>,
    eof: bool,
    next_due: Instant,
}

fn interval(duration_ticks: u64) -> Duration {
    Duration::from_millis(duration_ticks.div_ceil(10_000).clamp(100, 600_000))
}

impl RollingSession {
    fn new(publication: Arc<Published>) -> Self {
        let duration_ticks = publication.excerpt_duration_ticks.unwrap_or(10_000_000);
        Self {
            id: Uuid::new_v4().simple().to_string(),
            item: publication.item.clone(),
            digest: publication.digest,
            expires_at: Instant::now() + ROLLING_LIFETIME,
            advance: tokio::sync::Mutex::new(()),
            window: Mutex::new(RollingWindow {
                publications: VecDeque::from([publication]),
                eof: false,
                next_due: Instant::now() + interval(duration_ticks),
            }),
        }
    }

    fn snapshot(&self) -> Snapshot {
        let window = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Snapshot {
            publications: window.publications.iter().cloned().collect(),
            eof: window.eof,
            next_due: window.next_due,
        }
    }

    fn append(&self, previous_id: &str, publication: Arc<Published>) -> Result<(), StatusCode> {
        let mut window = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if window.eof
            || window
                .publications
                .back()
                .is_none_or(|entry| entry.id != previous_id)
        {
            return Err(StatusCode::NOT_FOUND);
        }
        if window.publications.len() >= 3 {
            window.publications.pop_front();
        }
        window.next_due = Instant::now()
            + interval(
                publication
                    .excerpt_duration_ticks
                    .ok_or(StatusCode::NOT_FOUND)?,
            );
        window.publications.push_back(publication);
        Ok(())
    }

    fn finish(&self) {
        let mut window = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        window.eof = true;
    }
}

impl HlsRegistry {
    pub(super) fn prune_rolling(&self) {
        self.rolling
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|session| session.expires_at > Instant::now());
    }

    fn register_rolling(
        &self,
        first: Arc<Published>,
        token: &str,
    ) -> Result<Arc<RollingSession>, StatusCode> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if first.digest != digest
            || first.kind != "video"
            || first.excerpt_duration_ticks.is_none()
            || first.rendition_count != 1
            || first.subtitle_stream_index.is_none()
        {
            return Err(StatusCode::NOT_FOUND);
        }
        let mut sessions = self
            .rolling
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|session| session.expires_at > Instant::now());
        if let Some(existing) = sessions.iter().find(|session| {
            session.item == first.item
                && session.digest == digest
                && session
                    .window
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .publications
                    .front()
                    .is_some_and(|entry| entry.id == first.id)
        }) {
            return Ok(existing.clone());
        }
        if sessions.len() >= MAX_ROLLING_SESSIONS {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let session = Arc::new(RollingSession::new(first));
        sessions.push_back(session.clone());
        Ok(session)
    }

    fn rolling_session(&self, id: &str, item: &str, token: &str) -> Option<Arc<RollingSession>> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut sessions = self
            .rolling
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|session| session.expires_at > Instant::now());
        sessions
            .iter()
            .find(|session| session.id == id && session.item == item && session.digest == digest)
            .cloned()
    }

    pub(super) fn rolling_publication(
        &self,
        id: &str,
        item: &str,
        kind: &str,
        digest: &[u8; 32],
    ) -> Option<Arc<Published>> {
        let mut sessions = self
            .rolling
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|session| session.expires_at > Instant::now());
        sessions
            .iter()
            .filter(|session| session.item == item && &session.digest == digest)
            .find_map(|session| {
                session
                    .window
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .publications
                    .iter()
                    .find(|entry| entry.id == id && entry.item == item && entry.kind == kind)
                    .cloned()
            })
    }

    fn release_rolling(&self, id: &str, item: &str, token: &str) -> bool {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut sessions = self
            .rolling
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|session| session.expires_at > Instant::now());
        sessions
            .iter()
            .position(|session| {
                session.id == id && session.item == item && session.digest == digest
            })
            .and_then(|position| sessions.remove(position))
            .is_some()
    }
}

async fn response_for_session(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    token: &str,
    session: &Arc<RollingSession>,
) -> Result<Response, StatusCode> {
    let snapshot = session.snapshot();
    let _probe = state.hls_registry().acquire_track_probe()?;
    for publication in &snapshot.publications {
        reauthorize_publication(state, headers, &super::stitch::lookup(publication, token)).await?;
    }
    if state
        .hls_registry()
        .rolling_session(&session.id, &session.item, token)
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let refs = snapshot
        .publications
        .iter()
        .map(Arc::as_ref)
        .collect::<Vec<_>>();
    let playlist = super::stitch::make_rolling_playlist(&session.item, &refs, snapshot.eof)?;
    let mut response = playlist_response(&playlist);
    response
        .headers_mut()
        .insert("x-pelagos-rolling", HeaderValue::from_static("true"));
    let path = format!("/Videos/{}/hls1/{}/rolling.m3u8", session.item, session.id);
    response.headers_mut().insert(
        "x-pelagos-rolling-path",
        HeaderValue::from_str(&path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    Ok(response)
}

pub(super) async fn video_rolling_start(
    State(state): State<Arc<AppState>>,
    RoutePath((item, publication_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let first = state
        .hls_registry()
        .lookup(&publication_id, &item, "video", &token)
        .ok_or(StatusCode::NOT_FOUND)?;
    if first.rendition_count != 1
        || first.excerpt_duration_ticks.is_none()
        || first.subtitle_stream_index.is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let _probe = state.hls_registry().acquire_track_probe()?;
    reauthorize_publication(&state, &headers, &super::stitch::lookup(&first, &token)).await?;
    drop(_probe);
    let session = state.hls_registry().register_rolling(first, &token)?;
    response_for_session(&state, &headers, &token, &session).await
}

pub(super) async fn video_rolling(
    State(state): State<Arc<AppState>>,
    RoutePath((item, session_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers.clone()).await?;
    let session = state
        .hls_registry()
        .rolling_session(&session_id, &item, &token)
        .ok_or(StatusCode::NOT_FOUND)?;

    // Reloading the SAME URI advances at most one bounded adjacent source
    // window when its scheduled interval has elapsed. Concurrent requests
    // receive the prior valid representation rather than queue more FFmpeg.
    let snapshot = session.snapshot();
    if !snapshot.eof && snapshot.next_due <= Instant::now() {
        if let Ok(_gate) = session.advance.try_lock() {
            let current = session.snapshot();
            if !current.eof && current.next_due <= Instant::now() {
                let last = current
                    .publications
                    .last()
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                let result = super::continuation::video_next(
                    State(state.clone()),
                    RoutePath((item.clone(), last.id.clone())),
                    headers.clone(),
                )
                .await;
                match result {
                    Ok(response) if response.status() == StatusCode::NO_CONTENT => {
                        // EOF is confirmed by the same authenticated FFprobe
                        // path that handles explicit private continuation.
                        session.finish();
                    }
                    Ok(response) if response.status() == StatusCode::OK => {
                        let path = response
                            .headers()
                            .get("x-pelagos-next-path")
                            .and_then(|value| value.to_str().ok())
                            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                        let next_id = super::stitch::continuation_publication_id(path, &item)
                            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                        let next = state
                            .hls_registry()
                            .lookup(next_id, &item, "video", &token)
                            .ok_or(StatusCode::NOT_FOUND)?;
                        super::stitch::validate_pair(last, &next)?;
                        let _probe = state.hls_registry().acquire_track_probe()?;
                        reauthorize_publication(
                            &state,
                            &headers,
                            &super::stitch::lookup(&next, &token),
                        )
                        .await?;
                        session.append(&last.id, next)?;
                    }
                    Err(StatusCode::TOO_MANY_REQUESTS) | Err(StatusCode::SERVICE_UNAVAILABLE) => {
                        // Keep valid segments available; a future reload may
                        // retry the bounded worker rather than crash playback.
                    }
                    Ok(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
                    Err(error) => return Err(error),
                }
            }
        }
    }
    response_for_session(&state, &headers, &token, &session).await
}

pub(super) async fn video_rolling_release(
    State(state): State<Arc<AppState>>,
    RoutePath((item, session_id)): RoutePath<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let (_, token) = auth_api::authenticated(state.clone(), headers).await?;
    if !state
        .hls_registry()
        .release_rolling(&session_id, &item, &token)
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

    fn example(root: &Path, id: &str, start: u64) -> Arc<Published> {
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
    fn rolling_session_is_single_slot_token_scoped_and_bounded_to_three_roots() {
        let root = tempfile::tempdir().unwrap();
        let registry = HlsRegistry::new(PathBuf::from("ffmpeg"));
        let first = example(root.path(), "first", 0);
        let second = example(root.path(), "second", 10_000_000);
        let third = example(root.path(), "third", 20_000_000);
        let fourth = example(root.path(), "fourth", 30_000_000);
        let rolling = registry
            .register_rolling(first.clone(), "viewer-token")
            .unwrap();
        let again = registry.register_rolling(first, "viewer-token").unwrap();
        assert_eq!(rolling.id, again.id);
        assert_eq!(
            registry
                .register_rolling(second.clone(), "viewer-token")
                .err(),
            Some(StatusCode::TOO_MANY_REQUESTS)
        );
        assert!(
            registry
                .rolling_session(&rolling.id, "movie", "another-token")
                .is_none()
        );
        rolling.append("first", second).unwrap();
        rolling.append("second", third).unwrap();
        rolling.append("third", fourth).unwrap();
        let snapshot = rolling.snapshot();
        assert_eq!(snapshot.publications.len(), 3);
        assert_eq!(snapshot.publications[0].id, "second");
        assert_eq!(snapshot.publications[2].id, "fourth");
        assert!(
            registry
                .rolling_publication("first", "movie", "video", &rolling.digest)
                .is_none()
        );
        assert!(
            registry
                .rolling_publication("fourth", "movie", "video", &rolling.digest)
                .is_some()
        );
        assert!(!registry.release_rolling(&rolling.id, "movie", "another-token"));
        assert!(registry.release_rolling(&rolling.id, "movie", "viewer-token"));
        assert!(
            registry
                .rolling_session(&rolling.id, "movie", "viewer-token")
                .is_none()
        );
    }
}
