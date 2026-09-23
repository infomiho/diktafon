use crate::dictation::PhaseEvent;
use anyhow::{Context, Result, anyhow, bail};
use diktafon_protocol::{
    ClientMsg, DaemonMsg, MODEL_MISMATCH_PREFIX, ModelSelection, Msg, PROTOCOL_VERSION,
    ReprocessOutcome, ReprocessRequest, VERSION_MISMATCH_PREFIX, read_frame, write_frame,
};
use std::cell::Cell;
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
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
const MODEL_SELECTION_POLL: Duration = Duration::from_millis(250);
/// How often the reprocess drive checks for dictation traffic that arrived
/// mid-rerun. Dictations are refused up front, so this only ever sees a press
/// that lost the race by milliseconds.
const REPROCESS_DRAIN_POLL: Duration = Duration::from_millis(50);
/// Chunk size for streaming a retained clip: 5s windows like the bench's
/// file transcription, small enough to flow, large enough for the ASR.
const REPROCESS_CHUNK: usize = 16_000 * 5;

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

#[derive(Clone, Copy)]
enum TransportSession {
    Idle,
    Active {
        start_failed: bool,
    },
    /// A settings-pane rerun owns the connection; dictation traffic is
    /// refused or failed fast until it completes.
    Reprocessing,
}

/// Client side of the streaming protocol, exposing the same chunks-in/text-out
/// channel interface the in-process worker had. A transport thread owns the
/// connection, reconnecting with backoff whenever a message finds it down;
/// chunks sent while the daemon is unreachable are dropped and the session's
/// `finish` surfaces the error.
pub struct DaemonClient {
    pub chunk_tx: mpsc::Sender<Msg>,
    pub models: ModelSelectionControl,
    /// When the transport last auto-spawned diktafond; a spawn inside a
    /// session's window marks that session as a cold start in the timings.
    pub spawned_at: Arc<Mutex<Option<Instant>>>,
    reprocessing: Arc<AtomicBool>,
    results_rx: mpsc::Receiver<SessionResult>,
    /// Results still owed by sessions whose `finish` timed out. Each `Flush`
    /// produces exactly one result in FIFO order, so this many must be
    /// discarded before the current session's; otherwise a late reply from a
    /// wedged daemon would be pasted into the next session.
    stale_results: Cell<usize>,
}

#[derive(Clone)]
pub struct ModelSelectionControl(Arc<Mutex<ModelSelection>>);

impl ModelSelectionControl {
    pub fn set(&self, models: ModelSelection) {
        let mut current = self.0.lock().unwrap();
        if *current != models {
            *current = models;
        }
    }

    fn get(&self) -> ModelSelection {
        self.0.lock().unwrap().clone()
    }
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
        models: ModelSelection,
    ) -> Self {
        let (chunk_tx, cmd_rx) = mpsc::channel::<Msg>();
        let (results_tx, results_rx) = mpsc::channel();
        let ledger = Arc::new(FlushLedger::new(results_tx, phase_tx));
        let spawned_at = Arc::new(Mutex::new(None));
        let transport_spawned_at = spawned_at.clone();
        let reprocessing = Arc::new(AtomicBool::new(false));
        let transport_reprocessing = reprocessing.clone();
        let models = ModelSelectionControl(Arc::new(Mutex::new(models)));
        let transport_models = models.clone();
        thread::spawn(move || {
            Transport::new(
                socket,
                daemon_bin,
                ledger,
                transport_spawned_at,
                transport_models,
                transport_reprocessing,
            )
            .run(cmd_rx)
        });
        Self {
            chunk_tx,
            models,
            spawned_at,
            reprocessing,
            results_rx,
            stale_results: Cell::new(0),
        }
    }

    /// Whether a settings-pane rerun currently owns the daemon connection.
    /// Dictations check it before arming so no speech is ever recorded into
    /// a session the daemon never started.
    pub fn is_reprocessing(&self) -> bool {
        self.reprocessing.load(Ordering::Acquire)
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
    /// Rerun routing: while a retranscription's Flush is owed, its
    /// Final/Failed go to the rerun waiter instead of the dictation results,
    /// phase signals are held so the pill never mirrors a settings-pane
    /// rerun, and the polish timestamp lets the transport split rerun
    /// timings.
    reprocess: Mutex<ReprocessState>,
    /// Set by the reader thread when the connection ends. The next send
    /// reconnects first instead of writing into a dead socket, whose first
    /// write always fails after an idle-timeout exit.
    conn_dead: AtomicBool,
}

#[derive(Default)]
struct ReprocessState {
    tx: Option<mpsc::Sender<SessionResult>>,
    polish_at: Option<Instant>,
    suppress_phases: bool,
}

impl FlushLedger {
    fn new(
        results_tx: mpsc::Sender<SessionResult>,
        phase_tx: Option<futures::channel::mpsc::UnboundedSender<PhaseEvent>>,
    ) -> Self {
        Self {
            results_tx,
            pending_flushes: Mutex::new(0),
            phase_tx,
            reprocess: Mutex::new(ReprocessState::default()),
            conn_dead: AtomicBool::new(false),
        }
    }

    fn begin_flush(&self) {
        *self.pending_flushes.lock().unwrap() += 1;
    }

    fn has_pending_flushes(&self) -> bool {
        *self.pending_flushes.lock().unwrap() > 0
    }

    /// Deliver daemon traffic: a rerun in flight takes precedence (its
    /// result belongs to the waiter, never to a dictation), otherwise the
    /// oldest pending flush is answered and anything else is stale-dropped.
    fn deliver(&self, result: SessionResult) {
        let reprocess = self.reprocess.lock().unwrap();
        if let Some(tx) = reprocess.tx.as_ref() {
            let _ = tx.send(result);
            return;
        }
        drop(reprocess);
        let mut pending = self.pending_flushes.lock().unwrap();
        if *pending > 0 {
            *pending -= 1;
            let _ = self.results_tx.send(result);
        }
    }

    /// Fail the current dictation session straight into its results. Unlike
    /// `deliver` this never routes to a rerun waiter: a dictation failure is
    /// always owed to `finish`, even mid-rerun. No counter is touched — the
    /// failure is manufactured here, not answered from the wire, so there is
    /// nothing to match a flush against.
    fn fail_dictation(&self, reason: String) {
        let _ = self.results_tx.send(SessionResult::Failed(reason));
    }

    /// Arm rerun routing before its Flush is written.
    fn begin_reprocess_flush(&self, tx: mpsc::Sender<SessionResult>) {
        let mut reprocess = self.reprocess.lock().unwrap();
        reprocess.tx = Some(tx);
        reprocess.polish_at = None;
        reprocess.suppress_phases = true;
    }

    /// Clear rerun routing after its answer arrived or the wait gave up. Late
    /// daemon replies then fall back to the normal stale-drop path.
    fn clear_reprocess(&self) {
        let mut reprocess = self.reprocess.lock().unwrap();
        reprocess.tx = None;
        reprocess.suppress_phases = false;
    }

    fn reprocess_suppressed(&self) -> bool {
        self.reprocess.lock().unwrap().suppress_phases
    }

    /// Timestamp a polish pass, but only while rerun routing is armed. A
    /// mid-dictation polish must never leak into a later rerun's timing
    /// split if arming ever forgets its reset.
    fn note_polish(&self) {
        let mut reprocess = self.reprocess.lock().unwrap();
        if reprocess.tx.is_some() {
            reprocess.polish_at = Some(Instant::now());
        }
    }

    /// Mark the connection dead. The reader calls this on EOF or read error
    /// so the next send reconnects instead of failing its first write.
    fn mark_conn_dead(&self) {
        self.conn_dead.store(true, Ordering::Release);
    }

    /// Take a pending connection death, if the reader reported one.
    fn take_conn_dead(&self) -> bool {
        self.conn_dead.swap(false, Ordering::AcqRel)
    }

    fn take_reprocess_polish(&self) -> Option<Instant> {
        self.reprocess.lock().unwrap().polish_at.take()
    }

    /// In principle a reader from an already-replaced connection could fail a
    /// newer connection's pending flush here; that needs the shutdown-woken
    /// reader to stay descheduled through a reconnect plus a whole session, and
    /// at worst turns one good result into an error, never wrong text.
    fn fail_pending(&self, reason: &str) {
        let mut reprocess = self.reprocess.lock().unwrap();
        if let Some(tx) = reprocess.tx.take() {
            let _ = tx.send(SessionResult::Failed(reason.to_string()));
        }
        drop(reprocess);
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

/// Set on the quit-everything path before the daemon is SIGTERMed, so a
/// transport hiccup during shutdown (a chunk write failing as the daemon
/// dies) cannot auto-respawn the daemon the user just asked to stop.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn disable_daemon_spawn() {
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::Relaxed);
}

impl Supervisor {
    /// Spawn the daemon if allowed: auto-spawn enabled, not shutting down, no
    /// live child of ours, and not within the crash-loop cooldown.
    fn try_spawn(&mut self, socket: &Path, models: &ModelSelection) -> bool {
        if SHUTTING_DOWN.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
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
        eprintln!("starting diktafond (logs: {})...", log_path.display());
        let mut command = std::process::Command::new(bin);
        // An explicit env override (tests) wins over the configured value.
        if std::env::var_os("DIKTAFOND_IDLE_SECS").is_none() {
            command.env(
                "DIKTAFOND_IDLE_SECS",
                crate::config::SessionSettings::load()
                    .idle_unload_secs
                    .to_string(),
            );
        }
        match command
            .env("DIKTAFOND_SOCKET", socket)
            .env("DIKTAFON_TRANSCRIPTION_MODEL", &models.transcription)
            .env("DIKTAFON_POLISHING_MODEL", &models.polishing)
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
    /// A mismatched daemon is retired at most once; if its replacement also
    /// mismatches, the spawn binary itself is stale and retiring again would
    /// churn forever.
    retired_mismatch: bool,
    /// Mirrored to [`DaemonClient::spawned_at`] whenever a daemon is spawned.
    spawned_at: Arc<Mutex<Option<Instant>>>,
    models: ModelSelectionControl,
    connected_models: Option<ModelSelection>,
    observed_models: ModelSelection,
    model_refresh_pending: bool,
    session: TransportSession,
    /// Mirrors [`DaemonClient::reprocessing`]: true while a rerun owns the
    /// connection, so a press that lost the race fails fast instead of
    /// recording into a session the daemon never started.
    reprocessing: Arc<AtomicBool>,
}

impl Transport {
    fn new(
        socket: PathBuf,
        daemon_bin: Option<PathBuf>,
        ledger: Arc<FlushLedger>,
        spawned_at: Arc<Mutex<Option<Instant>>>,
        models: ModelSelectionControl,
        reprocessing: Arc<AtomicBool>,
    ) -> Self {
        let observed_models = models.get();
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
            retired_mismatch: false,
            spawned_at,
            models,
            connected_models: None,
            observed_models,
            model_refresh_pending: false,
            session: TransportSession::Idle,
            reprocessing,
        }
    }

    fn run(mut self, cmd_rx: mpsc::Receiver<Msg>) {
        if !self.ensure_connected() {
            eprintln!(
                "diktafond is not reachable at {}; dictation will retry on use",
                self.socket.display()
            );
        }
        loop {
            let msg = match cmd_rx.recv_timeout(MODEL_SELECTION_POLL) {
                Ok(msg) => msg,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.refresh_models_if_idle();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match msg {
                Msg::Start(config) => {
                    self.prepare_session();
                    self.dropped_chunks = 0;
                    let start_failed = !self.send_start(config);
                    self.session = TransportSession::Active { start_failed };
                }
                Msg::Chunk(samples) => {
                    let start_failed = matches!(
                        self.session,
                        TransportSession::Active { start_failed: true }
                    );
                    if start_failed || !self.send(&ClientMsg::Chunk(samples)) {
                        self.dropped_chunks += 1;
                    }
                }
                Msg::Cancel => {
                    self.dropped_chunks = 0;
                    if !matches!(
                        self.session,
                        TransportSession::Active { start_failed: true }
                    ) {
                        self.send(&ClientMsg::Cancel);
                    }
                    self.session = TransportSession::Idle;
                }
                Msg::Flush => {
                    self.flush();
                    self.session = TransportSession::Idle;
                }
                Msg::Reprocess(req) => {
                    // The rerun owns the daemon while it runs; a second one
                    // (or a dictation, via the press-side guard) waits outside.
                    // A stale dictation flush still owed would be misdelivered
                    // as the rerun answer (and the real answer pasted later),
                    // so anything outstanding refuses too.
                    if self.reprocessing.load(Ordering::Acquire) {
                        let _ = req
                            .reply
                            .send(Err("a retranscription is already running".to_string()));
                    } else if !matches!(self.session, TransportSession::Idle)
                        || self.ledger.has_pending_flushes()
                    {
                        let _ = req.reply.send(Err(
                            "a dictation is in flight; try again when idle".to_string(),
                        ));
                    } else {
                        self.run_reprocess(req, &cmd_rx);
                    }
                }
            }
            self.refresh_models_if_idle();
        }
        // The client is gone; shutting down unblocks the reader thread.
        self.drop_conn();
    }

    fn prepare_session(&mut self) {
        if self.connected_models.as_ref() != Some(&self.models.get()) {
            self.drop_conn();
        }
    }

    fn send_start(&mut self, config: diktafon_protocol::SessionConfig) -> bool {
        if self.send(&ClientMsg::Start(config.clone())) {
            return true;
        }
        self.next_attempt = Instant::now();
        self.send(&ClientMsg::Start(config))
    }

    fn observe_model_selection(&mut self) {
        let desired = self.models.get();
        if desired != self.observed_models {
            self.observed_models = desired;
            self.model_refresh_pending = true;
            self.retired_mismatch = false;
        }
    }

    fn refresh_models_if_idle(&mut self) {
        self.observe_model_selection();
        if !self.model_refresh_pending
            || matches!(
                self.session,
                TransportSession::Active { .. } | TransportSession::Reprocessing
            )
            || self.ledger.has_pending_flushes()
        {
            return;
        }
        if self.connected_models.as_ref() != Some(&self.observed_models) {
            self.drop_conn();
        }
        if self.ensure_connected() && self.connected_models.as_ref() == Some(&self.observed_models)
        {
            self.model_refresh_pending = false;
        }
    }

    /// End the session. If any of its audio was dropped, the daemon only holds
    /// a fragment; discard that instead of pasting silently truncated text, and
    /// surface the loss as the session's error. A stray Flush mid-rerun is
    /// swallowed: no dictation can own it (press is refused first), and
    /// touching the ledger could leak the rerun's answer into dictation
    /// results.
    fn flush(&mut self) {
        if matches!(self.session, TransportSession::Reprocessing) {
            return;
        }
        if matches!(
            self.session,
            TransportSession::Active { start_failed: true }
        ) {
            self.dropped_chunks = 0;
            self.ledger.fail_dictation(
                "dictation could not start because diktafond was unreachable".to_string(),
            );
        } else if self.dropped_chunks > 0 {
            let dropped = std::mem::take(&mut self.dropped_chunks);
            self.send(&ClientMsg::Cancel);
            self.ledger.fail_dictation(format!(
                "{dropped} audio chunk(s) were lost while diktafond was unreachable"
            ));
        } else if !self.send(&ClientMsg::Flush) {
            self.ledger
                .fail_dictation("diktafond is unavailable".to_string());
        } else {
            self.ledger.begin_flush();
        }
    }

    /// Drain dictation traffic that arrived mid-rerun through the refusal
    /// paths, so a raced press fails fast instead of becoming a fresh
    /// session afterwards.
    fn drain_during_reprocess(&mut self, cmd_rx: &mpsc::Receiver<Msg>) {
        while let Ok(msg) = cmd_rx.try_recv() {
            self.handle_during_reprocess(msg);
        }
    }

    /// Drive one retained clip through the daemon as a normal session. Runs
    /// inline on the transport thread while the run loop waits: dictation
    /// traffic arriving meanwhile is refused or failed fast, never
    /// interleaved. The daemon never learns it is a rerun except through
    /// `no_history`, which is forced here so no caller can forget it into a
    /// history-polluting rerun.
    fn run_reprocess(&mut self, req: ReprocessRequest, cmd_rx: &mpsc::Receiver<Msg>) {
        let ReprocessRequest {
            samples,
            mut config,
            reply,
        } = req;
        config.no_history = true;
        self.session = TransportSession::Reprocessing;
        self.reprocessing.store(true, Ordering::Release);
        if !self.send(&ClientMsg::Start(config)) {
            self.end_reprocess();
            let _ = reply.send(Err("diktafond is unavailable".into()));
            return;
        }
        // Timed after the Start is accepted so a cold daemon's model load
        // does not pollute the rerun timings.
        let started = Instant::now();
        for chunk in samples.chunks(REPROCESS_CHUNK) {
            if !self.send(&ClientMsg::Chunk(chunk.to_vec())) {
                self.end_reprocess();
                let _ = reply.send(Err("diktafond is unavailable".into()));
                return;
            }
        }
        let (repro_tx, repro_rx) = mpsc::channel();
        self.ledger.begin_reprocess_flush(repro_tx);
        if !self.send(&ClientMsg::Flush) {
            self.end_reprocess();
            let _ = reply.send(Err("diktafond is unavailable".into()));
            return;
        }
        let deadline = Instant::now() + FINISH_TIMEOUT;
        loop {
            match repro_rx.recv_timeout(REPROCESS_DRAIN_POLL) {
                Ok(result) => {
                    self.drain_during_reprocess(cmd_rx);
                    match result {
                        SessionResult::Final(text) => {
                            let finished = Instant::now();
                            let polish_at = self.ledger.take_reprocess_polish();
                            let outcome = ReprocessOutcome {
                                text,
                                asr_ms: polish_at
                                    .unwrap_or(finished)
                                    .duration_since(started)
                                    .as_millis() as u64,
                                polish_ms: polish_at
                                    .map(|at| finished.duration_since(at).as_millis() as u64)
                                    .unwrap_or(0),
                            };
                            self.end_reprocess();
                            let _ = reply.send(Ok(outcome));
                            return;
                        }
                        SessionResult::Failed(reason) => {
                            self.end_reprocess();
                            let _ = reply.send(Err(reason));
                            return;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        self.end_reprocess();
                        let _ = reply.send(Err("retranscription timed out".into()));
                        return;
                    }
                    self.drain_during_reprocess(cmd_rx);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.end_reprocess();
                    let _ = reply.send(Err("retranscription was interrupted".into()));
                    return;
                }
            }
        }
    }

    /// Dictation traffic that arrives mid-rerun. A press that lost the race
    /// fails fast: no audio is ever recorded into a session the daemon never
    /// started. The raced Flush answers that failure immediately (a swallowed
    /// Flush would hang `finish` until timeout) and parks the session Idle;
    /// a second Flush finds Idle and is swallowed so no stale failure can
    /// poison the next dictation's result. A raced Cancel parks Idle too:
    /// the cancel path never reads a result, so without this the failure
    /// state would refuse every later rerun until the next dictation.
    fn handle_during_reprocess(&mut self, msg: Msg) {
        match msg {
            Msg::Start(_) => {
                self.session = TransportSession::Active { start_failed: true };
            }
            Msg::Flush
                if matches!(
                    self.session,
                    TransportSession::Active { start_failed: true }
                ) =>
            {
                self.ledger.fail_dictation(
                    "a retranscription is running; try again in a moment".to_string(),
                );
                self.session = TransportSession::Idle;
            }
            Msg::Cancel => {
                self.session = TransportSession::Idle;
            }
            Msg::Reprocess(req) => {
                let _ = req
                    .reply
                    .send(Err("a retranscription is already running".into()));
            }
            Msg::Chunk(_) | Msg::Flush => {}
        }
    }

    /// Release rerun ownership. The session returns to Idle only when the
    /// rerun still owns it: a raced press that failed fast keeps its failure
    /// state so its own Flush answers it instead of hanging.
    fn end_reprocess(&mut self) {
        self.reprocessing.store(false, Ordering::Release);
        self.ledger.clear_reprocess();
        if matches!(self.session, TransportSession::Reprocessing) {
            self.session = TransportSession::Idle;
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
            if self.ledger.take_conn_dead() {
                eprintln!("diktafond went away; reconnecting");
                self.drop_conn();
            } else {
                return true;
            }
        }
        if Instant::now() < self.next_attempt {
            return false;
        }
        let mut failure = match self.connect() {
            Ok((stream, models)) => return self.adopt(stream, models),
            Err(f) => f,
        };
        // A resident daemon from an older build refuses our handshake forever;
        // retire it and fall through to spawning our own.
        if let ConnectFailure::Rejected(e) = &failure
            && (format!("{e:#}").contains(VERSION_MISMATCH_PREFIX)
                || format!("{e:#}").contains(MODEL_MISMATCH_PREFIX))
        {
            if self.retired_mismatch {
                eprintln!(
                    "daemon still version-mismatched after a restart; the spawned binary is stale"
                );
            } else {
                self.retired_mismatch = true;
                if retire_mismatched_daemon(&self.socket) {
                    self.supervisor.last_spawn = None;
                    failure = ConnectFailure::NoDaemon(std::io::Error::other("retired old daemon"));
                }
            }
        }
        if matches!(failure, ConnectFailure::NoDaemon(_))
            && self.supervisor.try_spawn(&self.socket, &self.models.get())
        {
            *self.spawned_at.lock().unwrap() = Some(Instant::now());
            if let Some(tx) = &self.ledger.phase_tx {
                let _ = tx.unbounded_send(PhaseEvent::DaemonStarting);
            }
            match self.wait_for_spawned_daemon() {
                Some((stream, models)) => return self.adopt(stream, models),
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

    fn adopt(&mut self, stream: UnixStream, models: ModelSelection) -> bool {
        self.backoff = INITIAL_BACKOFF;
        self.retired_mismatch = false;
        eprintln!("connected to diktafond");
        self.conn = Some(stream);
        self.connected_models = Some(models);
        true
    }

    /// Poll until the daemon we just spawned answers the handshake, it dies, or
    /// the model-load deadline passes. Blocking the transport thread here is
    /// deliberate: queued session messages flow on as soon as the daemon is up.
    fn wait_for_spawned_daemon(&mut self) -> Option<(UnixStream, ModelSelection)> {
        let deadline = Instant::now() + DAEMON_READY_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.supervisor.child_exit_status() {
                eprintln!("diktafond died during startup: {status}");
                return None;
            }
            match self.connect() {
                Ok(connection) => return Some(connection),
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

    fn connect(&self) -> Result<(UnixStream, ModelSelection), ConnectFailure> {
        let requested = self.models.get();
        let stream = UnixStream::connect(&self.socket).map_err(ConnectFailure::NoDaemon)?;
        self.handshake(&stream, &requested)
            .map_err(ConnectFailure::Rejected)?;
        self.await_ready(&stream)
            .map_err(ConnectFailure::Rejected)?;
        spawn_reader(
            stream
                .try_clone()
                .map_err(|e| ConnectFailure::Rejected(e.into()))?,
            self.ledger.clone(),
        );
        Ok((stream, requested))
    }

    fn handshake(&self, stream: &UnixStream, requested: &ModelSelection) -> Result<()> {
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        write_frame(
            &mut &*stream,
            &ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                models: requested.clone(),
            },
        )?;
        match read_frame::<DaemonMsg>(&mut &*stream)? {
            Some(DaemonMsg::Hello { version, models })
                if version == PROTOCOL_VERSION && models == *requested =>
            {
                Ok(())
            }
            Some(DaemonMsg::Hello { version, models }) => bail!(
                "{MODEL_MISMATCH_PREFIX}: requested {:?}, daemon v{version} loaded {:?}",
                requested,
                models
            ),
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
        let result = self.await_ready_frames(stream);
        // Whether it came up or gave up, the daemon is no longer starting.
        if let Some(tx) = &self.ledger.phase_tx {
            let _ = tx.unbounded_send(PhaseEvent::DaemonReady);
        }
        // Also clear the UI's download state on failure; otherwise a daemon
        // dying mid-download leaves the pill claiming that download forever.
        if result.is_err()
            && let Some(tx) = &self.ledger.phase_tx
        {
            let _ = tx.unbounded_send(PhaseEvent::DownloadFinished);
        }
        result
    }

    fn await_ready_frames(&self, stream: &UnixStream) -> Result<()> {
        stream.set_read_timeout(Some(READY_FRAME_TIMEOUT))?;
        let mut last_print = Instant::now() - READY_FRAME_TIMEOUT;
        loop {
            match read_frame::<DaemonMsg>(&mut &*stream)? {
                Some(DaemonMsg::Ready) => {
                    if let Some(tx) = &self.ledger.phase_tx {
                        let _ = tx.unbounded_send(PhaseEvent::DownloadFinished);
                    }
                    stream.set_read_timeout(None)?;
                    return Ok(());
                }
                Some(DaemonMsg::DownloadProgress {
                    model,
                    downloaded_bytes,
                    total_bytes,
                }) => {
                    if let Some(tx) = &self.ledger.phase_tx {
                        let percent = (downloaded_bytes * 100 / total_bytes.max(1)).min(100) as u8;
                        let _ = tx.unbounded_send(PhaseEvent::DownloadProgress {
                            model: model.clone(),
                            percent,
                        });
                    }
                    if last_print.elapsed() >= Duration::from_secs(1) {
                        eprintln!(
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
        self.connected_models = None;
    }
}

/// SIGTERM a resident daemon that refuses our protocol version or model pair,
/// then wait briefly for its socket to vanish.
fn retire_mismatched_daemon(socket: &Path) -> bool {
    use crate::daemon_process::StopError;
    let Some(pid) = crate::daemon_process::pid_for(socket) else {
        eprintln!("mismatched daemon has no usable pid file; stop it manually");
        return false;
    };
    eprintln!("retiring mismatched diktafond (pid {pid})");
    match crate::daemon_process::stop(pid) {
        Ok(()) => {}
        Err(StopError::NotOurs) => {
            eprintln!("pid {pid} no longer names a diktafond; stop the old daemon manually");
            return false;
        }
        Err(StopError::SignalFailed) => {
            eprintln!("could not signal diktafond (pid {pid}); stop it manually");
            return false;
        }
    }
    for _ in 0..20 {
        if !socket.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!("old diktafond did not exit within 2s");
    false
}

fn spawn_reader(stream: UnixStream, ledger: Arc<FlushLedger>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        loop {
            match read_frame::<DaemonMsg>(&mut reader) {
                Ok(Some(DaemonMsg::Partial(text))) => {
                    println!("  partial: {text}");
                    if !ledger.reprocess_suppressed()
                        && let Some(tx) = &ledger.phase_tx
                    {
                        let _ = tx.unbounded_send(PhaseEvent::Partial(text));
                    }
                }
                Ok(Some(DaemonMsg::Polishing)) => {
                    ledger.note_polish();
                    if !ledger.reprocess_suppressed()
                        && let Some(tx) = &ledger.phase_tx
                    {
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
                Ok(None) => {
                    ledger.mark_conn_dead();
                    return ledger.fail_pending("diktafond closed the connection");
                }
                Err(e) => {
                    ledger.mark_conn_dead();
                    return ledger.fail_pending(&format!("connection to diktafond lost: {e}"));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use diktafon_protocol::{ReprocessRequest, ReprocessResult, SessionConfig};
    use std::os::unix::net::UnixListener;

    /// The handshake both fake daemons open with: Hello exchange, then Ready.
    fn serve_handshake(reader: &mut BufReader<UnixStream>, writer: &mut UnixStream) {
        match read_frame::<ClientMsg>(reader) {
            Ok(Some(ClientMsg::Hello { .. })) => {
                write_frame(
                    writer,
                    &DaemonMsg::Hello {
                        version: PROTOCOL_VERSION,
                        models: ModelSelection::default(),
                    },
                )
                .unwrap();
                write_frame(writer, &DaemonMsg::Ready).unwrap();
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    /// Handshake, count chunks per session, answer Flush with "<n> chunks".
    /// Returns (closing the connection) after serving `flushes` sessions, which
    /// simulates the daemon dying.
    fn serve_conn(stream: UnixStream, mut flushes: usize) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        serve_handshake(&mut reader, &mut writer);
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

    /// Handshake, then serve one rerun gated by the test: record whether its
    /// Start carried `no_history` and how many chunks arrived, tell the test
    /// the Flush landed, wait for release, then answer Polishing and Final.
    /// Afterwards serves one ordinary session the same way `serve_conn` does,
    /// reporting its own `(false, chunks, starts)` triple.
    fn serve_gated_rerun(
        stream: UnixStream,
        release: mpsc::Receiver<()>,
        report: mpsc::Sender<(bool, usize, usize)>,
    ) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        serve_handshake(&mut reader, &mut writer);
        let mut starts = 0;
        for gated in [true, false] {
            let mut no_history = false;
            let mut chunks = 0;
            loop {
                match read_frame::<ClientMsg>(&mut reader) {
                    Ok(Some(ClientMsg::Start(config))) => {
                        starts += 1;
                        no_history = config.no_history;
                    }
                    Ok(Some(ClientMsg::Chunk(_))) => chunks += 1,
                    Ok(Some(ClientMsg::Flush)) => break,
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => return,
                }
            }
            report.send((no_history, chunks, starts)).unwrap();
            if gated {
                release.recv().unwrap();
                write_frame(&mut writer, &DaemonMsg::Partial("rerun partial".into())).unwrap();
                write_frame(&mut writer, &DaemonMsg::Polishing).unwrap();
                write_frame(&mut writer, &DaemonMsg::Final("rerun text".into())).unwrap();
            } else {
                write_frame(&mut writer, &DaemonMsg::Final(format!("{chunks} chunks"))).unwrap();
                return;
            }
        }
    }

    /// Send one rerun without blocking, for tests that interleave dictation
    /// traffic mid-rerun.
    fn send_reprocess(client: &DaemonClient, samples: Vec<f32>) -> mpsc::Receiver<ReprocessResult> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let config = SessionConfig {
            no_history: true,
            ..SessionConfig::default()
        };
        client
            .chunk_tx
            .send(Msg::Reprocess(ReprocessRequest {
                samples,
                config,
                reply: reply_tx,
            }))
            .unwrap();
        reply_rx
    }

    /// Drive one rerun through `client`, waiting up to 10s for its answer.
    fn run_reprocess(client: &DaemonClient, samples: Vec<f32>) -> ReprocessResult {
        send_reprocess(client, samples)
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
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

    /// Send one rerun without blocking, for tests that interleave dictation
    /// traffic mid-rerun.
    #[test]
    fn reprocess_carries_no_history_and_leaves_dictations_clean() {
        let socket = test_socket("reprocess-clean");
        let listener = UnixListener::bind(&socket).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            serve_gated_rerun(listener.accept().unwrap().0, release_rx, report_tx)
        });

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        let reply = send_reprocess(&client, vec![0.0; 800]);
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (true, 1, 1)
        );
        release_tx.send(()).unwrap();
        let outcome = reply
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(outcome.text, "rerun text");
        assert_eq!(run_session(&client, 2).unwrap(), "2 chunks");
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (false, 2, 2)
        );
        server.join().unwrap();
    }

    #[test]
    fn reprocess_is_busy_while_a_dictation_is_live() {
        let socket = test_socket("reprocess-busy");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 2));

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        client
            .chunk_tx
            .send(Msg::Start(SessionConfig::default()))
            .unwrap();
        let busy = run_reprocess(&client, vec![0.0; 160]);
        assert!(
            busy.is_err_and(|e| e.contains("in flight")),
            "expected a busy refusal"
        );
        client.chunk_tx.send(Msg::Flush).unwrap();
        assert!(client.finish().is_ok());
        let outcome = run_reprocess(&client, vec![0.0; 160]).unwrap();
        assert_eq!(outcome.text, "1 chunks");
        server.join().unwrap();
    }

    #[test]
    fn daemon_error_reaches_the_waiter() {
        let socket = test_socket("reprocess-error");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let stream = listener.accept().unwrap().0;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            serve_handshake(&mut reader, &mut writer);
            loop {
                match read_frame::<ClientMsg>(&mut reader) {
                    Ok(Some(ClientMsg::Flush)) => {
                        write_frame(&mut writer, &DaemonMsg::Error("asr exploded".into())).unwrap();
                        return;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => return,
                }
            }
        });

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        let err = run_reprocess(&client, vec![0.0; 160]).unwrap_err();
        assert_eq!(err, "asr exploded");
        server.join().unwrap();
    }

    #[test]
    fn raced_press_fails_fast_and_rerun_survives() {
        let socket = test_socket("reprocess-race");
        let listener = UnixListener::bind(&socket).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            serve_gated_rerun(listener.accept().unwrap().0, release_rx, report_tx)
        });

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        let reply = send_reprocess(&client, vec![0.0; 800]);
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (true, 1, 1)
        );
        // A press that lost the race: its Start never reaches the daemon,
        // and its Flush fails fast instead of hanging `finish`.
        client
            .chunk_tx
            .send(Msg::Start(SessionConfig::default()))
            .unwrap();
        client.chunk_tx.send(Msg::Chunk(vec![0.0; 160])).unwrap();
        client.chunk_tx.send(Msg::Flush).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(
            reply
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .text,
            "rerun text"
        );
        let err = client.finish().unwrap_err().to_string();
        assert!(err.contains("retranscription"), "{err}");
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn second_rerun_waits_its_turn() {
        let socket = test_socket("repro-second");
        let listener = UnixListener::bind(&socket).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            serve_gated_rerun(listener.accept().unwrap().0, release_rx, report_tx)
        });

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        let first = send_reprocess(&client, vec![0.0; 800]);
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (true, 1, 1)
        );
        let busy = run_reprocess(&client, vec![0.0; 160]);
        assert!(
            busy.is_err_and(|e| e.contains("already running")),
            "expected an already-running refusal"
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            first
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .text,
            "rerun text"
        );
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn cancel_mid_rerun_parks_idle() {
        let socket = test_socket("repro-cancel");
        let listener = UnixListener::bind(&socket).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            serve_gated_rerun(listener.accept().unwrap().0, release_rx, report_tx)
        });

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        let reply = send_reprocess(&client, vec![0.0; 800]);
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (true, 1, 1)
        );
        // A raced press that cancels instead of releasing: no Flush ever
        // arrives, so without the Idle parking every later rerun would see
        // a dictation in flight that no longer exists.
        client
            .chunk_tx
            .send(Msg::Start(SessionConfig::default()))
            .unwrap();
        client.chunk_tx.send(Msg::Cancel).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(
            reply
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .text,
            "rerun text"
        );
        let outcome = run_reprocess(&client, vec![0.0; 160]).unwrap();
        assert_eq!(outcome.text, "1 chunks");
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (true, 1, 2)
        );
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn connection_loss_fails_the_waiter() {
        let socket = test_socket("repro-conn-loss");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let stream = listener.accept().unwrap().0;
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            serve_handshake(&mut reader, &mut writer);
            // Read the rerun's Start, then vanish without answering.
            while !matches!(
                read_frame::<ClientMsg>(&mut reader),
                Ok(Some(ClientMsg::Start(_))) | Ok(None) | Err(_)
            ) {}
        });

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        let err = run_reprocess(&client, vec![0.0; 800]).unwrap_err();
        assert!(
            err.contains("closed") || err.contains("lost") || err.contains("unavailable"),
            "{err}"
        );
        server.join().unwrap();
    }

    #[test]
    fn rerun_phases_never_reach_the_pill() {
        use futures::channel::mpsc::unbounded;

        let socket = test_socket("repro-phases");
        let listener = UnixListener::bind(&socket).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            serve_gated_rerun(listener.accept().unwrap().0, release_rx, report_tx)
        });

        let (phase_tx, mut phase_rx) = unbounded();
        let client = DaemonClient::spawn(
            socket.clone(),
            None,
            Some(phase_tx),
            ModelSelection::default(),
        );
        let reply = send_reprocess(&client, vec![0.0; 800]);
        use crate::dictation::PhaseEvent;
        // Connecting announces DownloadFinished then DaemonReady; drain that
        // handshake noise so the assertion below only sees rerun-time phases.
        // Partial or PolishingStarted here would already be a leak.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match phase_rx.try_recv() {
                Err(_) if Instant::now() >= deadline => panic!("no DaemonReady arrived"),
                Err(_) => thread::sleep(Duration::from_millis(10)),
                Ok(PhaseEvent::DaemonReady) => break,
                Ok(PhaseEvent::Partial(_)) => panic!("Partial leaked during rerun"),
                Ok(PhaseEvent::PolishingStarted) => {
                    panic!("PolishingStarted leaked during rerun")
                }
                Ok(_) => {}
            }
        }
        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (true, 1, 1)
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            reply
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap()
                .text,
            "rerun text"
        );
        // The daemon sent Partial and Polishing for the rerun; neither may
        // surface as pill phases.
        match phase_rx.try_recv() {
            Err(_) => {}
            Ok(crate::dictation::PhaseEvent::Partial(_)) => panic!("Partial leaked during rerun"),
            Ok(crate::dictation::PhaseEvent::PolishingStarted) => {
                panic!("PolishingStarted leaked during rerun")
            }
            Ok(_) => panic!("another phase leaked during rerun"),
        }
        drop(client);
        server.join().unwrap();
    }

    #[test]
    fn desired_model_selection_can_change_without_touching_the_connection() {
        let control = ModelSelectionControl(Arc::new(Mutex::new(ModelSelection::default())));
        let cohere = ModelSelection {
            transcription: "cohere-transcribe-q5-k-m".into(),
            ..ModelSelection::default()
        };
        control.set(cohere.clone());
        assert_eq!(control.get(), cohere);
    }

    #[test]
    fn a_new_model_choice_gets_its_own_restart_attempt() {
        let control = ModelSelectionControl(Arc::new(Mutex::new(ModelSelection::default())));
        let (results_tx, _) = mpsc::channel();
        let ledger = Arc::new(FlushLedger::new(results_tx, None));
        let mut transport = Transport::new(
            test_socket("new-model-restart"),
            None,
            ledger,
            Arc::new(Mutex::new(None)),
            control.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        transport.retired_mismatch = true;
        control.set(ModelSelection {
            transcription: "cohere-transcribe-q5-k-m".into(),
            ..ModelSelection::default()
        });

        transport.observe_model_selection();

        assert!(!transport.retired_mismatch);
        assert!(transport.model_refresh_pending);
    }

    #[test]
    fn chunks_do_not_reconnect_after_start_failed() {
        let socket = test_socket("failed-start-quarantine");
        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        client
            .chunk_tx
            .send(Msg::Start(SessionConfig::default()))
            .unwrap();
        thread::sleep(Duration::from_millis(50));

        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        thread::sleep(INITIAL_BACKOFF + Duration::from_millis(50));
        client.chunk_tx.send(Msg::Chunk(vec![0.0; 160])).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );

        client.chunk_tx.send(Msg::Flush).unwrap();
        assert!(client.finish().unwrap_err().to_string().contains("start"));
        drop(client);
        drop(listener);
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn idle_model_change_reconnects_without_waiting_for_a_session() {
        let socket = test_socket("model-prewarm");
        let listener = UnixListener::bind(&socket).unwrap();
        let (initial_tx, initial_rx) = mpsc::channel();
        let (switched_tx, switched_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for (expected, ready) in [
                (ModelSelection::default(), &initial_tx),
                (
                    ModelSelection {
                        transcription: "cohere-transcribe-q5-k-m".into(),
                        ..ModelSelection::default()
                    },
                    &switched_tx,
                ),
            ] {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let Some(ClientMsg::Hello { models, .. }) = read_frame(&mut reader).unwrap() else {
                    panic!("expected Hello");
                };
                assert_eq!(models, expected);
                write_frame(
                    &mut writer,
                    &DaemonMsg::Hello {
                        version: PROTOCOL_VERSION,
                        models,
                    },
                )
                .unwrap();
                write_frame(&mut writer, &DaemonMsg::Ready).unwrap();
                ready.send(()).unwrap();
                let _ = read_frame::<ClientMsg>(&mut reader);
            }
        });

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        initial_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        client.models.set(ModelSelection {
            transcription: "cohere-transcribe-q5-k-m".into(),
            ..ModelSelection::default()
        });
        switched_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        drop(client);
        server.join().unwrap();
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn start_reconnects_after_the_daemon_closed_an_idle_socket() {
        let socket = test_socket("idle-close");
        let listener = UnixListener::bind(&socket).unwrap();
        let first = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 0));
        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        first.join().unwrap();
        thread::sleep(Duration::from_millis(50));

        std::fs::remove_file(&socket).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let second = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 1));

        assert_eq!(run_session(&client, 1).unwrap(), "1 chunks");

        drop(client);
        second.join().unwrap();
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn connection_remembers_the_models_used_by_its_handshake() {
        let socket = test_socket("model-race");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let Some(ClientMsg::Hello { models, .. }) = read_frame(&mut reader).unwrap() else {
                panic!("expected Hello");
            };
            write_frame(
                &mut writer,
                &DaemonMsg::Hello {
                    version: PROTOCOL_VERSION,
                    models,
                },
            )
            .unwrap();
            thread::sleep(Duration::from_millis(50));
            write_frame(&mut writer, &DaemonMsg::Ready).unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        let initial = ModelSelection::default();
        let control = ModelSelectionControl(Arc::new(Mutex::new(initial.clone())));
        let change = control.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            change.set(ModelSelection {
                transcription: "cohere-transcribe-q5-k-m".into(),
                ..ModelSelection::default()
            });
        });
        let (results_tx, _) = mpsc::channel();
        let ledger = Arc::new(FlushLedger::new(results_tx, None));
        let mut transport = Transport::new(
            socket.clone(),
            None,
            ledger,
            Arc::new(Mutex::new(None)),
            control,
            Arc::new(AtomicBool::new(false)),
        );
        let (stream, connected_models) = transport
            .connect()
            .unwrap_or_else(|failure| panic!("connecting failed: {failure}"));
        assert_eq!(connected_models, initial);
        transport.drop_conn();
        drop(stream);
        server.join().unwrap();
        let _ = std::fs::remove_file(socket);
    }

    #[test]
    fn sessions_roundtrip_and_survive_daemon_restart() {
        let socket = test_socket("roundtrip");
        let listener = UnixListener::bind(&socket).unwrap();
        // First fake daemon: serves two sessions on one connection, then dies.
        let first = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 2));

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        assert_eq!(run_session(&client, 2).unwrap(), "2 chunks");
        assert_eq!(run_session(&client, 3).unwrap(), "3 chunks");

        // Daemon died; the next session fails instead of hanging. Whether the
        // error reports a failed start, dropped audio, or the closed connection
        // depends on when the old connection's reader observed EOF.
        std::fs::remove_file(&socket).unwrap();
        first.join().unwrap();
        let err = run_session(&client, 1).unwrap_err().to_string();
        assert!(
            err.contains("start") || err.contains("lost") || err.contains("closed"),
            "{err}"
        );

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
    fn first_session_after_daemon_death_reconnects() {
        let socket = test_socket("respawn-clean");
        // First fake daemon: serves one session, then vanishes like an
        // idle-timeout exit, socket file and all.
        let listener = UnixListener::bind(&socket).unwrap();
        let first = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 1));

        let client = DaemonClient::spawn(socket.clone(), None, None, ModelSelection::default());
        assert_eq!(run_session(&client, 1).unwrap(), "1 chunks");
        first.join().unwrap();
        // Let the reader observe EOF and mark the connection dead.
        thread::sleep(Duration::from_millis(200));
        std::fs::remove_file(&socket).unwrap();

        // A respawned daemon on the same path serves the very next session:
        // without the mark the first send would write into the dead socket
        // and fail, failing the session instead of the send.
        let listener = UnixListener::bind(&socket).unwrap();
        let second = thread::spawn(move || serve_conn(listener.accept().unwrap().0, 1));
        assert_eq!(run_session(&client, 1).unwrap(), "1 chunks");

        drop(client);
        second.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn finish_fails_fast_when_daemon_never_existed() {
        let client =
            DaemonClient::spawn(test_socket("absent"), None, None, ModelSelection::default());
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
            ModelSelection::default(),
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
        let client =
            DaemonClient::spawn(socket.clone(), Some(bin), None, ModelSelection::default());
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
