use anyhow::{anyhow, bail, Context, Result};
use diktafon_protocol::{read_frame, write_frame, ClientMsg, DaemonMsg, Msg, PROTOCOL_VERSION};
use std::cell::Cell;
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Upper bound on transcribing the queued chunks plus one polish pass; only hit
/// when the daemon is wedged, so `finish` errors instead of blocking forever.
const FINISH_TIMEOUT: Duration = Duration::from_secs(60);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

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
    pub fn spawn(socket: PathBuf) -> Self {
        let (chunk_tx, cmd_rx) = mpsc::channel::<Msg>();
        let (results_tx, results_rx) = mpsc::channel();
        let ledger = Arc::new(FlushLedger { results_tx, pending_flushes: Mutex::new(0) });
        thread::spawn(move || Transport::new(socket, ledger).run(cmd_rx));
        Self { chunk_tx, results_rx, stale_results: Cell::new(0) }
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
            let _ = self.results_tx.send(SessionResult::Failed(reason.to_string()));
        }
    }
}

struct Transport {
    socket: PathBuf,
    ledger: Arc<FlushLedger>,
    conn: Option<UnixStream>,
    backoff: Duration,
    next_attempt: Instant,
    /// Chunks of the current session lost while the daemon was unreachable;
    /// pasting silently truncated text would be worse than failing, so the
    /// session's flush turns into a Cancel plus an error.
    dropped_chunks: usize,
}

impl Transport {
    fn new(socket: PathBuf, ledger: Arc<FlushLedger>) -> Self {
        Self {
            socket,
            ledger,
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
            self.ledger.deliver(SessionResult::Failed("diktafond is unavailable".to_string()));
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
        match self.connect() {
            Ok(stream) => {
                self.backoff = INITIAL_BACKOFF;
                println!("connected to diktafond");
                self.conn = Some(stream);
                true
            }
            Err(e) => {
                eprintln!(
                    "connecting to diktafond failed: {e:#}; next attempt in {:.2?}",
                    self.backoff
                );
                self.next_attempt = Instant::now() + self.backoff;
                self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                false
            }
        }
    }

    fn connect(&self) -> Result<UnixStream> {
        let stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        write_frame(&mut &stream, &ClientMsg::Hello { version: PROTOCOL_VERSION })?;
        match read_frame::<DaemonMsg>(&mut &stream)? {
            Some(DaemonMsg::Hello { .. }) => {}
            Some(DaemonMsg::Error(e)) => bail!("daemon refused the connection: {e}"),
            Some(other) => bail!("unexpected handshake reply: {other:?}"),
            None => bail!("daemon closed the connection during the handshake"),
        }
        stream.set_read_timeout(None)?;
        spawn_reader(stream.try_clone()?, self.ledger.clone());
        Ok(stream)
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
                Ok(Some(DaemonMsg::Final(text))) => ledger.deliver(SessionResult::Final(text)),
                Ok(Some(DaemonMsg::Error(e))) => ledger.deliver(SessionResult::Failed(e)),
                // The client does not send Cancel yet (cancel gesture task), so
                // an ack needs no handling; Hello was consumed by the handshake.
                Ok(Some(DaemonMsg::Aborted | DaemonMsg::Hello { .. })) => {}
                Ok(None) => return ledger.fail_pending("diktafond closed the connection"),
                Err(e) => return ledger.fail_pending(&format!("connection to diktafond lost: {e}")),
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
                write_frame(&mut writer, &DaemonMsg::Hello { version: PROTOCOL_VERSION }).unwrap()
            }
            other => panic!("expected Hello, got {other:?}"),
        }
        let mut chunks = 0;
        while flushes > 0 {
            match read_frame::<ClientMsg>(&mut reader) {
                Ok(Some(ClientMsg::Chunk(_))) => chunks += 1,
                Ok(Some(ClientMsg::Flush)) => {
                    write_frame(&mut writer, &DaemonMsg::Final(format!("{chunks} chunks"))).unwrap();
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
        client.chunk_tx.send(Msg::Start(SessionConfig::default())).unwrap();
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

        let client = DaemonClient::spawn(socket.clone());
        assert_eq!(run_session(&client, 2).unwrap(), "2 chunks");
        assert_eq!(run_session(&client, 3).unwrap(), "3 chunks");

        // Daemon died; the next session fails instead of hanging, and reports
        // the dropped audio rather than pasting truncated text.
        std::fs::remove_file(&socket).unwrap();
        first.join().unwrap();
        let err = run_session(&client, 1).unwrap_err();
        assert!(err.to_string().contains("lost"), "{err}");

        // Daemon comes back; after the backoff the next session succeeds.
        let listener = UnixListener::bind(&socket).unwrap();
        let second = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 1));
        thread::sleep(2 * INITIAL_BACKOFF);
        assert_eq!(run_session(&client, 1).unwrap(), "1 chunks");

        drop(client);
        second.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn finish_fails_fast_when_daemon_never_existed() {
        let client = DaemonClient::spawn(test_socket("absent"));
        let start = Instant::now();
        assert!(run_session(&client, 0).is_err());
        assert!(start.elapsed() < FINISH_TIMEOUT / 2, "should not wait out the full timeout");
    }
}
