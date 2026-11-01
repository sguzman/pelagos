//! Linux lifecycle acceptance uses a child Pelagos process on a temporary
//! loopback port. It never signals the user's desktop, service or real server.
#[cfg(target_os = "linux")]
#[test]
fn pelagos_process_gracefully_exits_on_sigterm() {
    use std::{
        net::{SocketAddr, TcpListener, TcpStream},
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    let fixture = tempfile::tempdir().unwrap();
    let address = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = address.local_addr().unwrap().port();
    drop(address);

    let mut child = Command::new(env!("CARGO_BIN_EXE_pelagos"))
        .args(["--bind", "127.0.0.1", "--port", &port.to_string()])
        .arg("--state-dir")
        .arg(fixture.path())
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
        panic!("isolated Pelagos HTTP listener did not start");
    }
    let signaled = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(signaled.success(), "SIGTERM command failed");
    let mut stopped = None;
    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            stopped = Some(status);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if stopped.is_none() {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("Pelagos did not gracefully exit after SIGTERM");
    }
    assert!(stopped.unwrap().success());
}
