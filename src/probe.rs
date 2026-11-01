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
    input: R,
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
pub async fn probe(file: &Path, executable: &Path, deadline: Duration) -> io::Result<ProbeReport> {
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
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "FFprobe exceeded time limit",
            ));
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

/// An FFprobe-reported ASS codec can contain either ASS v4.00+ or
/// legacy SSA v4.00 script data. Parse the script header, not just the codec.
fn native_script_format(bytes: &[u8]) -> Option<&'static str> {
    let script = std::str::from_utf8(bytes).ok()?;
    let mut script_info = false;
    let mut format = None;
    let mut found_events = false;
    for line in script.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.eq_ignore_ascii_case("[Script Info]") {
            script_info = true;
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            script_info = false;
            if line.eq_ignore_ascii_case("[Events]") {
                found_events = true;
            }
            continue;
        }
        if script_info
            && let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("ScriptType")
        {
            if format.is_some() {
                return None;
            }
            format = if value.trim().eq_ignore_ascii_case("v4.00+") {
                Some("ass")
            } else if value.trim().eq_ignore_ascii_case("v4.00") {
                Some("ssa")
            } else {
                return None;
            };
        }
    }
    if found_events { format } else { None }
}

pub(crate) fn is_native_ass_script(bytes: &[u8]) -> bool {
    native_script_format(bytes) == Some("ass")
}

pub(crate) fn is_native_ssa_script(bytes: &[u8]) -> bool {
    native_script_format(bytes) == Some("ssa")
}

/// FFprobe's -show_data JSON renders stream extradata as addressed hex rows.
/// Decode only the hex column, never its untrusted printable preview.
fn decode_extradata_hex(rendered: &str) -> Option<Vec<u8>> {
    if rendered.len() > 128 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    for line in rendered.lines().filter(|line| !line.trim().is_empty()) {
        let (address, columns) = line.split_once(':')?;
        if address.len() != 8
            || !address.bytes().all(|digit| digit.is_ascii_hexdigit())
            || usize::from_str_radix(address, 16).ok()? != bytes.len()
        {
            return None;
        }
        let hex = columns.trim_start().split("  ").next()?;
        for group in hex.split_ascii_whitespace() {
            if group.len() % 2 != 0
                || group.len() > 8
                || !group.bytes().all(|digit| digit.is_ascii_hexdigit())
            {
                return None;
            }
            for pair in group.as_bytes().chunks_exact(2) {
                bytes.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
                if bytes.len() > 64 * 1024 {
                    return None;
                }
            }
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

/// Probe only subtitle extradata to distinguish genuine modern ASS from
/// legacy SSA sharing codec_name=ass. Failure means no confirmed native ASS:
/// the caller can still offer bounded text conversion. No source is modified.
pub(crate) async fn native_script_formats(
    file: &Path,
    executable: &Path,
) -> io::Result<std::collections::HashMap<i64, &'static str>> {
    let mut child = Command::new(executable)
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("s")
        .arg("-show_entries")
        .arg("stream=index,codec_name,extradata")
        .arg("-show_data")
        .arg("-of")
        .arg("json")
        .arg("-i")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let output_reader = tokio::spawn(capped_read(stdout, 256 * 1024));
    let status = match time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(result) => result?,
        Err(_) => {
            child.kill().await?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "ASS probe timed out",
            ));
        }
    };
    let bytes = output_reader.await.map_err(io::Error::other)??;
    if !status.success() || bytes.len() > 256 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ASS probe refused",
        ));
    }
    let json: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let streams = json["streams"]
        .as_array()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ASS streams missing"))?;
    let mut native = std::collections::HashMap::new();
    for stream in streams {
        if stream["codec_name"] != "ass" {
            continue;
        }
        let index = stream["index"]
            .as_i64()
            .filter(|index| (0..=i64::from(i32::MAX)).contains(index));
        let script = stream["extradata"].as_str().and_then(decode_extradata_hex);
        if let (Some(index), Some(script)) = (index, script)
            && let Some(format) = native_script_format(&script)
        {
            native.insert(index, format);
        }
    }
    Ok(native)
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

    #[test]
    fn ffprobe_ass_header_distinguishes_modern_from_legacy_and_malformed() {
        let native = b"[Script Info]\nScriptType: v4.00+\n[Events]\nFormat: Start,End,Text";
        let legacy = b"[Script Info]\nScriptType: v4.00\n[Events]\nFormat: Start,End,Text";
        assert!(is_native_ass_script(native));
        assert!(!is_native_ssa_script(native));
        assert!(is_native_ssa_script(legacy));
        assert!(!is_native_ass_script(legacy));
        assert!(!is_native_ass_script(
            b"[Script Info]\nScriptType: v4.00+\nScriptType: v4.00+\n[Events]\n"
        ));
        let rendered = "\n00000000: 5b53 6372 6970 7420 496e 666f 5d0a 5363  [Script Info].Sc\n00000010: 7269 7074 5479 7065 3a20 7634 2e30 302b  riptType: v4.00+\n00000020: 0a5b 4576 656e 7473 5d0a                 .[Events].\n";
        let decoded = decode_extradata_hex(rendered).unwrap();
        assert!(is_native_ass_script(&decoded));
        assert!(decode_extradata_hex("00000001: 5b53  bad offset").is_none());
        assert!(decode_extradata_hex("00000000: 5b5Z  bad hex").is_none());
    }

    #[tokio::test]
    async fn native_ass_probe_fails_closed_when_extradata_is_absent_or_probe_fails() {
        let (_dir, probe, media) =
            fake_probe(r#"printf '%s' '{"streams":[{"index":1,"codec_name":"ass"}]}'"#);
        assert!(
            native_script_formats(&media, &probe)
                .await
                .unwrap()
                .is_empty()
        );
        let (_dir, probe, media) = fake_probe("echo malformed-json");
        assert!(native_script_formats(&media, &probe).await.is_err());
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
        assert!(
            probe(&media, &bad_binary, Duration::from_secs(1))
                .await
                .is_err()
        );
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
