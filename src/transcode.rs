//! Bounded, CPU-first FFmpeg MPEG-TS transcode job runner.
//! This is not yet an HLS/DASH scheduler, Jellyfin PlaybackInfo transcode
//! response, GPU accelerator, or complete Smart TV playback implementation.
use std::{io, path::Path, process::Stdio, time::Duration};

use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time,
};

pub const MAX_TRANSCODE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 16 * 1024;

/// Transcode one local file into a newly created, size-bounded MPEG-TS file.
/// The executable receives a fixed argument sequence, never a shell command.
/// FFmpeg emits bytes to stdout so it cannot choose or overwrite the output
/// pathname. The Rust writer enforces byte limits before every write.
///
/// On failure, oversized output or timeout, the partial target is deleted.
/// Running FFmpeg processes are killed if this future is cancelled/dropped.
pub async fn transcode_mpegts(
    executable: &Path,
    source: &Path,
    output: &Path,
    deadline: Duration,
    max_bytes: u64,
) -> io::Result<u64> {
    if deadline.is_zero() || !(1..=MAX_TRANSCODE_BYTES).contains(&max_bytes) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid transcode deadline or output byte ceiling",
        ));
    }
    let metadata = fs::metadata(source).await?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "FFmpeg source must be a regular file",
        ));
    }
    // create_new makes an existing file or symlink an error and does not
    // silently clobber it, including when concurrently created.
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .await?;
    let result = time::timeout(deadline, async {
        let mut command = Command::new(executable);
        command
            .arg("-nostdin")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(source)
            .arg("-map")
            .arg("0:v:0?")
            .arg("-map")
            .arg("0:a:0?")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("veryfast")
            .arg("-c:a")
            .arg("aac")
            .arg("-f")
            .arg("mpegts")
            .arg("pipe:1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let mut stdout = child.stdout.take().expect("FFmpeg stdout piped");
        let mut stderr = child.stderr.take().expect("FFmpeg stderr piped");

        let copy = async {
            let mut total = 0_u64;
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                let count = stdout.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                total = total.checked_add(count as u64).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "transcode exceeds byte limit")
                })?;
                if total > max_bytes {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "transcode exceeds byte limit",
                    ));
                }
                destination.write_all(&buffer[..count]).await?;
            }
            destination.flush().await?;
            Ok::<u64, io::Error>(total)
        };
        let capture = async {
            let mut bytes = Vec::new();
            (&mut stderr)
                .take(MAX_STDERR_BYTES + 1)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() as u64 > MAX_STDERR_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "FFmpeg diagnostic exceeds limit",
                ));
            }
            Ok::<Vec<u8>, io::Error>(bytes)
        };
        let (written, diagnostics, status) = tokio::try_join!(copy, capture, child.wait())?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "FFmpeg exited with {status}: {}",
                String::from_utf8_lossy(&diagnostics).trim()
            )));
        }
        Ok::<u64, io::Error>(written)
    })
    .await;

    drop(destination);
    match result {
        Ok(Ok(written)) => Ok(written),
        Ok(Err(error)) => {
            let _ = fs::remove_file(output).await;
            Err(error)
        }
        Err(_) => {
            let _ = fs::remove_file(output).await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "FFmpeg exceeded transcode deadline",
            ))
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs as stdfs, os::unix::fs::PermissionsExt, path::PathBuf};

    fn fixture(script: &str) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("-movie.mp4");
        stdfs::write(&source, b"synthetic source").unwrap();
        let executable = temp.path().join("fake-ffmpeg");
        stdfs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        stdfs::set_permissions(&executable, stdfs::Permissions::from_mode(0o700)).unwrap();
        let output = temp.path().join("transcoded.ts");
        (temp, executable, source, output)
    }

    #[tokio::test]
    async fn bounded_success_streams_without_overwriting_source() {
        let (_temp, executable, source, output) = fixture("printf 'MPEGTS'");
        let written = transcode_mpegts(&executable, &source, &output, Duration::from_secs(3), 64)
            .await
            .unwrap();
        assert_eq!(written, 6);
        assert_eq!(stdfs::read(&output).unwrap(), b"MPEGTS");
        assert_eq!(stdfs::read(&source).unwrap(), b"synthetic source");
        assert_eq!(
            transcode_mpegts(&executable, &source, &output, Duration::from_secs(3), 64)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(stdfs::read(&output).unwrap(), b"MPEGTS");
    }

    #[tokio::test]
    async fn failed_process_and_partial_outputs_are_removed() {
        let (_temp, executable, source, output) =
            fixture("printf 'partial'; echo 'bad media' >&2; exit 7");
        let err = transcode_mpegts(&executable, &source, &output, Duration::from_secs(3), 64)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("bad media"));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn too_large_output_is_stopped_before_exceeding_limit() {
        let (_temp, executable, source, output) = fixture("head -c 1024 /dev/zero");
        let err = transcode_mpegts(&executable, &source, &output, Duration::from_secs(3), 32)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn hanging_transcode_is_killed_and_partial_file_removed() {
        let (_temp, executable, source, output) = fixture("exec sleep 2");
        let err = transcode_mpegts(&executable, &source, &output, Duration::from_millis(40), 64)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn zero_deadline_or_byte_limit_is_rejected_before_output_creation() {
        let (_temp, executable, source, output) = fixture("printf 'not reached'");
        for (deadline, cap) in [
            (Duration::ZERO, 10),
            (Duration::from_secs(1), 0),
            (Duration::from_secs(1), MAX_TRANSCODE_BYTES + 1),
        ] {
            let err = transcode_mpegts(&executable, &source, &output, deadline, cap)
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
        assert!(!output.exists());
        let directory_err = transcode_mpegts(
            &executable,
            _temp.path(),
            &output,
            Duration::from_secs(1),
            32,
        )
        .await
        .unwrap_err();
        assert_eq!(directory_err.kind(), io::ErrorKind::InvalidInput);
        assert!(!output.exists());
    }
}
