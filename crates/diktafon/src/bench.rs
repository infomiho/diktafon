//! Headless benchmark mode: `diktafon --transcribe-file x.wav [--repeat N]
//! [--json] [--paced] [--chunk-secs N]`. Feeds a 16kHz mono s16 WAV through
//! the daemon (auto-spawning it like a normal session would) and reports
//! per-stage timings; the daemon's `Polishing` frame marks the ASR/polish
//! boundary. Batch mode sends the whole file at once and measures raw
//! throughput; `--paced` replays chunks on the wall clock as if spoken live,
//! measuring what a user would wait at release: backlog at flush, tail ASR,
//! and polish. Benches run against their own daemon on a temp socket with
//! history recording off, so runs never appear as the user's dictations.

use crate::transport::DaemonClient;
use anyhow::{Context, Result, bail};
use diktafon_protocol::{
    ClientMsg, DaemonMsg, Msg, PROTOCOL_VERSION, TARGET_RATE, read_frame, socket_path, write_frame,
};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const CHUNK_SECS: usize = 5;

struct Run {
    asr_secs: f32,
    polish_secs: f32,
    total_secs: f32,
    text: String,
}

pub fn transcribe_file(args: &[String]) -> Result<()> {
    let path = args
        .first()
        .context("usage: --transcribe-file <wav> [--repeat N] [--json]")?;
    let repeat = args
        .iter()
        .position(|a| a == "--repeat")
        .and_then(|i| args.get(i + 1))
        .map(|n| n.parse::<usize>())
        .transpose()
        .context("--repeat wants a number")?
        .unwrap_or(1);
    let json = args.iter().any(|a| a == "--json");
    let paced = args.iter().any(|a| a == "--paced");
    let chunk_secs = args
        .iter()
        .position(|a| a == "--chunk-secs")
        .and_then(|i| args.get(i + 1))
        .map(|n| n.parse::<f32>())
        .transpose()
        .context("--chunk-secs wants a number")?
        .unwrap_or(5.0);

    let samples = wav_samples(path)?;
    let audio_secs = samples.len() as f32 / TARGET_RATE as f32;

    isolate_daemon();
    ensure_daemon()?;
    let stream = UnixStream::connect(socket_path()).context("connecting to diktafond")?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    write_frame(
        &mut writer,
        &ClientMsg::Hello {
            version: PROTOCOL_VERSION,
        },
    )?;
    loop {
        match read_frame::<DaemonMsg>(&mut reader)?.context("daemon closed")? {
            DaemonMsg::Ready => break,
            DaemonMsg::Hello { .. } | DaemonMsg::DownloadProgress { .. } => {}
            other => bail!("unexpected startup reply: {other:?}"),
        }
    }

    if paced {
        return run_paced(
            reader, writer, &samples, chunk_secs, repeat, json, audio_secs,
        );
    }

    let mut runs = Vec::new();
    for i in 0..repeat {
        let start = Instant::now();
        write_frame(
            &mut writer,
            &ClientMsg::Start(crate::config::SessionSettings::default().session()),
        )?;
        for chunk in samples.chunks(TARGET_RATE as usize * CHUNK_SECS) {
            write_frame(&mut writer, &ClientMsg::Chunk(chunk.to_vec()))?;
        }
        write_frame(&mut writer, &ClientMsg::Flush)?;

        let mut polishing_at = None;
        let (text, total) = loop {
            match read_frame::<DaemonMsg>(&mut reader)?.context("daemon closed")? {
                DaemonMsg::Partial(_) => {}
                DaemonMsg::Polishing => polishing_at = Some(start.elapsed()),
                DaemonMsg::Final(text) => break (text, start.elapsed()),
                other => bail!("unexpected reply: {other:?}"),
            }
        };
        let asr = polishing_at.unwrap_or(total);
        let run = Run {
            asr_secs: asr.as_secs_f32(),
            polish_secs: (total - asr.min(total)).as_secs_f32(),
            total_secs: total.as_secs_f32(),
            text,
        };
        if !json {
            println!(
                "run {}: asr {:.2}s, polish {:.2}s, total {:.2}s ({:.1}x RT)",
                i + 1,
                run.asr_secs,
                run.polish_secs,
                run.total_secs,
                audio_secs / run.total_secs
            );
        }
        runs.push(run);
    }

    let best = runs
        .iter()
        .map(|r| r.total_secs)
        .fold(f32::INFINITY, f32::min);
    if json {
        let runs_json: Vec<String> = runs
            .iter()
            .map(|r| {
                format!(
                    "{{\"asr_secs\":{:.3},\"polish_secs\":{:.3},\"total_secs\":{:.3}}}",
                    r.asr_secs, r.polish_secs, r.total_secs
                )
            })
            .collect();
        println!(
            "{{\"audio_secs\":{audio_secs:.3},\"best_total_secs\":{best:.3},\"runs\":[{}],\"text\":{}}}",
            runs_json.join(","),
            json_string(&runs.last().expect("at least one run").text)
        );
    } else {
        println!(
            "best of {repeat}: {best:.2}s total ({:.1}x RT) for {audio_secs:.1}s audio",
            audio_secs / best
        );
        println!("text: {}", runs.last().expect("at least one run").text);
    }
    Ok(())
}

struct PacedRun {
    backlog_secs: f32,
    tail_asr_secs: f32,
    polish_secs: f32,
    stop_to_text_secs: f32,
    text: String,
}

/// Replay the clip as if spoken live: each chunk is sent at the moment its
/// speech would have ended, so ASR overlaps "speaking" exactly as in a real
/// session. What remains at Flush is what a user would wait for.
fn run_paced(
    reader: BufReader<UnixStream>,
    mut writer: UnixStream,
    samples: &[f32],
    chunk_secs: f32,
    repeat: usize,
    json: bool,
    audio_secs: f32,
) -> Result<()> {
    let (event_tx, events) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = reader;
        while let Ok(Some(msg)) = read_frame::<DaemonMsg>(&mut reader) {
            if event_tx.send((msg, Instant::now())).is_err() {
                break;
            }
        }
    });

    let chunk_len = ((TARGET_RATE as f32 * chunk_secs) as usize).max(1);
    let mut runs = Vec::new();
    for i in 0..repeat {
        write_frame(
            &mut writer,
            &ClientMsg::Start(crate::config::SessionSettings::default().session()),
        )?;
        let started = Instant::now();
        let mut acked = 0usize;
        let mut sent_secs: Vec<f32> = Vec::new();
        let mut consumed = 0usize;
        for chunk in samples.chunks(chunk_len) {
            consumed += chunk.len();
            let due = started + Duration::from_secs_f32(consumed as f32 / TARGET_RATE as f32);
            loop {
                let now = Instant::now();
                if now >= due {
                    break;
                }
                match events.recv_timeout(due - now) {
                    Ok((DaemonMsg::Partial(_), _)) => acked += 1,
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => bail!("daemon connection ended"),
                }
            }
            write_frame(&mut writer, &ClientMsg::Chunk(chunk.to_vec()))?;
            sent_secs.push(chunk.len() as f32 / TARGET_RATE as f32);
        }
        let flushed_at = Instant::now();
        write_frame(&mut writer, &ClientMsg::Flush)?;

        let mut polishing_at = None;
        let (text, finished_at) = loop {
            match events
                .recv_timeout(Duration::from_secs(300))
                .context("daemon went silent after flush")?
            {
                // Only chunks acknowledged before the flush reduce the
                // backlog the user would have waited for.
                (DaemonMsg::Partial(_), at) => {
                    if at <= flushed_at {
                        acked += 1;
                    }
                }
                (DaemonMsg::Polishing, at) => polishing_at = Some(at),
                (DaemonMsg::Final(text), at) => break (text, at),
                (other, _) => bail!("unexpected reply: {other:?}"),
            }
        };
        let asr_done = polishing_at.unwrap_or(finished_at);
        let run = PacedRun {
            backlog_secs: sent_secs.iter().skip(acked).sum(),
            tail_asr_secs: (asr_done - flushed_at).as_secs_f32(),
            polish_secs: (finished_at - asr_done).as_secs_f32(),
            stop_to_text_secs: (finished_at - flushed_at).as_secs_f32(),
            text,
        };
        if !json {
            println!(
                "paced run {}: {audio_secs:.1}s audio in {} chunks | backlog at flush {:.2}s | tail asr {:.2}s | polish {:.2}s | stop-to-text {:.2}s",
                i + 1,
                sent_secs.len(),
                run.backlog_secs,
                run.tail_asr_secs,
                run.polish_secs,
                run.stop_to_text_secs,
            );
        }
        runs.push(run);
    }

    if json {
        let runs_json: Vec<String> = runs
            .iter()
            .map(|r| {
                format!(
                    "{{\"backlog_secs\":{:.3},\"tail_asr_secs\":{:.3},\"polish_secs\":{:.3},\"stop_to_text_secs\":{:.3}}}",
                    r.backlog_secs, r.tail_asr_secs, r.polish_secs, r.stop_to_text_secs
                )
            })
            .collect();
        println!(
            "{{\"audio_secs\":{audio_secs:.3},\"chunk_secs\":{chunk_secs:.3},\"runs\":[{}],\"text\":{}}}",
            runs_json.join(","),
            json_string(&runs.last().expect("at least one run").text)
        );
    } else {
        let best = runs
            .iter()
            .map(|r| r.stop_to_text_secs)
            .fold(f32::INFINITY, f32::min);
        println!("best stop-to-text of {repeat}: {best:.2}s for {audio_secs:.1}s audio");
        println!("text: {}", runs.last().expect("at least one run").text);
    }
    Ok(())
}

/// Route the bench through its own daemon: temp socket, history recording
/// off, quick idle exit. Models still come from the real data dir; the
/// resident daemon and the user's history are untouched. An explicit
/// DIKTAFOND_SOCKET wins, for benching a specific daemon.
fn isolate_daemon() {
    if std::env::var_os("DIKTAFOND_SOCKET").is_some() {
        return;
    }
    let dir = std::env::temp_dir().join("diktafon-bench");
    let _ = std::fs::create_dir_all(&dir);
    // Safety: bench mode runs before main spawns any thread.
    unsafe {
        std::env::set_var("DIKTAFOND_SOCKET", dir.join("diktafond.sock"));
        std::env::set_var("DIKTAFOND_NO_HISTORY", "1");
        std::env::set_var("DIKTAFOND_IDLE_SECS", "60");
    }
}

/// One throwaway client session so auto-spawn brings the daemon up, then its
/// connection closes and frees the daemon for our raw connection.
fn ensure_daemon() -> Result<()> {
    let warmup = DaemonClient::spawn(socket_path(), crate::daemon_bin(), None);
    warmup.chunk_tx.send(Msg::Flush)?;
    warmup.finish().context("daemon is not reachable")?;
    Ok(())
}

fn wav_samples(path: &str) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return Ok(bytes[pos + 8..pos + 8 + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
                .collect());
        }
        pos += 8 + size + (size & 1);
    }
    bail!("no data chunk in {path}; expected 16kHz mono s16 WAV");
}

fn json_string(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}
