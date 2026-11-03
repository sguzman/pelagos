//! End-to-end demux of authenticated PGS HLS snapshots. The test retrieves
//! every TS segment from Pelagos's private routes, then plays an offline,
//! locally rebound copy of the *same playlist* through FFmpeg's HLS demuxer.
//! This is not a real unmodified Jellyfin client or remote HTTP playback.
use super::call;
use axum::{Router, body::to_bytes, http::StatusCode};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
    time::Duration,
};

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
    assert_av_frames(&decoded);
}

/// Decode the immutable retained preview through the real localhost HTTP
/// endpoint, including FFmpeg's own token-header propagation to every TS GET.
/// The listener is test-only and bound to 127.0.0.1 on an ephemeral port.
pub(super) async fn assert_authenticated_hls_network_demux(
    app: &Router,
    token: &str,
    playlist_path: &str,
    live: bool,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let routed_app = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, routed_app).await.unwrap();
    });
    let url = format!("http://{address}{playlist_path}");
    let mut ffmpeg = tokio::process::Command::new("ffmpeg");
    ffmpeg
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-headers"])
        .arg(format!("X-Emby-Token: {token}\r\n"));
    if live {
        // Begin at the first advertised segment rather than the newest
        // live-edge segment, so playback crosses a source-window boundary.
        ffmpeg.args(["-live_start_index", "0"]);
    }
    ffmpeg
        .arg("-i")
        .arg(url)
        .args(["-map", "0:v:0", "-map", "0:a:0"]);
    if live {
        // A rolling playlist correctly has no ENDLIST; bound this client's
        // playback instead of waiting for the entire source to complete.
        ffmpeg.args(["-t", "2.5"]);
    }
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        ffmpeg
            .args(["-f", "framemd5", "-"])
            .kill_on_drop(true)
            .output(),
    )
    .await;
    server.abort();
    let _ = server.await;
    let decoded = result
        .expect("FFmpeg network HLS decode timed out")
        .unwrap();
    assert_av_frames(&decoded);
}

fn assert_av_frames(decoded: &Output) {
    assert!(
        decoded.status.success(),
        "FFmpeg could not demux authenticated HLS: {}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    let text = String::from_utf8(decoded.stdout.clone()).unwrap();
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
