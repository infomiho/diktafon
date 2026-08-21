//! Headless benchmark mode: `diktafon --transcribe-file x.wav [--repeat N]
//! [--json]`. Feeds a 16kHz mono s16 WAV through the daemon (auto-spawning it
//! like a normal session would) and reports per-stage timings; the daemon's
//! `Polishing` frame marks the ASR/polish boundary.

use crate::transport::DaemonClient;
use anyhow::{Context, Result, bail};
use diktafon_protocol::{
    ClientMsg, DaemonMsg, Msg, PROTOCOL_VERSION, TARGET_RATE, read_frame, socket_path, write_frame,
};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::Instant;

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

    let samples = wav_samples(path)?;
    let audio_secs = samples.len() as f32 / TARGET_RATE as f32;

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

    let mut runs = Vec::new();
    for i in 0..repeat {
        let start = Instant::now();
        write_frame(
            &mut writer,
            &ClientMsg::Start(crate::config::CONFIG.session()),
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
