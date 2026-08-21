use diktafon_protocol::{
    read_frame, write_frame, ClientMsg, DaemonMsg, SessionConfig, PROTOCOL_VERSION,
};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const MODEL_LOAD_TIMEOUT: Duration = Duration::from_secs(120);

fn wait_for_socket(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + MODEL_LOAD_TIMEOUT;
    while !path.exists() {
        assert!(child.try_wait().unwrap().is_none(), "daemon exited early");
        assert!(Instant::now() < deadline, "daemon never bound its socket");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn read_daemon_msg(stream: &mut UnixStream) -> DaemonMsg {
    read_frame::<DaemonMsg>(stream).unwrap().expect("daemon closed the stream")
}

/// Spawns the real daemon, so it loads the real models from Application
/// Support; run manually with `cargo test -p diktafond -- --ignored`.
#[test]
#[ignore = "loads real models"]
fn daemon_serves_sessions_over_the_socket() {
    let socket = std::env::temp_dir().join(format!("diktafond-test-{}.sock", std::process::id()));
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_diktafond"))
        .env("DIKTAFOND_SOCKET", &socket)
        .spawn()
        .unwrap();
    wait_for_socket(&socket, &mut daemon);

    let mut stream = UnixStream::connect(&socket).unwrap();
    write_frame(&mut stream, &ClientMsg::Hello { version: PROTOCOL_VERSION }).unwrap();
    assert_eq!(read_daemon_msg(&mut stream), DaemonMsg::Hello { version: PROTOCOL_VERSION });

    // An empty session skips ASR and polish entirely and finishes instantly.
    write_frame(&mut stream, &ClientMsg::Start(SessionConfig::default())).unwrap();
    write_frame(&mut stream, &ClientMsg::Flush).unwrap();
    assert_eq!(read_daemon_msg(&mut stream), DaemonMsg::Final(String::new()));

    write_frame(&mut stream, &ClientMsg::Start(SessionConfig::default())).unwrap();
    write_frame(&mut stream, &ClientMsg::Cancel).unwrap();
    assert_eq!(read_daemon_msg(&mut stream), DaemonMsg::Aborted);

    // A second Hello on a new connection must work after the first disconnects.
    drop(stream);
    let mut stream = UnixStream::connect(&socket).unwrap();
    write_frame(&mut stream, &ClientMsg::Hello { version: PROTOCOL_VERSION }).unwrap();
    assert_eq!(read_daemon_msg(&mut stream), DaemonMsg::Hello { version: PROTOCOL_VERSION });

    // A version mismatch is answered with Error.
    let mut mismatched = UnixStream::connect(&socket).unwrap();
    drop(stream);
    write_frame(&mut mismatched, &ClientMsg::Hello { version: PROTOCOL_VERSION + 1 }).unwrap();
    assert!(matches!(read_daemon_msg(&mut mismatched), DaemonMsg::Error(_)));

    // SIGTERM removes the socket file on the way out.
    Command::new("kill")
        .args(["-TERM", &daemon.id().to_string()])
        .status()
        .unwrap();
    let exited = daemon.wait().unwrap();
    assert!(exited.success());
    assert!(!socket.exists(), "socket file was not cleaned up");
}
