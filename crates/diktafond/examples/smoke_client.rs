//! Manual smoke client: streams a 16 kHz mono s16 WAV file through a running
//! daemon and prints the partials and final text.
//!
//! `DIKTAFOND_SOCKET=/tmp/d.sock cargo run -p diktafond --example smoke_client <wav>`

use diktafon_protocol::{
    read_frame, socket_path, write_frame, ClientMsg, DaemonMsg, SessionConfig, PROTOCOL_VERSION,
    TARGET_RATE,
};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const CHUNK_SECS: usize = 5;

fn wav_samples(path: &str) -> Vec<f32> {
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
    panic!("no data chunk in {path}");
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: smoke_client <wav>");
    let socket = std::env::var_os("DIKTAFOND_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(socket_path);
    let mut stream = UnixStream::connect(socket).expect("connecting to daemon");

    write_frame(&mut stream, &ClientMsg::Hello { version: PROTOCOL_VERSION }).unwrap();
    match read_frame::<DaemonMsg>(&mut stream).unwrap() {
        Some(DaemonMsg::Hello { .. }) => {}
        other => panic!("handshake failed: {other:?}"),
    }
    loop {
        match read_frame::<DaemonMsg>(&mut stream).unwrap() {
            Some(DaemonMsg::Ready) => break,
            Some(DaemonMsg::DownloadProgress { model, downloaded_bytes, total_bytes }) => {
                println!("daemon downloading {model}: {downloaded_bytes}/{total_bytes}")
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    write_frame(&mut stream, &ClientMsg::Start(SessionConfig::default())).unwrap();
    for chunk in wav_samples(&path).chunks(TARGET_RATE as usize * CHUNK_SECS) {
        write_frame(&mut stream, &ClientMsg::Chunk(chunk.to_vec())).unwrap();
    }
    write_frame(&mut stream, &ClientMsg::Flush).unwrap();

    loop {
        match read_frame::<DaemonMsg>(&mut stream).unwrap() {
            Some(DaemonMsg::Partial(text)) => println!("partial: {text}"),
            Some(DaemonMsg::Polishing) => println!("polishing..."),
            Some(DaemonMsg::Final(text)) => {
                println!("final: {text}");
                return;
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }
}
