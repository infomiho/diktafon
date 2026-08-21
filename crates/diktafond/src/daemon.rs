use anyhow::{bail, Context, Result};
use diktafon_protocol::{
    read_frame, write_frame, ClientMsg, DaemonMsg, Msg, PROTOCOL_VERSION,
};
use std::io::{BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
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

/// Load the models, then serve clients one at a time until killed. SIGTERM and
/// SIGINT remove the socket file and exit.
pub fn run(models_dir: &Path, socket: &Path) -> Result<()> {
    let mut last_print = Instant::now();
    crate::manifest::ensure_models(models_dir, &mut |file, done, total| {
        if last_print.elapsed() >= Duration::from_secs(1) || done == total {
            println!("  {file}: {}/{} MB", done / 1_000_000, total / 1_000_000);
            last_print = Instant::now();
        }
    })
    .context("provisioning models")?;

    println!("Loading models...");
    let load_start = Instant::now();
    let inference = Inference::spawn(models_dir)?;
    println!("Models loaded in {:.2?}", load_start.elapsed());

    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    ensure_sole_daemon(socket)?;
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("binding {}", socket.display()))?;
    remove_socket_on_termination(socket);
    println!("diktafond listening on {}", socket.display());

    loop {
        let (stream, _) = listener.accept()?;
        println!("client connected");
        let (unacked_cancels, result) = serve(&stream, &inference);
        if let Err(e) = result {
            eprintln!("connection ended: {e}");
        }
        // A failed reset means the worker is dead or wedged; exiting beats
        // accepting clients that can never be served.
        reset_worker(&inference, unacked_cancels).context("resetting inference worker")?;
        println!("client disconnected");
    }
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

/// Serve one connection: handshake, then forward client frames to the worker
/// while a relay thread streams worker events back. Also returns how many
/// forwarded `Cancel`s were still unacked when the connection ended, so the
/// reset can drain their late `Aborted`s instead of leaking them to the next
/// client.
fn serve(stream: &UnixStream, inference: &Inference) -> (usize, Result<()>) {
    let counts = ServeCounts::default();
    let result = (|| {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream.try_clone()?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        handshake(&mut reader, &mut writer)?;
        stream.set_read_timeout(None)?;

        let connection_ended = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| relay_events(inference, &mut writer, &connection_ended, &counts));
            let result = forward_client_frames(&mut reader, inference, &counts);
            connection_ended.store(true, Ordering::Relaxed);
            result
        })
    })();
    (counts.unacked_cancels(), result)
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
