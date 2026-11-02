//! PGS-specific bounded HLS adapters and their explicit playback-window contracts.
//! Encoding, timing validation and rollback remain in the generic HLS builder.
use super::*;

/// Encode a private bounded VOD with one caller-authorized PGS SUP track.
/// Windows are split at explicit PCS composition timestamps *before* output
/// creation and are then independently decoded and measured like text HLS.
/// Only the bounded supported PGS subset can be encoded; requests whose cue
/// boundaries would generate sub-100ms segments fail closed. This API does
/// not discover tracks, authorize access, or publish client-facing manifests.
pub async fn generate_segmented_vod_with_pgs_from(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    target_seconds: u32,
    start_seconds: f64,
    sup_bytes: &[u8],
) -> io::Result<HlsSegmentedArtifact> {
    generate_segmented_vod_configured(
        controller,
        ffprobe,
        source,
        output_dir,
        target_seconds,
        start_seconds,
        SegmentedOptions {
            pgs_track: Some(sup_bytes),
            ..SegmentedOptions::default()
        },
    )
    .await
}

/// A bounded, privately generated playback excerpt from a longer
/// already-authorized source. This is not a complete VOD playlist: callers
/// must not publish it as if it represents the entire source duration.
#[derive(Debug, Clone, Copy)]
pub struct PgsPlaybackWindow<'a> {
    pub start_seconds: f64,
    pub duration_seconds: f64,
    pub sup_bytes: &'a [u8],
}

/// Build up to 600 seconds of cue-aligned private MPEG-TS HLS from a source
/// longer than 600 seconds. The full source is probed, but FFmpeg processes
/// only the requested interval; PGS preflight remains strict and every
/// emitted byte is still covered by the existing size and segment limits.
/// This helper does not perform user authorization or HTTP publication.
pub async fn generate_segmented_pgs_window(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    target_seconds: u32,
    window: PgsPlaybackWindow<'_>,
) -> io::Result<HlsSegmentedArtifact> {
    generate_segmented_vod_configured(
        controller,
        ffprobe,
        source,
        output_dir,
        target_seconds,
        window.start_seconds,
        SegmentedOptions {
            pgs_track: Some(window.sup_bytes),
            window_duration_seconds: Some(window.duration_seconds),
            ..SegmentedOptions::default()
        },
    )
    .await
}

/// A private, bounded excerpt with 2–4 aligned HLS quality variants from
/// a longer PGS-bearing source. It is not a complete VOD master playlist.
#[derive(Debug, Clone, Copy)]
pub struct AdaptivePgsPlaybackWindow<'a> {
    pub start_seconds: f64,
    pub duration_seconds: f64,
    pub qualities: &'a [MpegTsQuality],
    pub sup_bytes: &'a [u8],
}

/// Encode only a finite excerpt through the existing aligned adaptive
/// rendition builder. Neither authentication nor publication is handled here.
pub async fn generate_adaptive_segmented_pgs_window(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    target_seconds: u32,
    window: AdaptivePgsPlaybackWindow<'_>,
) -> io::Result<HlsAdaptiveSegmentedArtifact> {
    generate_adaptive_segmented_vod_configured(
        controller,
        ffprobe,
        source,
        output_dir,
        target_seconds,
        AdaptiveWindow {
            start_seconds: window.start_seconds,
            qualities: window.qualities,
            subtitle: Some(AdaptiveSubtitle::Pgs(window.sup_bytes)),
        },
        Some(window.duration_seconds),
    )
    .await
}
