//! Bounded, CPU-first FFmpeg MPEG-TS transcode job runner.
//! This is not yet an HLS/DASH scheduler, Jellyfin PlaybackInfo transcode
//! response, GPU accelerator, or complete Smart TV playback implementation.
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Semaphore,
    time,
};

pub const MAX_TRANSCODE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 16 * 1024;

/// Fixed numeric rate targets for later HLS renditions. These are FFmpeg
/// encoder targets; actual playlist bandwidth must be measured separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MpegTsQuality {
    pub video_kbps: u32,
    pub audio_kbps: u32,
}

impl MpegTsQuality {
    fn valid(self) -> bool {
        (300..=20_000).contains(&self.video_kbps)
            && (32..=320).contains(&self.audio_kbps)
    }
}


/// A synchronous drop guard also removes partial output if the caller
/// cancels its Rust future before a timeout or FFmpeg exit is observed.
struct PartialOutput {
    path: PathBuf,
    committed: bool,
}

impl Drop for PartialOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Semaphore admission bounds concurrent FFmpeg work independently of
/// transport requests. Queued tasks spawn no child until granted a permit.
#[derive(Clone)]
pub struct TranscodeController {
    executable: PathBuf,
    permits: Arc<Semaphore>,
    capacity: usize,
    deadline: Duration,
    max_bytes: u64,
}

impl TranscodeController {
    pub fn new(
        executable: PathBuf,
        capacity: usize,
        deadline: Duration,
        max_bytes: u64,
    ) -> io::Result<Self> {
        if !(1..=16).contains(&capacity)
            || deadline.is_zero()
            || !(1..=MAX_TRANSCODE_BYTES).contains(&max_bytes)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid FFmpeg capacity, deadline or byte ceiling",
            ));
        }
        Ok(Self {
            executable,
            permits: Arc::new(Semaphore::new(capacity)),
            capacity,
            deadline,
            max_bytes,
        })
    }

    pub fn active_jobs(&self) -> usize {
        self.capacity - self.permits.available_permits()
    }

    pub async fn run(&self, source: &Path, output: &Path) -> io::Result<u64> {
        let _permit = self.permits.acquire().await.map_err(io::Error::other)?;
        self.transcode(source, output).await
    }

    /// Admission-controlled alternative for request handlers. Excess jobs
    /// receive WouldBlock immediately; no queued future, output file or child
    /// process is created. Permits are released even if the task is aborted.
    pub async fn try_run(&self, source: &Path, output: &Path) -> io::Result<u64> {
        let _permit = self.permits.try_acquire().map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "FFmpeg worker capacity exhausted",
            )
        })?;
        self.transcode(source, output).await
    }

    /// As with try_run, full-capacity workers reject extra callers before
    /// allocating files or launching FFmpeg. Quality fields are numeric only.
    pub async fn try_run_with_quality(
        &self,
        source: &Path,
        output: &Path,
        quality: MpegTsQuality,
    ) -> io::Result<u64> {
        if !quality.valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid MPEG-TS encoder target bitrates",
            ));
        }
        let _permit = self.permits.try_acquire().map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "FFmpeg worker capacity exhausted",
            )
        })?;
        transcode_mpegts_with_quality(
            &self.executable,
            source,
            output,
            self.deadline,
            self.max_bytes,
            quality,
        )
        .await
    }

    async fn transcode(&self, source: &Path, output: &Path) -> io::Result<u64> {
        transcode_mpegts(
            &self.executable,
            source,
            output,
            self.deadline,
            self.max_bytes,
        )
        .await
    }
}

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
    transcode_mpegts_inner(executable, source, output, deadline, max_bytes, None).await
}

/// Separate, validated encoder profile used for media variants. No shell
/// interpretation and no caller-supplied FFmpeg argument strings.
pub async fn transcode_mpegts_with_quality(
    executable: &Path,
    source: &Path,
    output: &Path,
    deadline: Duration,
    max_bytes: u64,
    quality: MpegTsQuality,
) -> io::Result<u64> {
    if !quality.valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid MPEG-TS encoder target bitrates",
        ));
    }
    transcode_mpegts_inner(executable, source, output, deadline, max_bytes, Some(quality)).await
}

async fn transcode_mpegts_inner(
    executable: &Path,
    source: &Path,
    output: &Path,
    deadline: Duration,
    max_bytes: u64,
    quality: Option<MpegTsQuality>,
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
    let mut partial = PartialOutput {
        path: output.to_path_buf(),
        committed: false,
    };
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
            .arg("aac");
        if let Some(quality) = quality {
            command
                .arg("-b:v")
                .arg(format!("{}k", quality.video_kbps))
                .arg("-b:a")
                .arg(format!("{}k", quality.audio_kbps));
        }
        command
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
        Ok(Ok(written)) => {
            partial.committed = true;
            Ok(written)
        }
        Ok(Err(error)) => Err(error),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "FFmpeg exceeded transcode deadline",
        )),
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
    async fn actual_ffmpeg_emits_probeable_aac_mpegts_when_ci_requires_it() {
        if std::env::var_os("PELAGOS_REQUIRE_FFMPEG").is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("silent.wav");
        // One second of valid 8-kHz, signed 16-bit mono PCM.
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
        stdfs::write(&source, wav).unwrap();
        let output = temp.path().join("audio.ts");
        let n = transcode_mpegts(
            Path::new("ffmpeg"),
            &source,
            &output,
            Duration::from_secs(20),
            2 * 1024 * 1024,
        )
        .await
        .unwrap();
        let transport = stdfs::read(&output).unwrap();
        assert_eq!(transport.len() as u64, n);
        assert!(n >= 188);
        assert_eq!(transport[0], 0x47);
        assert_eq!(transport.len() % 188, 0);
        let inspected = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "a",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(&output)
            .output()
            .await
            .unwrap();
        assert!(inspected.status.success());
        assert!(String::from_utf8_lossy(&inspected.stdout).contains("aac"));
    }

    #[tokio::test]
    async fn semaphore_limits_concurrent_jobs_without_spawning_queued_outputs() {
        let (_temp, executable, source, first_output) = fixture("sleep 1; printf 'done'");
        let second_output = first_output.with_file_name("second.ts");
        let controller =
            TranscodeController::new(executable, 1, Duration::from_secs(5), 64).unwrap();
        let running = controller.clone();
        let first_source = source.clone();
        let first_path = first_output.clone();
        let first = tokio::spawn(async move { running.run(&first_source, &first_path).await });
        time::timeout(Duration::from_secs(3), async {
            while controller.active_jobs() != 1 {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let queued = controller.clone();
        let second = second_output.clone();
        let second_source = source.clone();
        let second_job = tokio::spawn(async move { queued.run(&second_source, &second).await });
        time::sleep(Duration::from_millis(50)).await;
        assert_eq!(controller.active_jobs(), 1);
        assert!(!second_output.exists());
        assert_eq!(first.await.unwrap().unwrap(), 4);
        assert_eq!(second_job.await.unwrap().unwrap(), 4);
        assert_eq!(controller.active_jobs(), 0);
        assert_eq!(stdfs::read(&first_output).unwrap(), b"done");
        assert_eq!(stdfs::read(&second_output).unwrap(), b"done");
        assert!(
            TranscodeController::new(PathBuf::from("ffmpeg"), 0, Duration::from_secs(5), 64)
                .is_err()
        );
    }

    #[tokio::test]
    async fn try_run_rejects_over_capacity_without_file_or_child_and_recovers() {
        let (_temp, executable, source, output) = fixture("sleep 1; printf 'done'");
        let controller =
            TranscodeController::new(executable, 1, Duration::from_secs(4), 64).unwrap();
        let first_source = source.clone();
        let first_output = output.clone();
        let first_controller = controller.clone();
        let running =
            tokio::spawn(async move { first_controller.run(&first_source, &first_output).await });
        time::timeout(Duration::from_secs(3), async {
            while controller.active_jobs() == 0 {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let busy_output = output.with_file_name("busy.ts");
        let busy = controller.try_run(&source, &busy_output).await.unwrap_err();
        assert_eq!(busy.kind(), io::ErrorKind::WouldBlock);
        assert!(!busy_output.exists());
        assert_eq!(running.await.unwrap().unwrap(), 4);
        assert_eq!(controller.active_jobs(), 0);
        assert_eq!(controller.try_run(&source, &busy_output).await.unwrap(), 4);
        assert_eq!(stdfs::read(&busy_output).unwrap(), b"done");
        assert_eq!(controller.active_jobs(), 0);
    }

    #[tokio::test]
    async fn encoder_targets_accept_only_bounded_numeric_bitrates() {
        let (_temp, executable, source, output) = fixture("printf 'MPEGTS'");
        for invalid in [
            MpegTsQuality { video_kbps: 299, audio_kbps: 64 },
            MpegTsQuality { video_kbps: 20_001, audio_kbps: 64 },
            MpegTsQuality { video_kbps: 500, audio_kbps: 31 },
            MpegTsQuality { video_kbps: 500, audio_kbps: 321 },
        ] {
            assert_eq!(
                transcode_mpegts_with_quality(
                    &executable,
                    &source,
                    &output,
                    Duration::from_secs(2),
                    128,
                    invalid,
                )
                .await
                .unwrap_err()
                .kind(),
                io::ErrorKind::InvalidInput
            );
            assert!(!output.exists());
        }
        let controller =
            TranscodeController::new(executable, 1, Duration::from_secs(2), 128).unwrap();
        let quality = MpegTsQuality { video_kbps: 1500, audio_kbps: 96 };
        assert_eq!(
            controller.try_run_with_quality(&source, &output, quality)
                .await
                .unwrap(),
            6
        );
        assert_eq!(stdfs::read(&output).unwrap(), b"MPEGTS");
        assert_eq!(controller.active_jobs(), 0);
    }

    #[tokio::test]
    async fn cancelled_transcode_removes_partial_output_and_kills_child() {
        let (_temp, executable, source, output) = fixture("printf 'partial'; exec sleep 2");
        let output_in_job = output.clone();
        let job = tokio::spawn(async move {
            transcode_mpegts(
                &executable,
                &source,
                &output_in_job,
                Duration::from_secs(5),
                64,
            )
            .await
        });
        time::timeout(Duration::from_secs(2), async {
            while !output.exists() {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        job.abort();
        assert!(job.await.is_err());
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
