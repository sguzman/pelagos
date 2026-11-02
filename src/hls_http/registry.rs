//! Token-scoped private HLS sessions: bounded publication, LRU eviction and admission.
use super::*;
use crate::transcode::TranscodeController;
use std::{collections::VecDeque, sync::Mutex, time::Duration};
use tokio::sync::Semaphore;

const MAX_PUBLISHED: usize = 3;

#[derive(Debug)]
pub(super) struct PublishedVariant {
    pub(super) name: String,
    pub(super) media_playlist: String,
    pub(super) segments: Vec<(PathBuf, u64)>,
}

#[derive(Debug)]
pub(super) struct Published {
    pub(super) id: String,
    pub(super) item: String,
    pub(super) kind: &'static str,
    pub(super) digest: [u8; 32],
    pub(super) source: PathBuf,
    pub(super) source_size: u64,
    pub(super) source_modified: Option<std::time::SystemTime>,
    pub(super) target_seconds: u32,
    pub(super) start_time_ticks: u64,
    pub(super) excerpt_duration_ticks: Option<u64>,
    pub(super) rendition_count: u8,
    pub(super) subtitle_stream_index: Option<i64>,
    pub(super) sidecar: Option<SidecarRevision>,
    pub(super) private_root: PathBuf,
    pub(super) main_playlist: String,
    pub(super) master_playlist: String,
    pub(super) media_playlist: String,
    pub(super) segments: Vec<(PathBuf, u64)>,
    pub(super) variants: Vec<PublishedVariant>,
}
impl Drop for Published {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.private_root);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SidecarRevision {
    pub(super) filename: String,
    pub(super) digest: [u8; 32],
}

pub(super) struct CacheLookup<'a> {
    pub(super) item: &'a str,
    pub(super) kind: &'a str,
    pub(super) token: &'a str,
    pub(super) source: &'a Path,
    pub(super) size: u64,
    pub(super) modified: Option<std::time::SystemTime>,
    pub(super) target_seconds: u32,
    pub(super) start_time_ticks: u64,
    pub(super) excerpt_duration_ticks: Option<u64>,
    pub(super) rendition_count: u8,
    pub(super) subtitle_stream_index: Option<i64>,
    pub(super) sidecar: Option<&'a SidecarRevision>,
}

/// Small private LRU cache; segment access promotes active publications and
/// an in-flight request holds its own Arc during eviction.
#[derive(Debug)]
pub struct HlsRegistry {
    pub(super) jobs: Semaphore,
    pub(super) waiters: Semaphore,
    continuations: Semaphore,
    track_probes: Semaphore,
    pub(super) controller: TranscodeController,
    pub(super) entries: Mutex<VecDeque<Arc<Published>>>,
}
impl HlsRegistry {
    pub fn new(executable: PathBuf) -> Self {
        Self {
            jobs: Semaphore::new(1),
            waiters: Semaphore::new(4),
            continuations: Semaphore::new(4),
            track_probes: Semaphore::new(4),
            controller: TranscodeController::new(
                executable,
                1,
                Duration::from_secs(120),
                MAX_SEGMENT_BYTES,
            )
            .expect("constant HLS worker limits"),
            entries: Mutex::new(VecDeque::new()),
        }
    }

    /// Bound probes and downstream encodes begun by next-window requests.
    /// A rejected request never starts FFprobe or allocates a private HLS root.
    pub(super) fn acquire_continuation(
        &self,
    ) -> Result<tokio::sync::SemaphorePermit<'_>, StatusCode> {
        self.continuations
            .try_acquire()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)
    }

    /// Limit simultaneous probe or embedded subtitle extraction work
    /// before a request enters the stricter FFmpeg publication worker.
    pub(super) fn acquire_track_probe(
        &self,
    ) -> Result<tokio::sync::SemaphorePermit<'_>, StatusCode> {
        self.track_probes
            .try_acquire()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)
    }

    /// Join at most four ongoing HLS publication requests. Once the first
    /// worker publishes, a waiter rechecks its exact cache key before
    /// considering any additional FFmpeg work. Cancellation releases both
    /// admission permits without a background worker or unbounded queue.
    pub(super) async fn acquire_generation(
        &self,
    ) -> Result<tokio::sync::SemaphorePermit<'_>, StatusCode> {
        if let Ok(permit) = self.jobs.try_acquire() {
            return Ok(permit);
        }
        let queued = self
            .waiters
            .try_acquire()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
        let permit = tokio::time::timeout(Duration::from_secs(120), self.jobs.acquire())
            .await
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        drop(queued);
        Ok(permit)
    }

    pub(super) fn lookup(
        &self,
        id: &str,
        item: &str,
        kind: &str,
        token: &str,
    ) -> Option<Arc<Published>> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = entries.iter().position(|entry| {
            entry.id == id && entry.item == item && entry.kind == kind && entry.digest == digest
        })?;
        let entry = entries.remove(index)?;
        entries.push_back(entry.clone());
        Some(entry)
    }

    /// Reuse an already completed rendition, never one owned by a sibling
    /// session or built for different source bytes/segment settings.
    pub(super) fn cached(&self, query: &CacheLookup<'_>) -> Option<Arc<Published>> {
        let digest: [u8; 32] = Sha256::digest(query.token.as_bytes()).into();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = entries.iter().rposition(|entry| {
            entry.item == query.item
                && entry.kind == query.kind
                && entry.digest == digest
                && entry.source == query.source
                && entry.source_size == query.size
                && entry.source_modified == query.modified
                && entry.target_seconds == query.target_seconds
                && entry.start_time_ticks == query.start_time_ticks
                && entry.excerpt_duration_ticks == query.excerpt_duration_ticks
                && entry.rendition_count == query.rendition_count
                && entry.subtitle_stream_index == query.subtitle_stream_index
                && entry.sidecar.as_ref() == query.sidecar
        })?;
        let entry = entries.remove(index)?;
        entries.push_back(entry.clone());
        Some(entry)
    }

    pub(super) fn publish(&self, entry: Arc<Published>) {
        let removed = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.push_back(entry);
            (entries.len() > MAX_PUBLISHED)
                .then(|| entries.pop_front())
                .flatten()
        };
        drop(removed);
    }
}
