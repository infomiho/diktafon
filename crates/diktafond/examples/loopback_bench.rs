//! Measures what the client/daemon split costs versus in-process inference:
//! per-chunk (send → Partial) and finish (Flush → Final) latency on identical
//! audio, plus the pure transport roundtrip via Cancel → Aborted, which does
//! no inference work.
//!
//! `cargo run --release -p diktafond --example loopback_bench`

use diktafon_protocol::{
    ClientMsg, DaemonMsg, Msg, PROTOCOL_VERSION, TARGET_RATE, read_frame, write_frame,
};
use diktafond::Inference;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const CHUNK_SECS: usize = 5;
const TRANSPORT_ROUNDTRIPS: usize = 200;

fn wav_samples(path: &PathBuf) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("reading wav");
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return bytes[pos + 8..pos + 8 + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
                .collect();
        }
        pos += 8 + size + (size & 1);
    }
    panic!("no data chunk");
}

fn stats(label: &str, samples: &[Duration]) {
    let mut sorted = samples.to_vec();
    sorted.sort();
    let median = sorted[sorted.len() / 2];
    println!(
        "  {label}: min {:?}  median {median:?}  max {:?}  (n={})",
        sorted[0],
        sorted[sorted.len() - 1],
        sorted.len()
    );
}

fn main() {
    let models_dir = diktafon_protocol::models_dir();
    let clip = diktafon_protocol::data_dir().join("eval-own/01.wav");
    let samples = wav_samples(&clip);
    let chunks: Vec<Vec<f32>> = samples
        .chunks(TARGET_RATE as usize * CHUNK_SECS)
        .map(|c| c.to_vec())
        .collect();
    println!("clip: {} chunks of ≤{CHUNK_SECS}s", chunks.len());

    // In-process baseline.
    println!("\nin-process:");
    let mut chunk_times = Vec::new();
    {
        let inference = Inference::spawn(&models_dir, None, None).expect("loading models");
        // Warm up Metal shaders and caches.
        inference
            .chunk_tx
            .send(Msg::Chunk(chunks[0].clone()))
            .unwrap();
        wait_partial_inproc(&inference);
        inference.chunk_tx.send(Msg::Cancel).unwrap();
        wait_aborted_inproc(&inference);

        for chunk in &chunks {
            let start = Instant::now();
            inference.chunk_tx.send(Msg::Chunk(chunk.clone())).unwrap();
            wait_partial_inproc(&inference);
            chunk_times.push(start.elapsed());
        }
        let start = Instant::now();
        inference.chunk_tx.send(Msg::Flush).unwrap();
        let final_time = loop {
            if let DaemonMsg::Final(_) = inference.recv_event(Duration::from_secs(60)).unwrap() {
                break start.elapsed();
            }
        };
        stats("chunk -> partial", &chunk_times);
        println!("  flush -> final: {final_time:?}");

        let mut rtts = Vec::new();
        for _ in 0..TRANSPORT_ROUNDTRIPS {
            let start = Instant::now();
            inference.chunk_tx.send(Msg::Cancel).unwrap();
            wait_aborted_inproc(&inference);
            rtts.push(start.elapsed());
        }
        stats("cancel -> aborted (no inference)", &rtts);
    }
    // Dropping the Inference frees the models before the daemon loads its own.

    // Loopback through the real daemon.
    println!("\nunix socket loopback:");
    let socket = std::env::temp_dir().join(format!("dkt-bench-{}.sock", std::process::id()));
    let daemon_bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("diktafond");
    let mut daemon = std::process::Command::new(&daemon_bin)
        .env("DIKTAFOND_SOCKET", &socket)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawning diktafond");
    while !socket.exists() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let stream = UnixStream::connect(&socket).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    write_frame(
        &mut writer,
        &ClientMsg::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .unwrap();
    loop {
        if let DaemonMsg::Ready = read_frame::<DaemonMsg>(&mut reader)
            .unwrap()
            .expect("daemon closed")
        {
            break;
        }
    }

    let wait_partial = |reader: &mut BufReader<UnixStream>| loop {
        if let DaemonMsg::Partial(_) = read_frame::<DaemonMsg>(reader).unwrap().unwrap() {
            return;
        }
    };
    let wait_aborted = |reader: &mut BufReader<UnixStream>| loop {
        if let DaemonMsg::Aborted = read_frame::<DaemonMsg>(reader).unwrap().unwrap() {
            return;
        }
    };

    write_frame(&mut writer, &ClientMsg::Chunk(chunks[0].clone())).unwrap();
    wait_partial(&mut reader);
    write_frame(&mut writer, &ClientMsg::Cancel).unwrap();
    wait_aborted(&mut reader);

    let mut chunk_times = Vec::new();
    for chunk in &chunks {
        let start = Instant::now();
        write_frame(&mut writer, &ClientMsg::Chunk(chunk.clone())).unwrap();
        wait_partial(&mut reader);
        chunk_times.push(start.elapsed());
    }
    let start = Instant::now();
    write_frame(&mut writer, &ClientMsg::Flush).unwrap();
    let final_time = loop {
        if let DaemonMsg::Final(_) = read_frame::<DaemonMsg>(&mut reader).unwrap().unwrap() {
            break start.elapsed();
        }
    };
    stats("chunk -> partial", &chunk_times);
    println!("  flush -> final: {final_time:?}");

    let mut rtts = Vec::new();
    for _ in 0..TRANSPORT_ROUNDTRIPS {
        let start = Instant::now();
        write_frame(&mut writer, &ClientMsg::Cancel).unwrap();
        wait_aborted(&mut reader);
        rtts.push(start.elapsed());
    }
    stats("cancel -> aborted (no inference)", &rtts);

    let _ = std::process::Command::new("kill")
        .args(["-TERM", &daemon.id().to_string()])
        .status();
    let _ = daemon.wait();
}

fn wait_partial_inproc(inference: &Inference) {
    loop {
        if let DaemonMsg::Partial(_) = inference.recv_event(Duration::from_secs(60)).unwrap() {
            return;
        }
    }
}

fn wait_aborted_inproc(inference: &Inference) {
    loop {
        if let DaemonMsg::Aborted = inference.recv_event(Duration::from_secs(60)).unwrap() {
            return;
        }
    }
}
