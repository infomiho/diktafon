use crate::dictation::PhaseEvent;
use anyhow::{Context, Result, anyhow, bail};
use diktafon_protocol::{ClientMsg, DaemonMsg, Msg, PROTOCOL_VERSION, read_frame, write_frame};
use std::cell::Cell;
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Upper bound on transcribing the queued chunks plus one polish pass; only hit
/// when the daemon is wedged, so `finish` errors instead of blocking forever.
const FINISH_TIMEOUT: Duration = Duration::from_secs(60);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest silence tolerated between startup frames while waiting for `Ready`;
/// covers model loading and sha256 verification of multi-GB files.
const READY_FRAME_TIMEOUT: Duration = Duration::from_secs(120);
const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

/// How long a freshly spawned daemon may take to bind its socket (it binds
/// before provisioning, so this is process startup, not model loading). Once
/// connected, `await_ready`'s per-frame timeout takes over; a session that
/// triggered a cold multi-minute download can still exceed FINISH_TIMEOUT, in
/// which case it fails cleanly and the late result is discarded as stale.
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(30);
const DAEMON_READY_POLL: Duration = Duration::from_millis(250);
/// Minimum gap between spawn attempts, so a daemon that dies on startup does
/// not get relaunched in a tight loop.
const SPAWN_COOLDOWN: Duration = Duration::from_secs(5);

enum SessionResult {
    Final(String),
    Failed(String),
}

/// Client side of the streaming protocol, exposing the same chunks-in/text-out
/// channel interface the in-process worker had. A transport thread owns the
/// connection, reconnecting with backoff whenever a message finds it down;
/// chunks sent while the daemon is unreachable are dropped and the session's
/// `finish` surfaces the error.
pub struct DaemonClient {
    pub chunk_tx: mpsc::Sender<Msg>,
    results_rx: mpsc::Receiver<SessionResult>,
    /// Results still owed by sessions whose `finish` timed out. Each `Flush`
    /// produces exactly one result in FIFO order, so this many must be
    /// discarded before the current session's; otherwise a late reply from a
    /// wedged daemon would be pasted into the next session.
    stale_results: Cell<usize>,
}

impl DaemonClient {
    /// `daemon_bin`: the diktafond binary to auto-spawn when the socket is
    /// dead; `None` disables auto-spawn (the daemon must be started manually).
    /// `phase_tx`: receives pipeline phase signals arriving over the
    /// connection, for the UI's state entity.
    pub fn spawn(
        socket: PathBuf,
        daemon_bin: Option<PathBuf>,
        phase_tx: Option<futures::channel::mpsc::UnboundedSender<PhaseEvent>>,
    ) -> Self {
        let (chunk_tx, cmd_rx) = mpsc::channel::<Msg>();
        let (results_tx, results_rx) = mpsc::channel();
        let ledger = Arc::new(FlushLedger {
            results_tx,
            pending_flushes: Mutex::new(0),
            phase_tx,
        });
        thread::spawn(move || Transport::new(socket, daemon_bin, ledger).run(cmd_rx));
        Self {
            chunk_tx,
            results_rx,
            stale_results: Cell::new(0),
        }
    }

    /// Wait for the session flushed by `Session::stop` to finish transcribing
    /// and polishing on the daemon.
    pub fn finish(&self) -> Result<String> {
        loop {
            let result = match self.results_rx.recv_timeout(FINISH_TIMEOUT) {
                Ok(result) => result,
                Err(e) => {
                    self.stale_results.set(self.stale_results.get() + 1);
                    return Err(e).context("diktafond did not respond");
                }
            };
            if self.stale_results.get() > 0 {
                self.stale_results.set(self.stale_results.get() - 1);
                eprintln!("discarding late result from a timed-out session");
                continue;
            }
            return match result {
                SessionResult::Final(text) => Ok(text),
                SessionResult::Failed(reason) => Err(anyhow!(reason)),
            };
        }
    }
}

/// Shared between the transport thread and the per-connection reader thread.
/// `pending_flushes` counts `Flush`es written but not yet answered (normally 0
/// or 1; a session that timed out in `finish` can leave a stale one alongside
/// the next); emitting results only under its lock guarantees exactly one
/// result per `Flush` even when both threads notice a dead connection.
struct FlushLedger {
    results_tx: mpsc::Sender<SessionResult>,
    pending_flushes: Mutex<usize>,
    /// UI phase signals; piggybacks on the ledger since the reader thread
    /// already holds it.
    phase_tx: Option<futures::channel::mpsc::UnboundedSender<PhaseEvent>>,
}

impl FlushLedger {
    fn begin_flush(&self) {
        *self.pending_flushes.lock().unwrap() += 1;
    }

    /// Deliver a result for the oldest pending flush; a result arriving with
    /// none pending is stale and dropped.
    fn deliver(&self, result: SessionResult) {
        let mut pending = self.pending_flushes.lock().unwrap();
        if *pending > 0 {
            *pending -= 1;
            let _ = self.results_tx.send(result);
        }
    }

    /// In principle a reader from an already-replaced connection could fail a
    /// newer connection's pending flush here; that needs the shutdown-woken
    /// reader to stay descheduled through a reconnect plus a whole session, and
    /// at worst turns one good result into an error, never wrong text.
    fn fail_pending(&self, reason: &str) {
        let mut pending = self.pending_flushes.lock().unwrap();
        while *pending > 0 {
            *pending -= 1;
            let _ = self
                .results_tx
                .send(SessionResult::Failed(reason.to_string()));
        }
    }
}

/// Why a connection attempt failed: only `NoDaemon` (nothing listening) may
/// trigger an auto-spawn; a daemon that answered the handshake, however badly,
/// must not be spawned over.
enum ConnectFailure {
    NoDaemon(std::io::Error),
    Rejected(anyhow::Error),
}

impl std::fmt::Display for ConnectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectFailure::NoDaemon(e) => write!(f, "nothing listening: {e}"),
            ConnectFailure::Rejected(e) => write!(f, "{e:#}"),
        }
    }
}

/// Ollama-style supervision: spawn diktafond when the socket is dead, respawn
/// after it crashes. The daemon is never stopped by the client, so models stay
/// warm across client restarts.
struct Supervisor {
    bin: Option<PathBuf>,
    child: Option<std::process::Child>,
    last_spawn: Option<Instant>,
}

impl Supervisor {
    /// Spawn the daemon if allowed: auto-spawn enabled, no live child of ours,
    /// and not within the crash-loop cooldown.
    fn try_spawn(&mut self, socket: &Path) -> bool {
        let Some(bin) = &self.bin else { return false };
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(None) => return false,
                Ok(Some(status)) => eprintln!("diktafond exited: {status}"),
                Err(e) => eprintln!("checking diktafond status failed: {e}"),
            }
            self.child = None;
        }
        if let Some(last) = self.last_spawn
            && last.elapsed() < SPAWN_COOLDOWN
        {
            return false;
        }
        // Its own process group so Ctrl+C or terminal close on the client
        // doesn't take the daemon (and the warm models) down with it; stdio
        // goes to a log file next to the socket for the same reason.
        use std::os::unix::process::CommandExt;
        let log_path = socket.with_extension("log");
        let log = std::fs::File::options()
            .create(true)
            .append(true)
            .open(&log_path);
        let (stdout, stderr) = match log {
            Ok(f) => match f.try_clone() {
                Ok(clone) => (Stdio::from(clone), Stdio::from(f)),
                Err(_) => (Stdio::null(), Stdio::from(f)),
            },
            Err(_) => (Stdio::null(), Stdio::null()),
        };
        println!("starting diktafond (logs: {})...", log_path.display());
        match std::process::Command::new(bin)
            .env("DIKTAFOND_SOCKET", socket)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
        {
            Ok(child) => {
                self.child = Some(child);
                self.last_spawn = Some(Instant::now());
                true
            }
            Err(e) => {
                eprintln!("starting diktafond failed ({}): {e}", bin.display());
                self.last_spawn = Some(Instant::now());
                false
            }
        }
    }

    /// Exit status of our spawned daemon, if it has died. A `try_wait` error is
    /// folded into "still running"; the next `try_spawn` will report it.
    fn child_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.as_mut()?.try_wait().ok().flatten()
    }
}

struct Transport {
    socket: PathBuf,
    ledger: Arc<FlushLedger>,
    supervisor: Supervisor,
    conn: Option<UnixStream>,
    backoff: Duration,
    next_attempt: Instant,
    /// Chunks of the current session lost while the daemon was unreachable;
    /// pasting silently truncated text would be worse than failing, so the
    /// session's flush turns into a Cancel plus an error.
    dropped_chunks: usize,
}

impl Transport {
    fn new(socket: PathBuf, daemon_bin: Option<PathBuf>, ledger: Arc<FlushLedger>) -> Self {
        Self {
            socket,
            ledger,
            supervisor: Supervisor {
                bin: daemon_bin,
                child: None,
                last_spawn: None,
            },
            conn: None,
            backoff: INITIAL_BACKOFF,
            next_attempt: Instant::now(),
            dropped_chunks: 0,
        }
    }

    fn run(mut self, cmd_rx: mpsc::Receiver<Msg>) {
        if !self.ensure_connected() {
            eprintln!(
                "diktafond is not reachable at {}; dictation will retry on use",
                self.socket.display()
            );
        }
        for msg in cmd_rx {
            match msg {
                Msg::Start(config) => {
                    self.dropped_chunks = 0;
                    self.send(&ClientMsg::Start(config));
                }
                Msg::Chunk(samples) => {
                    if !self.send(&ClientMsg::Chunk(samples)) {
                        self.dropped_chunks += 1;
                    }
                }
                Msg::Cancel => {
                    self.dropped_chunks = 0;
                    self.send(&ClientMsg::Cancel);
                }
                Msg::Flush => self.flush(),
            }
        }
        // The client is gone; shutting down unblocks the reader thread.
        self.drop_conn();
    }

    /// End the session. If any of its audio was dropped, the daemon only holds
    /// a fragment; discard that instead of pasting silently truncated text, and
    /// surface the loss as the session's error.
    fn flush(&mut self) {
        self.ledger.begin_flush();
        if self.dropped_chunks > 0 {
            let dropped = std::mem::take(&mut self.dropped_chunks);
            self.send(&ClientMsg::Cancel);
            self.ledger.deliver(SessionResult::Failed(format!(
                "{dropped} audio chunk(s) were lost while diktafond was unreachable"
            )));
        } else if !self.send(&ClientMsg::Flush) {
            self.ledger.deliver(SessionResult::Failed(
                "diktafond is unavailable".to_string(),
            ));
        }
    }

    /// Send a message if the daemon is reachable, reconnecting first if needed.
    /// Returns false when the message could not be delivered.
    fn send(&mut self, msg: &ClientMsg) -> bool {
        if !self.ensure_connected() {
            return false;
        }
        let conn = self.conn.as_mut().expect("connected");
        if let Err(e) = write_frame(conn, msg) {
            eprintln!("send to diktafond failed: {e:#}");
            self.drop_conn();
            return false;
        }
        true
    }

    fn ensure_connected(&mut self) -> bool {
        if self.conn.is_some() {
            return true;
        }
        if Instant::now() < self.next_attempt {
            return false;
        }
        let failure = match self.connect() {
            Ok(stream) => return self.adopt(stream),
            Err(f) => f,
        };
        if matches!(failure, ConnectFailure::NoDaemon(_)) && self.supervisor.try_spawn(&self.socket)
        {
            match self.wait_for_spawned_daemon() {
                Some(stream) => return self.adopt(stream),
                // The wait already printed why it gave up.
                None => return self.schedule_retry(),
            }
        }
        eprintln!(
            "connecting to diktafond failed: {failure}; next attempt in {:.2?}",
            self.backoff
        );
        self.schedule_retry()
    }

    fn schedule_retry(&mut self) -> bool {
        self.next_attempt = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
        false
    }

    fn adopt(&mut self, stream: UnixStream) -> bool {
        self.backoff = INITIAL_BACKOFF;
        println!("connected to diktafond");
        self.conn = Some(stream);
        true
    }

    /// Poll until the daemon we just spawned answers the handshake, it dies, or
    /// the model-load deadline passes. Blocking the transport thread here is
    /// deliberate: queued session messages flow on as soon as the daemon is up.
    fn wait_for_spawned_daemon(&mut self) -> Option<UnixStream> {
        let deadline = Instant::now() + DAEMON_READY_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.supervisor.child_exit_status() {
                eprintln!("diktafond died during startup: {status}");
                return None;
            }
            match self.connect() {
                Ok(stream) => return Some(stream),
                Err(ConnectFailure::NoDaemon(_)) => thread::sleep(DAEMON_READY_POLL),
                Err(ConnectFailure::Rejected(e)) => {
                    eprintln!("spawned diktafond rejected the handshake: {e:#}");
                    return None;
                }
            }
        }
        eprintln!("diktafond did not become ready within {DAEMON_READY_TIMEOUT:?}");
        None
    }

    fn connect(&self) -> Result<UnixStream, ConnectFailure> {
        let stream = UnixStream::connect(&self.socket).map_err(ConnectFailure::NoDaemon)?;
        self.handshake(&stream).map_err(ConnectFailure::Rejected)?;
        self.await_ready(&stream)
            .map_err(ConnectFailure::Rejected)?;
        spawn_reader(
            stream
                .try_clone()
                .map_err(|e| ConnectFailure::Rejected(e.into()))?,
            self.ledger.clone(),
        );
        Ok(stream)
    }

    fn handshake(&self, stream: &UnixStream) -> Result<()> {
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        write_frame(
            &mut &*stream,
            &ClientMsg::Hello {
                version: PROTOCOL_VERSION,
            },
        )?;
        match read_frame::<DaemonMsg>(&mut &*stream)? {
            Some(DaemonMsg::Hello { .. }) => Ok(()),
            Some(DaemonMsg::Error(e)) => bail!("daemon refused the connection: {e}"),
            Some(other) => bail!("unexpected handshake reply: {other:?}"),
            None => bail!("daemon closed the connection during the handshake"),
        }
    }

    /// After the handshake the daemon sends `Ready` — immediately when warm,
    /// or after streaming download progress on a first run. Waiting here means
    /// a dictation session that triggered a cold start blocks until the daemon
    /// can actually serve it. The generous per-frame timeout resets with every
    /// progress frame, so a multi-gigabyte download never trips it while it is
    /// moving.
    fn await_ready(&self, stream: &UnixStream) -> Result<()> {
        stream.set_read_timeout(Some(READY_FRAME_TIMEOUT))?;
        let mut last_print = Instant::now() - READY_FRAME_TIMEOUT;
        loop {
            match read_frame::<DaemonMsg>(&mut &*stream)? {
                Some(DaemonMsg::Ready) => {
                    stream.set_read_timeout(None)?;
                    return Ok(());
                }
                Some(DaemonMsg::DownloadProgress {
                    model,
                    downloaded_bytes,
                    total_bytes,
                }) => {
                    if last_print.elapsed() >= Duration::from_secs(1) {
                        println!(
                            "  daemon is downloading {model}: {}/{} MB",
                            downloaded_bytes / 1_000_000,
                            total_bytes / 1_000_000
                        );
                        last_print = Instant::now();
                    }
                }
                Some(DaemonMsg::Error(e)) => bail!("daemon startup failed: {e}"),
                Some(other) => bail!("unexpected startup message: {other:?}"),
                None => bail!("daemon closed the connection during startup"),
            }
        }
    }

    fn drop_conn(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Wakes the reader thread out of its blocking read.
            let _ = conn.shutdown(Shutdown::Both);
        }
    }
}

fn spawn_reader(stream: UnixStream, ledger: Arc<FlushLedger>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        loop {
            match read_frame::<DaemonMsg>(&mut reader) {
                Ok(Some(DaemonMsg::Partial(text))) => println!("  partial: {text}"),
                Ok(Some(DaemonMsg::Polishing)) => {
                    if let Some(tx) = &ledger.phase_tx {
                        let _ = tx.unbounded_send(PhaseEvent::PolishingStarted);
                    }
                }
                Ok(Some(DaemonMsg::Final(text))) => ledger.deliver(SessionResult::Final(text)),
                Ok(Some(DaemonMsg::Error(e))) => ledger.deliver(SessionResult::Failed(e)),
                // Aborted needs no handling: Cancel never begins a flush, so no
                // result is owed; Hello, Ready, and DownloadProgress belong to
                // the pre-adoption startup phase.
                Ok(Some(
                    DaemonMsg::Aborted
                    | DaemonMsg::Hello { .. }
                    | DaemonMsg::Ready
                    | DaemonMsg::DownloadProgress { .. },
                )) => {}
                Ok(None) => return ledger.fail_pending("diktafond closed the connection"),
                Err(e) => {
                    return ledger.fail_pending(&format!("connection to diktafond lost: {e}"));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use diktafon_protocol::SessionConfig;
    use std::os::unix::net::UnixListener;

    /// Handshake, count chunks per session, answer Flush with "<n> chunks".
    /// Returns (closing the connection) after serving `flushes` sessions, which
    /// simulates the daemon dying.
    fn serve_conn(stream: UnixStream, mut flushes: usize) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        match read_frame::<ClientMsg>(&mut reader) {
            Ok(Some(ClientMsg::Hello { .. })) => {
                write_frame(
                    &mut writer,
                    &DaemonMsg::Hello {
                        version: PROTOCOL_VERSION,
                    },
                )
                .unwrap();
                write_frame(&mut writer, &DaemonMsg::Ready).unwrap();
            }
            other => panic!("expected Hello, got {other:?}"),
        }
        let mut chunks = 0;
        while flushes > 0 {
            match read_frame::<ClientMsg>(&mut reader) {
                Ok(Some(ClientMsg::Chunk(_))) => chunks += 1,
                Ok(Some(ClientMsg::Flush)) => {
                    write_frame(&mut writer, &DaemonMsg::Final(format!("{chunks} chunks")))
                        .unwrap();
                    chunks = 0;
                    flushes -= 1;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return,
            }
        }
    }

    fn test_socket(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("dkt-{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn run_session(client: &DaemonClient, chunks: usize) -> Result<String> {
        client
            .chunk_tx
            .send(Msg::Start(SessionConfig::default()))
            .unwrap();
        for _ in 0..chunks {
            client.chunk_tx.send(Msg::Chunk(vec![0.0; 160])).unwrap();
        }
        client.chunk_tx.send(Msg::Flush).unwrap();
        client.finish()
    }

    #[test]
    fn sessions_roundtrip_and_survive_daemon_restart() {
        let socket = test_socket("roundtrip");
        let listener = UnixListener::bind(&socket).unwrap();
        // First fake daemon: serves two sessions on one connection, then dies.
        let first = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 2));

        let client = DaemonClient::spawn(socket.clone(), None, None);
        assert_eq!(run_session(&client, 2).unwrap(), "2 chunks");
        assert_eq!(run_session(&client, 3).unwrap(), "3 chunks");

        // Daemon died; the next session fails instead of hanging. Whether the
        // error reports the dropped audio or the closed connection depends on
        // when the old connection's reader observed EOF; both are honest.
        std::fs::remove_file(&socket).unwrap();
        first.join().unwrap();
        let err = run_session(&client, 1).unwrap_err().to_string();
        assert!(err.contains("lost") || err.contains("closed"), "{err}");

        // Daemon comes back; once the backoff elapses a session succeeds. The
        // exact backoff state depends on how many attempts the failed session
        // made, so retry instead of sleeping a guessed amount.
        let listener = UnixListener::bind(&socket).unwrap();
        let second = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 1));
        let mut result = run_session(&client, 1);
        for _ in 0..30 {
            if result.is_ok() {
                break;
            }
            thread::sleep(INITIAL_BACKOFF);
            result = run_session(&client, 1);
        }
        assert_eq!(result.unwrap(), "1 chunks");

        drop(client);
        second.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn finish_fails_fast_when_daemon_never_existed() {
        let client = DaemonClient::spawn(test_socket("absent"), None, None);
        let start = Instant::now();
        assert!(run_session(&client, 0).is_err());
        assert!(
            start.elapsed() < FINISH_TIMEOUT / 2,
            "should not wait out the full timeout"
        );
    }

    /// A daemon binary that exits immediately must fail the session quickly
    /// instead of waiting out the whole ready deadline or respawning in a loop.
    #[test]
    fn failed_spawn_fails_session_fast() {
        let client = DaemonClient::spawn(
            test_socket("badspawn"),
            Some(PathBuf::from("/usr/bin/false")),
            None,
        );
        let start = Instant::now();
        assert!(run_session(&client, 0).is_err());
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "took {:?}",
            start.elapsed()
        );
    }

    /// Spawns the real daemon (real models); needs `cargo build -p diktafond`
    /// first. Run with `cargo test -p diktafon -- --ignored`.
    #[test]
    #[ignore = "spawns the real daemon"]
    fn auto_spawns_the_real_daemon() {
        let socket = test_socket("autospawn");
        let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/diktafond");
        assert!(
            bin.exists(),
            "build diktafond first: cargo build -p diktafond"
        );
        let client = DaemonClient::spawn(socket.clone(), Some(bin), None);
        assert_eq!(run_session(&client, 0).unwrap(), "");

        // Kill the daemon we spawned: every process on the socket that is not
        // this test process.
        let lsof = std::process::Command::new("lsof")
            .args(["-t", socket.to_str().unwrap()])
            .output()
            .unwrap();
        let own_pid = std::process::id().to_string();
        for pid in String::from_utf8_lossy(&lsof.stdout).split_whitespace() {
            if pid != own_pid {
                std::process::Command::new("kill")
                    .args(["-TERM", pid])
                    .status()
                    .unwrap();
            }
        }
    }
}
