//! Confined subtitle sidecars, embedded track selection and HLS encode negotiation.
use super::*;
use crate::streaming;
use tokio::io::AsyncReadExt;

pub(super) enum HlsSubtitle {
    Embedded(u8),
    EmbeddedPgs(Vec<u8>),
    ExternalSrt {
        bytes: Vec<u8>,
        revision: SidecarRevision,
    },
    ExternalPgs {
        bytes: Vec<u8>,
        revision: SidecarRevision,
    },
}

/// Read through the same confined descriptor path used by direct subtitle
/// delivery. No filename is passed to FFmpeg; the renderer receives only
/// validated UTF-8 bytes from this bounded read.
pub(super) async fn read_confined_srt(
    source: &Path,
    filename: &str,
) -> Result<Vec<u8>, StatusCode> {
    let parent = source.parent().ok_or(StatusCode::NOT_FOUND)?.to_path_buf();
    let filename = filename.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let (file, _, size) = streaming::open_confined(&parent, Path::new(&filename))?;
        if size == 0 || size > 4 * 1024 * 1024 {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        let mut bytes = Vec::with_capacity(size as usize);
        // Metadata is only a snapshot. A concurrent writer may extend a
        // sidecar after the size check, so bound the actual descriptor read.
        file.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if bytes.len() as u64 != size {
            return Err(StatusCode::NOT_FOUND);
        }
        let normalized = if filename.to_ascii_lowercase().ends_with(".vtt") {
            crate::subtitle_timing::vtt_sidecar_to_srt(&bytes).ok_or(StatusCode::NOT_IMPLEMENTED)?
        } else if filename.to_ascii_lowercase().ends_with(".ass") {
            if !crate::probe::is_native_ass_script(&bytes) {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
            bytes
        } else if filename.to_ascii_lowercase().ends_with(".ssa") {
            if !crate::probe::is_native_ssa_script(&bytes) {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
            bytes
        } else {
            playback::hls_normalized_srt(&bytes).ok_or(StatusCode::NOT_IMPLEMENTED)?
        };
        if normalized.len() > 4 * 1024 * 1024 {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        Ok(normalized)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

/// Read a selected graphical sidecar without changing the media or
/// admitting arbitrary filesystem paths. Packet/render compatibility is
/// verified by the bounded private PGS HLS builder before publication.
pub(super) async fn read_confined_sup(
    source: &Path,
    filename: &str,
) -> Result<Vec<u8>, StatusCode> {
    if !filename.to_ascii_lowercase().ends_with(".sup") {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    let parent = source.parent().ok_or(StatusCode::NOT_FOUND)?.to_path_buf();
    let filename = filename.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let (file, _, size) = streaming::open_confined(&parent, Path::new(&filename))?;
        if size == 0 || size > crate::pgs::MAX_SUP_BYTES {
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(crate::pgs::MAX_SUP_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if bytes.len() as u64 != size {
            return Err(StatusCode::NOT_FOUND);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

pub(super) async fn read_selected_sidecar(
    source: &Path,
    filename: &str,
) -> Result<Vec<u8>, StatusCode> {
    if filename.to_ascii_lowercase().ends_with(".sup") {
        read_confined_sup(source, filename).await
    } else {
        read_confined_srt(source, filename).await
    }
}

/// Honor a selected FFprobe global index through either an embedded
/// text-subtitle-relative position or the same deterministic external SRT
/// inventory used by PlaybackInfo. Only explicit Encode requests burn in.
pub(super) async fn validate_hls_track_selection(
    state: &AppState,
    source: &Path,
    query: &PlaylistQuery,
    kind: &str,
) -> Result<Option<HlsSubtitle>, StatusCode> {
    // The pinned v12.3 DynamicHlsController defaults SubtitleMethod to
    // External. Burn-in is only the explicit Encode (=0) delivery method;
    // never silently turn an external subtitle request into a video overlay.
    let encode = query
        .subtitle_method
        .as_deref()
        .is_some_and(|method| method.eq_ignore_ascii_case("encode") || method == "0");
    let supported_method = query
        .subtitle_method
        .as_deref()
        .is_none_or(|method| encode || method.eq_ignore_ascii_case("external") || method == "2");
    if !supported_method
        || (query.subtitle_stream_index.is_some_and(|index| index >= 0) && !encode)
        || (kind == "audio" && query.video_stream_index.is_some())
        || (kind != "video" && query.subtitle_stream_index.is_some_and(|index| index != -1))
    {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if query.audio_stream_index.is_none()
        && query.video_stream_index.is_none()
        && query.subtitle_stream_index.is_none_or(|index| index == -1)
    {
        return Ok(None);
    }
    let report = crate::probe::probe(
        source,
        state.probe_executable(),
        crate::probe::DEFAULT_TIMEOUT,
    )
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    for (stream_kind, requested) in [
        ("audio", query.audio_stream_index),
        ("video", query.video_stream_index),
    ] {
        if let Some(index) = requested {
            let first = report
                .streams
                .iter()
                .find(|stream| stream["codec_type"] == stream_kind)
                .and_then(|stream| stream["index"].as_i64());
            if first != Some(index) {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
        }
    }
    if let Some(global_index) = query.subtitle_stream_index.filter(|index| *index >= 0) {
        let track = report
            .streams
            .iter()
            .filter(|stream| stream["codec_type"] == "subtitle")
            .enumerate()
            .find(|(_, stream)| stream["index"].as_i64() == Some(global_index));
        if let Some((position, stream)) = track {
            if position > 31 {
                return Err(StatusCode::NOT_IMPLEMENTED);
            }
            if matches!(
                stream["codec_name"].as_str(),
                Some("subrip" | "ass" | "ssa")
            ) {
                return Ok(Some(HlsSubtitle::Embedded(position as u8)));
            }
            if matches!(
                stream["codec_name"].as_str(),
                Some("hdmv_pgs_subtitle" | "pgssub")
            ) {
                let bytes =
                    playback::extract_embedded_pgs(state.ffmpeg_executable(), source, global_index)
                        .await?;
                return Ok(Some(HlsSubtitle::EmbeddedPgs(bytes)));
            }
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
        if let Some(filename) = playback::hls_text_sidecar_filename(source, &report, global_index) {
            let bytes = read_confined_srt(source, &filename).await?;
            let revision = SidecarRevision {
                filename,
                digest: Sha256::digest(&bytes).into(),
            };
            return Ok(Some(HlsSubtitle::ExternalSrt { bytes, revision }));
        }
        if let Some(filename) = playback::hls_pgs_sidecar_filename(source, &report, global_index) {
            let bytes = read_confined_sup(source, &filename).await?;
            let revision = SidecarRevision {
                filename,
                digest: Sha256::digest(&bytes).into(),
            };
            return Ok(Some(HlsSubtitle::ExternalPgs { bytes, revision }));
        }
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if query.subtitle_stream_index.is_some_and(|index| index != -1) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    Ok(None)
}
