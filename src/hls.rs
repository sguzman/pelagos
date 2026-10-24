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
    io::AsyncWriteExt,
};

use crate::{probe, transcode::TranscodeController};

const SEGMENT_NAME: &str = "segment00000.ts";
const PLAYLIST_NAME: &str = "index.m3u8";
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

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

#[derive(Debug, Clone)]
pub struct HlsVodArtifact {
    pub playlist_path: PathBuf,
    pub segment_path: PathBuf,
    pub duration_seconds: f64,
    pub transport_bytes: u64,
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
    fs::create_dir(output_dir).await?;
    let mut partial = PartialBundle {
        dir: output_dir.to_path_buf(),
        committed: false,
    };
    let segment_path = output_dir.join(SEGMENT_NAME);
    let transport_bytes = controller.try_run(source, &segment_path).await?;
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
    if transport_bytes < 188 || transport_bytes % 188 != 0 || inspection.streams.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transport must contain MPEG-TS packets and stream metadata",
        ));
    }
    let playlist = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:{duration_seconds:.3},\n{SEGMENT_NAME}\n#EXT-X-ENDLIST\n",
        duration_seconds.ceil() as u32,
    );
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
    }
}
