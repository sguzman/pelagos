//! Isolated config CLI acceptance: no installed Jellyfin server, media files,
//! persistent account database, or external LAN sockets are touched.
#[cfg(target_os = "linux")]
#[test]
fn config_file_and_cli_overrides_launch_only_an_isolated_loopback_server() {
    use std::{
        fs,
        net::{SocketAddr, TcpListener, TcpStream},
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    let fixture = tempfile::tempdir().unwrap();
    let configuration = fixture.path().join("config.json");
    let original = r#"{"schema_version":1,"bind":"0.0.0.0","port":8098,"discover":true}"#;
    fs::write(&configuration, original).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let state_dir = fixture.path().join("state");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pelagos"))
        .arg("--config")
        .arg(&configuration)
        .args([
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--no-discover",
        ])
        .arg("--state-dir")
        .arg(&state_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let started = (0..100).any(|_| {
        if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
            true
        } else {
            thread::sleep(Duration::from_millis(50));
            false
        }
    });
    if !started {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("configured loopback Pelagos server did not start");
    }
    let signal = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    let stopped = (0..100).any(|_| {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            true
        } else {
            thread::sleep(Duration::from_millis(50));
            false
        }
    });
    if !stopped {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    assert!(stopped, "configured Pelagos did not exit gracefully");
    assert_eq!(fs::read_to_string(&configuration).unwrap(), original);
    assert!(state_dir.join("server-id").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn invalid_explicit_config_fails_before_creating_private_state() {
    use std::{fs, process::Command};

    let fixture = tempfile::tempdir().unwrap();
    let configuration = fixture.path().join("invalid.json");
    let state_dir = fixture.path().join("private-state");
    let raw = r#"{"schema_version":999,"bind":"127.0.0.1"}"#;
    fs::write(&configuration, raw).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_pelagos"))
        .arg("--config")
        .arg(&configuration)
        .arg("--state-dir")
        .arg(&state_dir)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!state_dir.exists());
    assert_eq!(fs::read_to_string(&configuration).unwrap(), raw);
}
