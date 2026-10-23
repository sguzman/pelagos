//! Read-only FFprobe integration. FFmpeg remains an external tool, not a Rust rewrite.
//! No media bytes are copied or modified by this module.
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::Serialize;
use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Command, time};

const MAX_STDOUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 16 * 1024;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
pub struct ProbeReport {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub format: Option<Value>,
    pub streams: Vec<Value>,
    pub chapters: Vec<Value>,
}

async fn capped_read<R: tokio::io::AsyncRead + Unpin>(
    mut input: R,
    max_size: usize,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .take((max_size + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

/// Probe one explicitly supplied local media file. The executable is invoked directly,
/// never through a shell; the source is passed as a single argument after -i.
/// Process lifetime and output size are bounded.
pub async fn probe(
    file: &Path,
    executable: &Path,
    deadline: Duration,
) -> io::Result<ProbeReport> {
    let metadata = tokio::fs::metadata(file).await?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "FFprobe input must be a regular file",
        ));
    }

    let mut command = Command::new(executable);
    command
        .arg("-v")
        .arg("error")
        .arg("-show_format")
        .arg("-show_streams")
        .arg("-show_chapters")
        .arg("-of")
        .arg("json")
        .arg("-i")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout pipe requested");
    let stderr = child.stderr.take().expect("stderr pipe requested");
    let output_reader = tokio::spawn(capped_read(stdout, MAX_STDOUT_BYTES));
    let error_reader = tokio::spawn(capped_read(stderr, MAX_STDERR_BYTES));

    // A blocked, oversized producer is also terminated by the deadline.
    let status = match time::timeout(deadline, child.wait()).await {
        Ok(result) => result?,
        Err(_) => {
            child.kill().await?;
            return Err(io::Error::new(io::ErrorKind::TimedOut, "FFprobe exceeded time limit"));
        }
    };
    let output = output_reader.await.map_err(io::Error::other)??;
    let stderr = error_reader.await.map_err(io::Error::other)??;
    if output.len() > MAX_STDOUT_BYTES || stderr.len() > MAX_STDERR_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "FFprobe output exceeds limit",
        ));
    }
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr);
        return Err(io::Error::other(format!(
            "FFprobe exited with {status}: {}",
            message.trim()
        )));
    }
    let data: Value = serde_json::from_slice(&output).map_err(io::Error::other)?;
    let streams = data["streams"]
        .as_array()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "FFprobe streams missing"))?
        .clone();
    let chapters = data["chapters"].as_array().cloned().unwrap_or_default();
    let format = data.get("format").filter(|f| f.is_object()).cloned();
    Ok(ProbeReport {
        path: file.to_path_buf(),
        size_bytes: metadata.len(),
        format,
        streams,
        chapters,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn fake_probe(script: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("-sample.mp4");
        fs::write(&media, b"synthetic test only").unwrap();
        let executable = dir.path().join("fake-ffprobe");
        fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        (dir, executable, media)
    }

    #[tokio::test]
    async fn probes_synthetic_json_without_executing_media_path() {
        let (_dir, exe, media) = fake_probe(
            r#"printf '%s' '{"streams":[{"codec_type":"video","codec_name":"h264"}],"format":{"format_name":"mov,mp4"},"chapters":[]}'"#,
        );
        let result = probe(&media, &exe, Duration::from_secs(3)).await.unwrap();
        assert_eq!(result.streams.len(), 1);
        assert_eq!(result.streams[0]["codec_name"], "h264");
        assert_eq!(result.format.unwrap()["format_name"], "mov,mp4");
        assert_eq!(result.size_bytes, 19);
        assert_eq!(fs::read(&media).unwrap(), b"synthetic test only");
    }

    #[tokio::test]
    async fn missing_binary_fails_without_fallback() {
        let (_dir, exe, media) = fake_probe("exit 0");
        let bad_binary = exe.with_extension("missing");
        assert!(probe(&media, &bad_binary, Duration::from_secs(1))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn failed_probe_does_not_succeed_with_empty_data() {
        let (_dir, exe, media) = fake_probe("echo 'unsupported media' >&2; exit 2");
        let error = probe(&media, &exe, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unsupported media"));
    }

    #[tokio::test]
    async fn invalid_json_is_rejected() {
        let (_dir, exe, media) = fake_probe("echo 'not-json'");
        assert_eq!(
            probe(&media, &exe, Duration::from_secs(2))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::Other
        );
    }

    #[tokio::test]
    async fn stalls_are_cancelled() {
        let (_dir, exe, media) = fake_probe("sleep 2");
        let error = probe(&media, &exe, Duration::from_millis(60))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn directory_is_not_a_valid_probe_target() {
        let dir = tempfile::tempdir().unwrap();
        let error = probe(dir.path(), Path::new("ffprobe"), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
