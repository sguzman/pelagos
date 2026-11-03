//! End-to-end demux of authenticated PGS HLS snapshots. The test retrieves
//! every TS segment from Pelagos's private routes, then plays an offline,
//! locally rebound copy of the *same playlist* through FFmpeg's HLS demuxer.
//! This is not a real unmodified Jellyfin client or remote HTTP playback.
use super::call;
use axum::{Router, body::to_bytes, http::StatusCode};
use std::{fs, path::Path, process::Command};

pub(super) async fn assert_authenticated_hls_demux(
    app: &Router,
    token: &str,
    manifest: &str,
    root: &Path,
) {
    assert!(manifest.starts_with("#EXTM3U\n"));
    assert!(!manifest.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
    assert!(!manifest.contains("#EXT-X-ENDLIST"));
    fs::create_dir(root).unwrap();
    let mut rebound = String::new();
    let mut count = 0_usize;
    for line in manifest.lines() {
        if line.starts_with("/Videos/") && line.ends_with(".ts") {
            let response = call(app, line, Some(token)).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "missing HLS segment {line}"
            );
            let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap();
            assert!(!bytes.is_empty());
            assert_eq!(bytes.len() % 188, 0, "TS packet size");
            assert!(bytes.chunks_exact(188).all(|packet| packet[0] == 0x47));
            let name = format!("window-segment-{count}.ts");
            fs::write(root.join(&name), &bytes).unwrap();
            rebound.push_str(&name);
            rebound.push('\n');
            count += 1;
        } else {
            rebound.push_str(line);
            rebound.push('\n');
        }
    }
    assert!(count >= 3, "must span multiple independent PGS excerpts");
    // An immutable snapshot must be closed for FFmpeg's offline decode
    // without modifying the live HTTP representation or its sequence tags.
    rebound.push_str("#EXT-X-ENDLIST\n");
    let playlist = root.join("authenticated-rolling.m3u8");
    fs::write(&playlist, rebound).unwrap();
    let decoded = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file,crypto,data",
            "-allowed_extensions",
            "ALL",
            "-i",
        ])
        .arg(playlist)
        .args(["-map", "0:v:0", "-map", "0:a:0", "-f", "framemd5", "-"])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "FFmpeg could not demux decoded rolling HLS: {}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    let text = String::from_utf8(decoded.stdout).unwrap();
    let frame_streams = text
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split(',').next().map(str::trim))
        .collect::<Vec<_>>();
    let video_frames = frame_streams
        .iter()
        .filter(|stream| **stream == "0")
        .count();
    let audio_frames = frame_streams
        .iter()
        .filter(|stream| **stream == "1")
        .count();
    assert!(
        video_frames >= 10,
        "expected decoded frames across at least two PGS windows; video={video_frames}, audio={audio_frames}"
    );
    assert!(
        audio_frames >= 10,
        "expected AAC frames across multiple HLS windows; video={video_frames}, audio={audio_frames}"
    );
}
