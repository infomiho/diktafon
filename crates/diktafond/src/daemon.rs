use anyhow::{bail, Context, Result};
use diktafon_protocol::{
    read_frame, write_frame, ClientMsg, DaemonMsg, Msg, PROTOCOL_VERSION,
};
use std::io::{BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::Inference;

/// How often the event relay checks whether the connection ended.
const RELAY_POLL: Duration = Duration::from_millis(100);

/// How long a worker reset may take: at worst it waits out chunks already
/// queued for transcription before the `Aborted` ack arrives.
const RESET_TIMEOUT: Duration = Duration::from_secs(60);

/// A client that connects but never completes the handshake would otherwise
/// block the daemon forever, since connections are served one at a time.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often startup connections check for fresh download progress, incoming
/// frames, and readiness; also the effective progress-frame rate cap.
const STARTUP_POLL: Duration = Duration::from_millis(200);

/// Progress is re-sent at least this often even when unchanged, as a
/// heartbeat: a stalled download produces no new values for over a minute, and
/// without frames the client's per-frame ready timeout would give up on a
/// daemon that is recovering fine.
const STARTUP_HEARTBEAT: Duration = Duration::from_secs(2);

/// A startup client that stops reading would otherwise block a progress write
/// forever once the socket buffer fills, and with it the whole startup
/// handover.
const STARTUP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bind the socket, provision and load the models while streaming
/// `DownloadProgress` to early clients, then serve clients one at a time until
/// killed. SIGTERM and SIGINT remove the socket file and exit.
pub fn run(models_dir: &Path, socket: &Path) -> Result<()> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    ensure_sole_daemon(socket)?;
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("binding {}", socket.display()))?;
    remove_socket_on_termination(socket);
    println!("diktafond listening on {}", socket.display());

    let (inference, early_clients) = start_up(&listener, models_dir)?;
    listener.set_nonblocking(false)?;

    // Clients that connected during startup already got their Ready; serve
    // them before accepting new connections.
    for client in early_clients {
        println!("client connected");
        let counts = ServeCounts::default();
        let result = serve_established(client.reader, client.writer, &inference, &counts);
        finish_connection(&inference, &counts, result)?;
    }
    loop {
        let (stream, _) = listener.accept()?;
        println!("client connected");
        let (counts, result) = serve(&stream, &inference);
        finish_connection(&inference, &counts, result)?;
    }
}

fn finish_connection(inference: &Inference, counts: &ServeCounts, result: Result<()>) -> Result<()> {
    if let Err(e) = result {
        eprintln!("connection ended: {e}");
    }
    // A failed reset means the worker is dead or wedged; exiting beats
    // accepting clients that can never be served.
    reset_worker(inference, counts.unacked_cancels()).context("resetting inference worker")?;
    println!("client disconnected");
    Ok(())
}

/// A connection accepted during startup, already handshaked and told `Ready`.
struct EarlyClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

#[derive(Clone, PartialEq)]
struct Download {
    model: String,
    downloaded_bytes: u64,
    total_bytes: u64,
}

#[derive(Default)]
struct StartupStatus {
    download: Mutex<Option<Download>>,
    ready: AtomicBool,
}

/// Provision and load the models on a worker thread while accepting
/// connections and streaming them download progress; returns the loaded
/// `Inference` plus every connection that survived to `Ready`.
fn start_up(listener: &UnixListener, models_dir: &Path) -> Result<(Inference, Vec<EarlyClient>)> {
    let status = Arc::new(StartupStatus::default());
    let (loaded_tx, loaded_rx) = mpsc::channel();
    let (handover_tx, handover_rx) = mpsc::channel::<EarlyClient>();
    thread::spawn({
        let status = status.clone();
        let models_dir = models_dir.to_path_buf();
        move || {
            let mut last_log = Instant::now();
            let result = crate::manifest::ensure_models(&models_dir, &mut |file, done, total| {
                if last_log.elapsed() >= Duration::from_secs(1) || done == total {
                    println!("  {file}: {}/{} MB", done / 1_000_000, total / 1_000_000);
                    last_log = Instant::now();
                }
                *status.download.lock().unwrap() = Some(Download {
                    model: file.to_string(),
                    downloaded_bytes: done,
                    total_bytes: total,
                });
            })
            .and_then(|()| {
                println!("Loading models...");
                let load_start = Instant::now();
                let inference = Inference::spawn(&models_dir);
                if inference.is_ok() {
                    println!("Models loaded in {:.2?}", load_start.elapsed());
                }
                inference
            });
            let _ = loaded_tx.send(result);
        }
    });

    listener.set_nonblocking(true)?;
    let inference = loop {
        match loaded_rx.try_recv() {
            Ok(result) => break result?,
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => bail!("startup thread died"),
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let status = status.clone();
                let handover = handover_tx.clone();
                thread::spawn(move || serve_startup_client(stream, &status, handover));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(STARTUP_POLL),
            Err(e) => return Err(e).context("accepting during startup"),
        }
    };
    status.ready.store(true, Ordering::Relaxed);
    drop(handover_tx);
    // Blocks until every startup thread finished (each notices `ready` within
    // one poll) and handed its connection over or dropped it.
    Ok((inference, handover_rx.iter().collect()))
}

/// Startup-phase connection: handshake, stream download progress, fail any
/// session flush with a clear error, and on readiness send `Ready` and hand
/// the connection over for normal serving.
fn serve_startup_client(
    stream: UnixStream,
    status: &StartupStatus,
    handover: mpsc::Sender<EarlyClient>,
) {
    let result = (|| -> Result<()> {
        // Accepted from a non-blocking listener, so undo the inherited mode.
        stream.set_nonblocking(false)?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream.try_clone()?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        stream.set_write_timeout(Some(STARTUP_WRITE_TIMEOUT))?;
        handshake(&mut reader, &mut writer)?;
        stream.set_read_timeout(Some(STARTUP_POLL))?;

        let mut last_sent = None;
        let mut last_sent_at = Instant::now();
        loop {
            if status.ready.load(Ordering::Relaxed) {
                write_frame(&mut writer, &DaemonMsg::Ready)?;
                stream.set_read_timeout(None)?;
                stream.set_write_timeout(None)?;
                let _ = handover.send(EarlyClient { reader, writer });
                return Ok(());
            }
            let download = status.download.lock().unwrap().clone();
            if let Some(d) = &download
                && (download != last_sent || last_sent_at.elapsed() >= STARTUP_HEARTBEAT)
            {
                write_frame(
                    &mut writer,
                    &DaemonMsg::DownloadProgress {
                        model: d.model.clone(),
                        downloaded_bytes: d.downloaded_bytes,
                        total_bytes: d.total_bytes,
                    },
                )?;
                last_sent = download;
                last_sent_at = Instant::now();
            }
            match read_frame::<ClientMsg>(&mut reader) {
                Ok(Some(ClientMsg::Flush)) => write_frame(
                    &mut writer,
                    &DaemonMsg::Error("diktafond is still fetching or loading models".into()),
                )?,
                Ok(Some(_)) => {}
                Ok(None) => return Ok(()),
                // The timeout may fire mid-frame for a client that stalls while
                // sending, which desyncs the stream; diktafon's own client
                // sends nothing before Ready, so only a non-conforming client
                // can hit that (and it then fails the length sanity check).
                Err(e) if is_read_timeout(&e) => {}
                Err(e) => return Err(e),
            }
        }
    })();
    if let Err(e) = result {
        eprintln!("startup client dropped: {e:#}");
    }
}

fn is_read_timeout(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|io| {
        matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
    })
}

/// Exit instead of binding over a live daemon (two clients can race their
/// auto-spawns; the loser would otherwise leak a resident daemon on an
/// unlinked socket). A connect probe distinguishes alive from a stale file
/// left by a crash, which is removed so bind can succeed.
fn ensure_sole_daemon(socket: &Path) -> Result<()> {
    if !socket.exists() {
        return Ok(());
    }
    match UnixStream::connect(socket) {
        Ok(_) => bail!("another diktafond is already serving {}", socket.display()),
        Err(_) => {
            std::fs::remove_file(socket).context("removing stale socket file")?;
            Ok(())
        }
    }
}

fn remove_socket_on_termination(socket: &Path) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let socket = socket.to_path_buf();
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGINT])
        .expect("registering signal handler");
    thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = std::fs::remove_file(&socket);
            // _exit, not exit: atexit runs ggml's Metal destructor, which
            // asserts (and aborts) while the model is still resident.
            signal_hook::low_level::exit(0);
        }
    });
}

/// Serve one connection: handshake and `Ready`, then forward client frames to
/// the worker while a relay thread streams worker events back. The returned
/// `ServeCounts` says how many forwarded `Cancel`s were still unacked when the
/// connection ended, so the reset can drain their late `Aborted`s instead of
/// leaking them to the next client.
fn serve(stream: &UnixStream, inference: &Inference) -> (ServeCounts, Result<()>) {
    let counts = ServeCounts::default();
    let result = (|| {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream.try_clone()?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        handshake(&mut reader, &mut writer)?;
        stream.set_read_timeout(None)?;
        write_frame(&mut writer, &DaemonMsg::Ready)?;
        serve_established(reader, writer, inference, &counts)
    })();
    (counts, result)
}

fn serve_established(
    mut reader: BufReader<UnixStream>,
    mut writer: UnixStream,
    inference: &Inference,
    counts: &ServeCounts,
) -> Result<()> {
    let connection_ended = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| relay_events(inference, &mut writer, &connection_ended, counts));
        let result = forward_client_frames(&mut reader, inference, counts);
        connection_ended.store(true, Ordering::Relaxed);
        result
    })
}

/// Each `Cancel` forwarded to the worker produces exactly one `Aborted` event;
/// any not consumed by the relay before the connection ended are still in the
/// channel.
#[derive(Default)]
struct ServeCounts {
    cancels_forwarded: AtomicUsize,
    aborteds_consumed: AtomicUsize,
}

impl ServeCounts {
    fn unacked_cancels(&self) -> usize {
        let cancels = self.cancels_forwarded.load(Ordering::Relaxed);
        let aborteds = self.aborteds_consumed.load(Ordering::Relaxed);
        cancels.saturating_sub(aborteds)
    }
}

fn handshake(reader: &mut impl Read, writer: &mut impl Write) -> Result<()> {
    match read_frame::<ClientMsg>(reader).context("reading handshake")? {
        Some(ClientMsg::Hello { version }) if version == PROTOCOL_VERSION => {
            write_frame(writer, &DaemonMsg::Hello { version: PROTOCOL_VERSION })
        }
        Some(ClientMsg::Hello { version }) => {
            let error =
                format!("protocol version mismatch: client {version}, daemon {PROTOCOL_VERSION}");
            let _ = write_frame(writer, &DaemonMsg::Error(error.clone()));
            bail!(error);
        }
        Some(_) => {
            let error = "expected Hello as the first message".to_string();
            let _ = write_frame(writer, &DaemonMsg::Error(error.clone()));
            bail!(error);
        }
        None => bail!("client closed before handshake"),
    }
}

fn forward_client_frames(
    reader: &mut impl Read,
    inference: &Inference,
    counts: &ServeCounts,
) -> Result<()> {
    loop {
        let msg = match read_frame::<ClientMsg>(reader)? {
            Some(ClientMsg::Start(config)) => Msg::Start(config),
            Some(ClientMsg::Chunk(samples)) => Msg::Chunk(samples),
            Some(ClientMsg::Flush) => Msg::Flush,
            Some(ClientMsg::Cancel) => {
                counts.cancels_forwarded.fetch_add(1, Ordering::Relaxed);
                Msg::Cancel
            }
            Some(ClientMsg::Hello { .. }) => bail!("unexpected Hello after handshake"),
            None => return Ok(()),
        };
        inference.chunk_tx.send(msg).context("inference worker is gone")?;
    }
}

fn relay_events(
    inference: &Inference,
    writer: &mut impl Write,
    connection_ended: &AtomicBool,
    counts: &ServeCounts,
) {
    loop {
        match inference.recv_event(RELAY_POLL) {
            Ok(event) => {
                if let DaemonMsg::Aborted = event {
                    counts.aborteds_consumed.fetch_add(1, Ordering::Relaxed);
                }
                if let Err(e) = write_frame(writer, &event) {
                    eprintln!("dropping client, write failed: {e}");
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if connection_ended.load(Ordering::Relaxed) {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Discard any half-finished session so the next connection starts clean:
/// `Cancel` resets the worker, and everything queued before its `Aborted` ack
/// belonged to the previous client. Late `Aborted`s owed to that client's own
/// unacked `Cancel`s arrive first (FIFO, one ack per `Cancel`), so this drains
/// those before treating an `Aborted` as the reset's barrier.
fn reset_worker(inference: &Inference, unacked_cancels: usize) -> Result<()> {
    inference.chunk_tx.send(Msg::Cancel).context("inference worker is gone")?;
    let deadline = Instant::now() + RESET_TIMEOUT;
    let mut aborteds_to_drain = unacked_cancels + 1;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("worker did not ack the reset in time")?;
        if let DaemonMsg::Aborted = inference.recv_event(remaining)? {
            aborteds_to_drain -= 1;
            if aborteds_to_drain == 0 {
                return Ok(());
            }
        }
    }
}
