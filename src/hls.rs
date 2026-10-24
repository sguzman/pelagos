//! Minimal bounded single-segment HLS VOD artifact from a verified MPEG-TS
//! transcode. Adaptive rendition sets, seeking and Jellyfin HLS routes are
//! not implemented here. Do not advertise general HLS client compatibility.
use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::{
    probe,
    transcode::{MpegTsQuality, TranscodeController},
};

const SEGMENT_NAME: &str = "segment00000.ts";
const PLAYLIST_NAME: &str = "index.m3u8";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

const MAX_VOD_SEGMENTS: usize = 512;

/// A completed independently decodable MPEG-TS segment, with duration
/// measured from the segment rather than guessed from FFmpeg target time.
#[derive(Debug, Clone, Copy)]
pub struct HlsSegment {
    pub duration_seconds: f64,
    pub transport_bytes: u64,
}

/// Serialize a bounded VOD media playlist only after every MPEG-TS segment
/// has passed probing/packet validation. Filenames are generated internally,
/// never accepted from clients or arbitrary filesystem content.
///
/// This function does not perform segment muxing; callers must independently
/// verify that all supplied TS segments begin at usable decoder boundaries.
pub fn render_vod_media_playlist(segments: &[HlsSegment]) -> io::Result<String> {
    render_vod_media_playlist_inner(segments, false)
}

/// Independently re-encoded FFmpeg windows restart MPEG-TS program timestamps.
/// Signal that reset before each next segment so the player can recover the
/// stream timeline without assuming one continuous PTS sequence.
fn render_vod_media_playlist_inner(
    segments: &[HlsSegment],
    independent_windows: bool,
) -> io::Result<String> {
    if segments.is_empty() || segments.len() > MAX_VOD_SEGMENTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid HLS segment count",
        ));
    }
    let mut max_duration = 0.0_f64;
    let mut total = 0.0_f64;
    for segment in segments {
        if !segment.duration_seconds.is_finite()
            || !(0.001..=86_400.0).contains(&segment.duration_seconds)
            || segment.transport_bytes < 188
            || segment.transport_bytes % 188 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid HLS segment metadata",
            ));
        }
        total += segment.duration_seconds;
        max_duration = max_duration.max(segment.duration_seconds);
        if total > 86_400.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HLS VOD exceeds one day",
            ));
        }
    }
    let mut playlist = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n",
        max_duration.ceil() as u32
    );
    for (index, segment) in segments.iter().enumerate() {
        if independent_windows && index > 0 {
            playlist.push_str("#EXT-X-DISCONTINUITY\n");
        }
        playlist.push_str(&format!(
            "#EXTINF:{:.3},\nsegment{index:05}.ts\n",
            segment.duration_seconds
        ));
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    Ok(playlist)
}

struct PartialBundle {
    dir: PathBuf,
    committed: bool,
}

impl Drop for PartialBundle {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Metadata for a completed, independently decodable HLS rendition.
/// Bandwidth must come from measured or safely bounded media output, not a
/// guessed codec profile. This does not create or transcode the rendition.
#[derive(Debug, Clone)]
pub struct HlsVariant {
    pub playlist_name: String,
    pub bandwidth: u64,
    pub resolution: Option<(u32, u32)>,
}

/// Render only trusted relative variant playlists and increasing, measured
/// bandwidth values. No absolute URLs, path traversal or attacker-authored
/// playlist paths can enter this HLS master playlist.
pub fn render_vod_master_playlist(variants: &[HlsVariant]) -> io::Result<String> {
    if variants.is_empty() || variants.len() > 8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid variant count",
        ));
    }
    let mut master = String::from("#EXTM3U\n#EXT-X-VERSION:3\n");
    let mut previous_bandwidth = 0_u64;
    for (index, variant) in variants.iter().enumerate() {
        let expected = format!("variant{index:02}/index.m3u8");
        if variant.playlist_name != expected
            || variant.bandwidth == 0
            || variant.bandwidth > 1_000_000_000
            || variant.bandwidth <= previous_bandwidth
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid HLS rendition path or bandwidth order",
            ));
        }
        if let Some((width, height)) = variant.resolution {
            if !(16..=16_384).contains(&width) || !(16..=16_384).contains(&height) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid HLS rendition resolution",
                ));
            }
            master.push_str(&format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={width}x{height}\n",
                variant.bandwidth
            ));
        } else {
            master.push_str(&format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={}\n",
                variant.bandwidth
            ));
        }
        master.push_str(&variant.playlist_name);
        master.push('\n');
        previous_bandwidth = variant.bandwidth;
    }
    Ok(master)
}

#[derive(Debug, Clone)]
pub struct HlsVodArtifact {
    pub playlist_path: PathBuf,
    pub segment_path: PathBuf,
    pub duration_seconds: f64,
    pub transport_bytes: u64,
    pub resolution: Option<(u32, u32)>,
}

/// Create a new, isolated directory containing a complete, single-segment
/// HLS VOD playlist. No pre-existing directory is ever overwritten. FFmpeg's
/// transport bytes are capped by the existing Rust stdout writer; FFprobe
/// validates the completed transport duration before publishing a manifest.
///
/// Cancelling the future or failing either external process removes the new
/// directory. The caller owns successful artifact cleanup.
pub async fn generate_single_segment_vod(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
) -> io::Result<HlsVodArtifact> {
    generate_single_segment_vod_with_quality(controller, ffprobe, source, output_dir, None).await
}

async fn generate_single_segment_vod_with_quality(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    quality: Option<MpegTsQuality>,
) -> io::Result<HlsVodArtifact> {
    // Create the directory synchronously before the first suspension point,
    // ensuring the drop guard exists whenever cancellation is possible.
    std::fs::create_dir(output_dir)?;
    let mut partial = PartialBundle {
        dir: output_dir.to_path_buf(),
        committed: false,
    };
    let segment_path = output_dir.join(SEGMENT_NAME);
    let transport_bytes = match quality {
        Some(profile) => {
            controller
                .try_run_with_quality(source, &segment_path, profile)
                .await?
        }
        None => controller.try_run(source, &segment_path).await?,
    };
    let inspection = probe::probe(&segment_path, ffprobe, PROBE_TIMEOUT).await?;
    let duration_seconds = inspection
        .format
        .as_ref()
        .and_then(|format| format["duration"].as_str())
        .and_then(|duration| duration.parse::<f64>().ok())
        .filter(|duration| duration.is_finite() && (0.001..=86_400.0).contains(duration))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "transport has no finite, positive bounded duration",
            )
        })?;
    let resolution = inspection.streams.iter().find_map(|stream| {
        if stream["codec_type"].as_str() != Some("video") {
            return None;
        }
        let width = u32::try_from(stream["width"].as_u64()?).ok()?;
        let height = u32::try_from(stream["height"].as_u64()?).ok()?;
        ((16..=16_384).contains(&width) && (16..=16_384).contains(&height))
            .then_some((width, height))
    });
    if transport_bytes < 188 || transport_bytes % 188 != 0 || inspection.streams.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transport must contain MPEG-TS packets and stream metadata",
        ));
    }
    let playlist = render_vod_media_playlist(&[HlsSegment {
        duration_seconds,
        transport_bytes,
    }])?;
    let playlist_path = output_dir.join(PLAYLIST_NAME);
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&playlist_path)
        .await?;
    out.write_all(playlist.as_bytes()).await?;
    out.flush().await?;
    partial.committed = true;
    Ok(HlsVodArtifact {
        playlist_path,
        segment_path,
        duration_seconds,
        transport_bytes,
        resolution,
    })
}

/// A bounded, independently encoded series of MPEG-TS VOD segments.
/// The published playlist references only the generated relative filenames.
/// Consumers own cleanup; there is no public HTTP HLS route in this subset.
#[derive(Debug, Clone)]
pub struct HlsSegmentedArtifact {
    pub playlist_path: PathBuf,
    pub segment_paths: Vec<PathBuf>,
    pub segments: Vec<HlsSegment>,
    pub transport_bytes: u64,
    root: PathBuf,
}

impl HlsSegmentedArtifact {
    pub fn remove(self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.root)
    }
}

const MAX_ENCODED_SEGMENTS: usize = 64;
const MAX_SEGMENTED_BYTES: u64 = 128 * 1024 * 1024;

/// Produce short independently encoded TS segments from authenticated,
/// caller-authorized local media. The input is probed first; offsets, segment
/// count, total duration, bytes and process lifetimes are bounded. Each
/// segment is re-encoded in a separate FFmpeg process so it starts at a fresh
/// decoder boundary, then independently probed and checked as MPEG-TS.
/// All partial files disappear after any error or cancelled Rust future.
///
/// This is a private VOD artifact, NOT a client-facing HLS scheduler/route.
pub async fn generate_segmented_vod(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    target_seconds: u32,
) -> io::Result<HlsSegmentedArtifact> {
    generate_segmented_vod_with_quality(
        controller,
        ffprobe,
        source,
        output_dir,
        target_seconds,
        None,
    )
    .await
}

async fn generate_segmented_vod_with_quality(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    target_seconds: u32,
    quality: Option<MpegTsQuality>,
) -> io::Result<HlsSegmentedArtifact> {
    if !(1..=10).contains(&target_seconds) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid HLS segment target",
        ));
    }
    let inspected = probe::probe(source, ffprobe, PROBE_TIMEOUT).await?;
    let source_seconds = inspected
        .format
        .as_ref()
        .and_then(|format| format["duration"].as_str())
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && (0.1..=600.0).contains(seconds))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unbounded source duration"))?;
    if inspected.streams.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source has no audio or video",
        ));
    }
    let mut segment_count = (source_seconds / f64::from(target_seconds)).ceil() as usize;
    // A tiny final remainder belongs to the preceding full window.
    let final_remainder =
        source_seconds - (segment_count.saturating_sub(1) as f64 * f64::from(target_seconds));
    if segment_count > 1 && final_remainder < 0.1 {
        segment_count -= 1;
    }
    if segment_count == 0 || segment_count > MAX_ENCODED_SEGMENTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HLS window count exceeds limit",
        ));
    }
    // The synchronous create + guard ensures cancellation cleans the entire
    // private output tree, even while FFmpeg is still producing one window.
    std::fs::create_dir(output_dir)?;
    let mut partial = PartialBundle {
        dir: output_dir.to_path_buf(),
        committed: false,
    };
    let mut segments = Vec::with_capacity(segment_count);
    let mut segment_paths = Vec::with_capacity(segment_count);
    let mut total_bytes = 0_u64;
    for index in 0..segment_count {
        let start = index as f64 * f64::from(target_seconds);
        let remaining = source_seconds - start;
        if remaining <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty HLS time window",
            ));
        }
        let target = f64::from(target_seconds);
        // Avoid sub-100ms tail jobs by adding short remainders to the
        // preceding segment, still well under the 30s FFmpeg window cap.
        let duration = if remaining > target && remaining - target < 0.1 {
            remaining
        } else {
            remaining.min(target)
        };
        if duration < 0.1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "short HLS tail window",
            ));
        }
        let path = output_dir.join(format!("segment{index:05}.ts"));
        let size = match quality {
            Some(target) => {
                controller
                    .try_run_window_with_quality(source, &path, start, duration, target)
                    .await?
            }
            None => {
                controller
                    .try_run_window(source, &path, start, duration)
                    .await?
            }
        };
        total_bytes = total_bytes.checked_add(size).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "HLS output byte overflow")
        })?;
        if total_bytes > MAX_SEGMENTED_BYTES || size < 188 || size % 188 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid HLS output bytes",
            ));
        }
        // Verify TS sync markers in constant memory, rather than trusting
        // an extension or the FFmpeg exit code.
        let mut packet_reader = fs::File::open(&path).await?;
        let mut packet = [0_u8; 188];
        for _ in 0..size / 188 {
            packet_reader.read_exact(&mut packet).await?;
            if packet[0] != 0x47 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid TS sync packet",
                ));
            }
        }
        let report = probe::probe(&path, ffprobe, PROBE_TIMEOUT).await?;
        if report.streams.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unprobeable HLS segment",
            ));
        }
        let measured_seconds = report
            .format
            .as_ref()
            .and_then(|format| format["duration"].as_str())
            .and_then(|raw| raw.parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite() && (0.001..=30.0).contains(seconds))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid segment duration")
            })?;
        segments.push(HlsSegment {
            duration_seconds: measured_seconds,
            transport_bytes: size,
        });
        segment_paths.push(path);
    }
    let manifest = render_vod_media_playlist_inner(&segments, true)?;
    let playlist_path = output_dir.join(PLAYLIST_NAME);
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&playlist_path)
        .await?;
    out.write_all(manifest.as_bytes()).await?;
    out.flush().await?;
    partial.committed = true;
    Ok(HlsSegmentedArtifact {
        playlist_path,
        segment_paths,
        segments,
        transport_bytes: total_bytes,
        root: output_dir.to_path_buf(),
    })
}

/// Completed adaptive multi-segment renditions with measured bandwidths.
/// These are private artifacts, not yet authenticated Jellyfin HLS endpoints.
#[derive(Debug, Clone)]
pub struct HlsAdaptiveSegmentedArtifact {
    pub master_path: PathBuf,
    pub variants: Vec<HlsVariant>,
    pub renditions: Vec<HlsSegmentedArtifact>,
    root: PathBuf,
}

impl HlsAdaptiveSegmentedArtifact {
    pub fn remove(self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.root)
    }
}

/// Produce 2–4 independently encoded, short-segment HLS VOD renditions.
/// Publish the master only after all requested outputs are verified.
/// Variant order follows measured transport bandwidth, not input targets.
pub async fn generate_adaptive_segmented_vod(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    target_seconds: u32,
    qualities: &[MpegTsQuality],
) -> io::Result<HlsAdaptiveSegmentedArtifact> {
    if !(2..=4).contains(&qualities.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid adaptive segmented rendition count",
        ));
    }
    std::fs::create_dir(output_dir)?;
    let mut partial = PartialBundle {
        dir: output_dir.to_path_buf(),
        committed: false,
    };
    let mut candidates = Vec::new();
    for (index, quality) in qualities.iter().enumerate() {
        let name = format!("candidate{index:02}");
        let rendition = generate_segmented_vod_with_quality(
            controller,
            ffprobe,
            source,
            &output_dir.join(&name),
            target_seconds,
            Some(*quality),
        )
        .await?;
        let duration: f64 = rendition
            .segments
            .iter()
            .map(|segment| segment.duration_seconds)
            .sum();
        let bandwidth = (rendition.transport_bytes as f64 * 8.0 / duration).ceil() as u64;
        if duration <= 0.0 || bandwidth == 0 || bandwidth > 1_000_000_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "measured segmented rendition bandwidth is invalid",
            ));
        }
        candidates.push((bandwidth, name, rendition));
    }
    candidates.sort_by_key(|candidate| candidate.0);
    let mut variants = Vec::new();
    let mut renditions = Vec::new();
    let mut previous = 0_u64;
    for (bandwidth, original, rendition) in candidates {
        let old_path = output_dir.join(&original);
        if previous == bandwidth {
            fs::remove_dir_all(old_path).await?;
            continue;
        }
        let name = format!("variant{:02}", variants.len());
        let dir = output_dir.join(&name);
        fs::rename(&old_path, &dir).await?;
        variants.push(HlsVariant {
            playlist_name: format!("{name}/index.m3u8"),
            bandwidth,
            resolution: None,
        });
        renditions.push(HlsSegmentedArtifact {
            playlist_path: dir.join(PLAYLIST_NAME),
            segment_paths: rendition
                .segment_paths
                .iter()
                .enumerate()
                .map(|(index, _)| dir.join(format!("segment{index:05}.ts")))
                .collect(),
            segments: rendition.segments,
            transport_bytes: rendition.transport_bytes,
            root: dir,
        });
        previous = bandwidth;
    }
    let master = render_vod_master_playlist(&variants)?;
    let master_path = output_dir.join("master.m3u8");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&master_path)
        .await?;
    file.write_all(master.as_bytes()).await?;
    file.flush().await?;
    partial.committed = true;
    Ok(HlsAdaptiveSegmentedArtifact {
        master_path,
        variants,
        renditions,
        root: output_dir.to_path_buf(),
    })
}

/// A single bounded VOD rendition and its measured peak segment bandwidth.
/// This first stage contains one segment per rendition, not yet rolling HLS.
#[derive(Debug, Clone)]
pub struct HlsAdaptiveArtifact {
    pub master_path: PathBuf,
    pub variants: Vec<HlsVariant>,
    pub renditions: Vec<HlsVodArtifact>,
    root: PathBuf,
}

impl HlsAdaptiveArtifact {
    /// Release a finished private transcode bundle when a future session or
    /// cache stops using it. The deletion root cannot be replaced by callers.
    pub fn remove(self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.root)
    }
}

/// Generate 2–4 independent CPU bitrate-target renditions and a master
/// playlist ordered by measured transport bandwidth, not requested targets.
/// Equal-bandwidth duplicates are removed instead of inventing differences.
/// Every output is private and atomically created; any error rolls the
/// bundle directory back, including partially completed variants.
pub async fn generate_adaptive_vod(
    controller: &TranscodeController,
    ffprobe: &Path,
    source: &Path,
    output_dir: &Path,
    qualities: &[MpegTsQuality],
) -> io::Result<HlsAdaptiveArtifact> {
    if !(2..=4).contains(&qualities.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HLS ladder requires two to four requested renditions",
        ));
    }
    std::fs::create_dir(output_dir)?;
    let mut partial = PartialBundle {
        dir: output_dir.to_path_buf(),
        committed: false,
    };
    let mut staged = Vec::new();
    for (index, quality) in qualities.iter().enumerate() {
        let name = format!("candidate{index:02}");
        let rendition = generate_single_segment_vod_with_quality(
            controller,
            ffprobe,
            source,
            &output_dir.join(&name),
            Some(*quality),
        )
        .await?;
        let bandwidth =
            (rendition.transport_bytes as f64 * 8.0 / rendition.duration_seconds).ceil() as u64;
        if bandwidth == 0 || bandwidth > 1_000_000_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "measured HLS segment bandwidth is invalid",
            ));
        }
        staged.push((bandwidth, name, rendition));
    }
    staged.sort_by_key(|(bandwidth, _, _)| *bandwidth);
    let mut variants = Vec::new();
    let mut renditions = Vec::new();
    let mut previous = 0_u64;
    for (bandwidth, name, rendition) in staged {
        let previous_path = output_dir.join(&name);
        if bandwidth == previous {
            fs::remove_dir_all(previous_path).await?;
            continue;
        }
        let new_name = format!("variant{:02}", variants.len());
        fs::rename(&previous_path, output_dir.join(&new_name)).await?;
        let playlist_name = format!("{new_name}/index.m3u8");
        variants.push(HlsVariant {
            playlist_name,
            bandwidth,
            resolution: rendition.resolution,
        });
        renditions.push(HlsVodArtifact {
            playlist_path: output_dir.join(&new_name).join(PLAYLIST_NAME),
            segment_path: output_dir.join(&new_name).join(SEGMENT_NAME),
            ..rendition
        });
        previous = bandwidth;
    }
    let manifest = render_vod_master_playlist(&variants)?;
    let master_path = output_dir.join("master.m3u8");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&master_path)
        .await?;
    file.write_all(manifest.as_bytes()).await?;
    file.flush().await?;
    partial.committed = true;
    Ok(HlsAdaptiveArtifact {
        master_path,
        variants,
        renditions,
        root: output_dir.to_path_buf(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs as stdfs, os::unix::fs::PermissionsExt};

    fn executable(dir: &Path, name: &str, script: &str) -> PathBuf {
        let path = dir.join(name);
        stdfs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn measured_multi_segment_manifest_has_bounded_independent_uris_and_ceil_target() {
        let segments = vec![
            HlsSegment {
                duration_seconds: 1.5,
                transport_bytes: 376,
            },
            HlsSegment {
                duration_seconds: 2.125,
                transport_bytes: 752,
            },
            HlsSegment {
                duration_seconds: 0.5,
                transport_bytes: 188,
            },
        ];
        assert_eq!(
            render_vod_media_playlist(&segments).unwrap(),
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:3\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:1.500,\nsegment00000.ts\n#EXTINF:2.125,\nsegment00001.ts\n#EXTINF:0.500,\nsegment00002.ts\n#EXT-X-ENDLIST\n"
        );
        let single = render_vod_media_playlist(&[HlsSegment {
            duration_seconds: 6.125,
            transport_bytes: 376,
        }])
        .unwrap();
        assert!(single.contains("#EXT-X-TARGETDURATION:7\n"));
        assert!(single.contains("#EXTINF:6.125,\nsegment00000.ts\n"));
    }

    #[test]
    fn manifest_refuses_unverified_segment_bytes_and_duration_overflow() {
        let valid = HlsSegment {
            duration_seconds: 1.0,
            transport_bytes: 376,
        };
        assert_eq!(
            render_vod_media_playlist(&[]).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            render_vod_media_playlist(&vec![valid; 513])
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        for duration_seconds in [f64::NAN, f64::INFINITY, -1.0, 0.0, 86_401.0] {
            assert_eq!(
                render_vod_media_playlist(&[HlsSegment {
                    duration_seconds,
                    ..valid
                }])
                .unwrap_err()
                .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for transport_bytes in [0, 187, 189, 377] {
            assert_eq!(
                render_vod_media_playlist(&[HlsSegment {
                    transport_bytes,
                    ..valid
                }])
                .unwrap_err()
                .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(
            render_vod_media_playlist(&[
                HlsSegment {
                    duration_seconds: 50_000.0,
                    ..valid
                },
                HlsSegment {
                    duration_seconds: 40_000.0,
                    ..valid
                }
            ])
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[tokio::test]
    async fn private_independent_segments_publish_measured_playlist_and_remove_on_request() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        stdfs::write(&source, b"synthetic").unwrap();
        let encoder = executable(
            temp.path(),
            "fake-ffmpeg",
            "for i in 1 2; do printf '\\107'; head -c 187 /dev/zero; done",
        );
        let inspector = executable(
            temp.path(),
            "fake-ffprobe",
            r#"case "$*" in *segment*) printf '%s' '{"format":{"duration":"2.0"},"streams":[{"codec_type":"audio","codec_name":"aac"}]}' ;; *) printf '%s' '{"format":{"duration":"4.05"},"streams":[{"codec_type":"audio","codec_name":"pcm_s16le"}]}' ;; esac"#,
        );
        let runner = TranscodeController::new(encoder, 1, Duration::from_secs(4), 1024).unwrap();
        let dir = temp.path().join("segments");
        let bundle = generate_segmented_vod(&runner, &inspector, &source, &dir, 2)
            .await
            .unwrap();
        assert_eq!(bundle.segment_paths.len(), 2);
        assert_eq!(bundle.transport_bytes, 752);
        for path in &bundle.segment_paths {
            assert_eq!(stdfs::metadata(path).unwrap().len(), 376);
            assert_eq!(stdfs::read(path).unwrap()[0], 0x47);
        }
        let manifest = stdfs::read_to_string(&bundle.playlist_path).unwrap();
        assert!(manifest.contains("#EXT-X-TARGETDURATION:2\n"));
        assert!(manifest.contains("#EXTINF:2.000,\nsegment00000.ts\n"));
        assert!(manifest.contains("#EXTINF:2.000,\nsegment00001.ts\n"));
        assert!(manifest.contains(
            "#EXT-X-DISCONTINUITY\n#EXTINF:2.000,\nsegment00001.ts\n"
        ));
        assert!(manifest.ends_with("#EXT-X-ENDLIST\n"));
        assert_eq!(stdfs::read(&source).unwrap(), b"synthetic");
        bundle.remove().unwrap();
        assert!(!dir.exists());
        assert_eq!(
            generate_segmented_vod(&runner, &inspector, &source, temp.path(), 2)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[tokio::test]
    async fn segmented_hls_rejects_invalid_jobs_and_rolls_back_midstream() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("audio.wav");
        stdfs::write(&source, b"synthetic").unwrap();
        let encoder = executable(
            temp.path(),
            "fake-ffmpeg",
            "for i in 1 2; do printf '\\107'; head -c 187 /dev/zero; done",
        );
        let fail_second = executable(
            temp.path(),
            "fail-second-ffprobe",
            r#"case "$*" in *segment00001*) exit 7 ;; *segment*) printf '%s' '{"format":{"duration":"2.0"},"streams":[{"codec_type":"audio"}]}' ;; *) printf '%s' '{"format":{"duration":"4.0"},"streams":[{"codec_type":"audio"}]}' ;; esac"#,
        );
        let runner = TranscodeController::new(encoder, 1, Duration::from_secs(4), 1024).unwrap();
        for target in [0, 11] {
            let dir = temp.path().join(format!("invalid-{target}"));
            assert_eq!(
                generate_segmented_vod(&runner, &fail_second, &source, &dir, target)
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(!dir.exists());
        }
        let bundle_dir = temp.path().join("rollback");
        assert!(
            generate_segmented_vod(&runner, &fail_second, &source, &bundle_dir, 2)
                .await
                .is_err()
        );
        assert!(!bundle_dir.exists());
    }

    #[tokio::test]
    async fn bounded_playlist_uses_verified_duration_and_relative_segment_name() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.mp4");
        stdfs::write(&input, b"synthetic").unwrap();
        let ffmpeg = executable(temp.path(), "fake-ffmpeg", "head -c 376 /dev/zero");
        let ffprobe = executable(
            temp.path(),
            "fake-ffprobe",
            r#"printf '%s' '{"format":{"duration":"6.125"},"streams":[{"codec_name":"aac"}]}'"#,
        );
        let controller = TranscodeController::new(ffmpeg, 1, Duration::from_secs(2), 1024).unwrap();
        let bundle = temp.path().join("vod");
        let info = generate_single_segment_vod(&controller, &ffprobe, &input, &bundle)
            .await
            .unwrap();
        assert_eq!(info.transport_bytes, 376);
        assert_eq!(info.duration_seconds, 6.125);
        assert_eq!(stdfs::metadata(info.segment_path).unwrap().len(), 376);
        let manifest = stdfs::read_to_string(info.playlist_path).unwrap();
        assert!(manifest.starts_with("#EXTM3U\n#EXT-X-VERSION:3\n"));
        assert!(manifest.contains("#EXT-X-TARGETDURATION:7\n"));
        assert!(manifest.contains("#EXTINF:6.125,\nsegment00000.ts\n"));
        assert!(manifest.ends_with("#EXT-X-ENDLIST\n"));
        assert!(!manifest.contains(input.to_str().unwrap()));
        assert_eq!(stdfs::read(input).unwrap(), b"synthetic");
        assert_eq!(
            generate_single_segment_vod(
                &controller,
                &ffprobe,
                temp.path().join("input.mp4").as_path(),
                &bundle,
            )
            .await
            .unwrap_err()
            .kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[tokio::test]
    async fn invalid_probe_result_or_overflow_removes_entire_partial_vod_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.mp4");
        stdfs::write(&input, b"synthetic").unwrap();
        let ffmpeg = executable(temp.path(), "fake-ffmpeg", "head -c 376 /dev/zero");
        let ffprobe = executable(
            temp.path(),
            "fake-ffprobe",
            r#"printf '%s' '{"format":{"duration":"NaN"},"streams":[{"codec_name":"aac"}]}'"#,
        );
        let controller =
            TranscodeController::new(ffmpeg.clone(), 1, Duration::from_secs(2), 1024).unwrap();
        let invalid_dir = temp.path().join("invalid");
        let error = generate_single_segment_vod(&controller, &ffprobe, &input, &invalid_dir)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!invalid_dir.exists());
        let too_small = TranscodeController::new(ffmpeg, 1, Duration::from_secs(2), 100).unwrap();
        let overflow_dir = temp.path().join("overflow");
        let error = generate_single_segment_vod(&too_small, &ffprobe, &input, &overflow_dir)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!overflow_dir.exists());
    }

    #[tokio::test]
    async fn malformed_packet_count_or_absent_streams_cannot_publish_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.wav");
        stdfs::write(&input, b"synthetic").unwrap();
        let bad_packets = executable(temp.path(), "fake-ffmpeg", "head -c 190 /dev/zero");
        let good_probe = executable(
            temp.path(),
            "fake-ffprobe",
            r#"printf '%s' '{"format":{"duration":"2.5"},"streams":[{"codec_name":"aac"}]}'"#,
        );
        let c = TranscodeController::new(bad_packets, 1, Duration::from_secs(2), 1024).unwrap();
        let bad_dir = temp.path().join("malformed");
        assert_eq!(
            generate_single_segment_vod(&c, &good_probe, &input, &bad_dir)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!bad_dir.exists());

        let good_packets = executable(temp.path(), "good-ffmpeg", "head -c 376 /dev/zero");
        let no_streams = executable(
            temp.path(),
            "empty-ffprobe",
            r#"printf '%s' '{"format":{"duration":"2.5"},"streams":[]}'"#,
        );
        let c = TranscodeController::new(good_packets, 1, Duration::from_secs(2), 1024).unwrap();
        let empty_dir = temp.path().join("no-streams");
        assert_eq!(
            generate_single_segment_vod(&c, &no_streams, &input, &empty_dir)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!empty_dir.exists());
    }

    #[tokio::test]
    async fn aborted_bundle_job_removes_private_directory_before_completion() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("input.wav");
        stdfs::write(&input, b"synthetic").unwrap();
        let long_running = executable(temp.path(), "sleep-ffmpeg", "exec sleep 2");
        let fake_probe = executable(temp.path(), "fake-ffprobe", "exit 1");
        let controller =
            TranscodeController::new(long_running, 1, Duration::from_secs(5), 1024).unwrap();
        let dir = temp.path().join("cancelled");
        let job_dir = dir.clone();
        let job = tokio::spawn(async move {
            generate_single_segment_vod(&controller, &fake_probe, &input, &job_dir).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !dir.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        job.abort();
        assert!(job.await.is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn hls_master_validates_bitrate_order_and_fixed_relative_variant_uris() {
        let variants = vec![
            HlsVariant {
                playlist_name: "variant00/index.m3u8".to_owned(),
                bandwidth: 100_000,
                resolution: None,
            },
            HlsVariant {
                playlist_name: "variant01/index.m3u8".to_owned(),
                bandwidth: 1_500_000,
                resolution: Some((1280, 720)),
            },
        ];
        let manifest = render_vod_master_playlist(&variants).unwrap();
        assert_eq!(
            manifest,
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH=100000\nvariant00/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=1500000,RESOLUTION=1280x720\nvariant01/index.m3u8\n"
        );
        for invalid in [
            vec![],
            vec![HlsVariant {
                playlist_name: "../index.m3u8".to_owned(),
                ..variants[0].clone()
            }],
            vec![HlsVariant {
                bandwidth: 0,
                ..variants[0].clone()
            }],
            vec![HlsVariant {
                bandwidth: 1_000_000_001,
                ..variants[0].clone()
            }],
            vec![HlsVariant {
                resolution: Some((1, 720)),
                ..variants[0].clone()
            }],
            vec![variants[1].clone(), variants[0].clone()],
            vec![variants[0].clone(), variants[0].clone()],
        ] {
            assert_eq!(
                render_vod_master_playlist(&invalid).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[tokio::test]
    async fn adaptive_bundle_orders_measured_variants_and_rejects_empty_ladders() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        stdfs::write(&source, b"fixture").unwrap();
        let ffmpeg = executable(
            temp.path(),
            "fake-ffmpeg",
            "for arg in \"$@\"; do if [ \"$arg\" = \"64k\" ]; then head -c 376 /dev/zero; exit 0; fi; done; head -c 752 /dev/zero",
        );
        let ffprobe = executable(
            temp.path(),
            "fake-ffprobe",
            r#"printf '%s' '{"format":{"duration":"2.0"},"streams":[{"codec_type":"video","codec_name":"h264","width":320,"height":180}]}'"#,
        );
        let controller = TranscodeController::new(ffmpeg, 1, Duration::from_secs(3), 1024).unwrap();
        let profiles = [
            MpegTsQuality {
                video_kbps: 1500,
                audio_kbps: 128,
            },
            MpegTsQuality {
                video_kbps: 300,
                audio_kbps: 64,
            },
        ];
        let dir = temp.path().join("adaptive");
        let result = generate_adaptive_vod(&controller, &ffprobe, &source, &dir, &profiles)
            .await
            .unwrap();
        assert_eq!(result.variants.len(), 2);
        assert_eq!(result.renditions.len(), 2);
        assert_eq!(result.variants[0].bandwidth, 1504);
        assert_eq!(result.variants[0].resolution, Some((320, 180)));
        assert_eq!(result.variants[1].bandwidth, 3008);
        assert_eq!(result.variants[1].resolution, Some((320, 180)));
        assert_eq!(result.variants[0].playlist_name, "variant00/index.m3u8");
        assert_eq!(result.variants[1].playlist_name, "variant01/index.m3u8");
        assert_eq!(
            stdfs::metadata(&result.renditions[0].segment_path)
                .unwrap()
                .len(),
            376
        );
        assert_eq!(
            stdfs::metadata(&result.renditions[1].segment_path)
                .unwrap()
                .len(),
            752
        );
        assert!(!dir.join("candidate00").exists());
        assert!(!dir.join("candidate01").exists());
        let master = stdfs::read_to_string(&result.master_path).unwrap();
        assert!(master.contains("BANDWIDTH=1504,RESOLUTION=320x180\nvariant00/index.m3u8\n"));
        assert!(master.contains("BANDWIDTH=3008,RESOLUTION=320x180\nvariant01/index.m3u8\n"));
        assert_eq!(
            generate_adaptive_vod(&controller, &ffprobe, &source, &dir, &profiles)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            generate_adaptive_vod(
                &controller,
                &ffprobe,
                &source,
                &temp.path().join("missing-profiles"),
                &profiles[..1],
            )
            .await
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
        result.remove().unwrap();
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn invalid_adaptive_rendition_rolls_back_previously_written_segments() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        stdfs::write(&source, b"fixture").unwrap();
        let ffmpeg = executable(temp.path(), "fake-ffmpeg", "head -c 376 /dev/zero");
        let ffprobe = executable(
            temp.path(),
            "fake-ffprobe",
            r#"printf '%s' '{"format":{"duration":"2.0"},"streams":[{"codec_name":"aac"}]}'"#,
        );
        let controller = TranscodeController::new(ffmpeg, 1, Duration::from_secs(3), 1024).unwrap();
        let profiles = [
            MpegTsQuality {
                video_kbps: 500,
                audio_kbps: 64,
            },
            MpegTsQuality {
                video_kbps: 500,
                audio_kbps: 1,
            },
        ];
        let dir = temp.path().join("rollback");
        assert_eq!(
            generate_adaptive_vod(&controller, &ffprobe, &source, &dir, &profiles)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn cancelling_segmented_job_removes_partial_private_manifest_and_transport() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        stdfs::write(&source, b"synthetic").unwrap();
        let encoder = executable(temp.path(), "sleeping-ffmpeg", "exec sleep 2");
        let inspector = executable(
            temp.path(),
            "fake-ffprobe",
            r#"printf '%s' '{"format":{"duration":"4.0"},"streams":[{"codec_type":"audio"}]}'"#,
        );
        let controller =
            TranscodeController::new(encoder, 1, Duration::from_secs(5), 1024).unwrap();
        let output = temp.path().join("cancelled");
        let destination = output.clone();
        let job = tokio::spawn(async move {
            generate_segmented_vod(&controller, &inspector, &source, &destination, 2).await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !output.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        job.abort();
        assert!(job.await.is_err());
        assert!(!output.exists());
        assert!(!output.join("index.m3u8").exists());
    }

    #[tokio::test]
    async fn measured_multi_segment_adaptive_ladder_orders_variants_and_rolls_back() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("sound.wav");
        stdfs::write(&source, b"synthetic").unwrap();
        let encoder = executable(
            temp.path(),
            "fake-ffmpeg",
            "for arg in \"$@\"; do if [ \"$arg\" = \"64k\" ]; then for i in 1 2; do printf '\\107'; head -c 187 /dev/zero; done; exit 0; fi; done; for i in 1 2 3 4; do printf '\\107'; head -c 187 /dev/zero; done",
        );
        let inspector = executable(
            temp.path(),
            "fake-ffprobe",
            r#"case "$*" in *segment*) printf '%s' '{"format":{"duration":"2.0"},"streams":[{"codec_type":"audio","codec_name":"aac"}]}' ;; *) printf '%s' '{"format":{"duration":"4.0"},"streams":[{"codec_type":"audio","codec_name":"pcm_s16le"}]}' ;; esac"#,
        );
        let controller =
            TranscodeController::new(encoder, 1, Duration::from_secs(4), 1024).unwrap();
        let high = MpegTsQuality {
            video_kbps: 1200,
            audio_kbps: 128,
        };
        let low = MpegTsQuality {
            video_kbps: 300,
            audio_kbps: 64,
        };
        let dir = temp.path().join("adaptive");
        let result = generate_adaptive_segmented_vod(
            &controller,
            &inspector,
            &source,
            &dir,
            2,
            &[high, low],
        )
        .await
        .unwrap();
        assert_eq!(result.variants.len(), 2);
        assert_eq!(result.renditions.len(), 2);
        assert!(result.variants[0].bandwidth < result.variants[1].bandwidth);
        assert_eq!(result.renditions[0].segment_paths.len(), 2);
        assert_eq!(result.renditions[1].segment_paths.len(), 2);
        assert_eq!(result.renditions[0].transport_bytes, 752);
        assert_eq!(result.renditions[1].transport_bytes, 1504);
        let master = stdfs::read_to_string(&result.master_path).unwrap();
        assert!(master.contains("variant00/index.m3u8"));
        assert!(master.contains("variant01/index.m3u8"));
        for rendition in &result.renditions {
            assert!(rendition.playlist_path.exists());
            for path in &rendition.segment_paths {
                assert!(path.exists());
            }
        }
        result.remove().unwrap();
        assert!(!dir.exists());

        let bad_quality = MpegTsQuality {
            video_kbps: 200,
            audio_kbps: 96,
        };
        let rejected = temp.path().join("rejected");
        assert_eq!(
            generate_adaptive_segmented_vod(
                &controller,
                &inspector,
                &source,
                &rejected,
                2,
                &[low, bad_quality],
            )
            .await
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!rejected.exists());
    }

    #[tokio::test]
    async fn real_ffmpeg_multisegment_playlist_demuxes_when_ci_requires_it() {
        if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("five-seconds.wav");
        let samples = vec![0_u8; 80_000];
        let mut wav = Vec::with_capacity(44 + samples.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36_u32 + samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&8_000_u32.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);
        stdfs::write(&input, wav).unwrap();
        let controller = TranscodeController::new(
            PathBuf::from("ffmpeg"),
            1,
            Duration::from_secs(20),
            4 * 1024 * 1024,
        )
        .unwrap();
        let dir = temp.path().join("multisegment");
        let artifact = generate_segmented_vod(&controller, Path::new("ffprobe"), &input, &dir, 2)
            .await
            .unwrap();
        assert_eq!(artifact.segment_paths.len(), 3);
        assert_eq!(artifact.segments.len(), 3);
        assert_eq!(
            stdfs::read_to_string(&artifact.playlist_path)
                .unwrap()
                .matches("#EXT-X-DISCONTINUITY")
                .count(),
            2
        );
        let manifest = stdfs::read_to_string(&artifact.playlist_path).unwrap();
        for (i, path) in artifact.segment_paths.iter().enumerate() {
            assert!(path.exists());
            assert_eq!(stdfs::metadata(path).unwrap().len() % 188, 0);
            assert!(manifest.contains(&format!("segment{i:05}.ts")));
            let details = probe::probe(path, Path::new("ffprobe"), PROBE_TIMEOUT)
                .await
                .unwrap();
            assert!(
                details
                    .streams
                    .iter()
                    .any(|stream| stream["codec_name"].as_str() == Some("aac"))
            );
        }
        let assembled = probe::probe(&artifact.playlist_path, Path::new("ffprobe"), PROBE_TIMEOUT)
            .await
            .unwrap();
        assert!(
            assembled
                .streams
                .iter()
                .any(|s| s["codec_name"].as_str() == Some("aac"))
        );
        assert!(manifest.ends_with("#EXT-X-ENDLIST\n"));
        artifact.remove().unwrap();
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn real_ffmpeg_hls_vod_bundle_when_ci_requires_it() {
        if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("silence.wav");
        let samples = vec![0_u8; 16_000];
        let mut wav = Vec::with_capacity(44 + samples.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36_u32 + samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&8_000_u32.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);
        stdfs::write(&input, wav).unwrap();
        let controller = TranscodeController::new(
            PathBuf::from("ffmpeg"),
            1,
            Duration::from_secs(20),
            2 * 1024 * 1024,
        )
        .unwrap();
        let artifact = generate_single_segment_vod(
            &controller,
            Path::new("ffprobe"),
            &input,
            &temp.path().join("hls"),
        )
        .await
        .unwrap();
        assert!(artifact.duration_seconds > 0.0);
        assert!(artifact.transport_bytes >= 188);
        assert!(artifact.playlist_path.exists());
        assert!(artifact.segment_path.exists());
        let manifest = stdfs::read_to_string(&artifact.playlist_path).unwrap();
        assert!(manifest.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
        assert!(manifest.contains("#EXT-X-ENDLIST"));
        let media_playlist_probe =
            probe::probe(&artifact.playlist_path, Path::new("ffprobe"), PROBE_TIMEOUT)
                .await
                .unwrap();
        assert!(
            media_playlist_probe
                .streams
                .iter()
                .any(|stream| stream["codec_name"].as_str() == Some("aac"))
        );

        let ladder = generate_adaptive_vod(
            &controller,
            Path::new("ffprobe"),
            &input,
            &temp.path().join("ladder"),
            &[
                MpegTsQuality {
                    video_kbps: 300,
                    audio_kbps: 64,
                },
                MpegTsQuality {
                    video_kbps: 1200,
                    audio_kbps: 128,
                },
            ],
        )
        .await
        .unwrap();
        assert!(!ladder.variants.is_empty());
        assert_eq!(ladder.variants.len(), ladder.renditions.len());
        assert!(ladder.master_path.exists());
        let master_probe = probe::probe(&ladder.master_path, Path::new("ffprobe"), PROBE_TIMEOUT)
            .await
            .unwrap();
        assert!(!master_probe.streams.is_empty());
        for variant in &ladder.renditions {
            assert!(variant.transport_bytes >= 188);
            assert_eq!(variant.transport_bytes % 188, 0);
            assert!(variant.duration_seconds > 0.0);
        }
    }
}
