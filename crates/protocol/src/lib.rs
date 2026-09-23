//! Streaming inference protocol between the diktafon client and the diktafond
//! daemon. Messages are bincode-encoded and length-prefixed, so the same codec
//! runs over any byte stream: a Unix socket locally, a WebSocket remotely.
//!
//! Half-close is not supported: closing either direction of the stream ends
//! the whole connection.
//!
//! A connection starts with a `Hello` exchange carrying [`PROTOCOL_VERSION`],
//! after which the daemon sends zero or more `DownloadProgress` frames (while
//! it is still provisioning models) and then exactly one `Ready`. Each
//! dictation session is then `Start`, streamed `Chunk`s (answered with
//! `Partial` transcripts), and a `Flush` answered with the `Final` polished
//! text.

pub mod history;

pub use history::HistoryEntry;

use anyhow::{Context, Result, bail};
use bincode::{Decode, Encode};
use std::io::{Read, Write};
use std::path::PathBuf;

pub const PROTOCOL_VERSION: u32 = 7;
pub const DEFAULT_APPLE_PROMPT: &str = "Touch up the raw transcript slightly so it looks a bit more like written communication.\n\nReturn only the cleaned transcript.";

/// Prefix of the daemon's handshake rejection for a version mismatch; the
/// client matches on it to decide a resident daemon needs replacing. Every
/// daemon version so far has used this exact wording.
pub const VERSION_MISMATCH_PREFIX: &str = "protocol version mismatch";
pub const MODEL_MISMATCH_PREFIX: &str = "model selection mismatch";

pub const DEFAULT_TRANSCRIPTION_MODEL: &str = "canary-1b-flash-q5-k-m";
pub const DEFAULT_POLISHING_MODEL: &str = "s1-mini-q4-k-m";
pub const MODEL_CATALOG_JSON: &str = include_str!("models.json");

/// Models a client expects the daemon to have resident. It is exchanged in the
/// handshake so the same selection contract works over local and remote
/// transports; local supervision restarts a warm daemon when it differs.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub transcription: String,
    pub polishing: String,
}

impl Default for ModelSelection {
    fn default() -> Self {
        Self {
            transcription: DEFAULT_TRANSCRIPTION_MODEL.into(),
            polishing: DEFAULT_POLISHING_MODEL.into(),
        }
    }
}

/// Where the daemon records its pid, next to the socket, so a newer client can
/// retire an older resident daemon.
pub fn pid_path() -> PathBuf {
    pid_path_for(&socket_path())
}

/// The pid file that belongs to a daemon serving `socket`.
pub fn pid_path_for(socket: &std::path::Path) -> PathBuf {
    socket.with_extension("pid")
}

/// Where the daemon mirrors its state (pid, model residency) as JSON for UI
/// surfaces like the client's menu bar. A file rather than a protocol message:
/// the daemon serves one connection at a time, and the client's own resident
/// connection would make a second status connection queue forever.
pub fn status_path() -> PathBuf {
    status_path_for(&socket_path())
}

/// The status file that belongs to a daemon serving `socket`.
pub fn status_path_for(socket: &std::path::Path) -> PathBuf {
    socket.with_extension("status.json")
}

/// Sample rate of the audio chunks the client delivers and the daemon's ASR
/// model expects.
pub const TARGET_RATE: u32 = 16_000;

/// Largest accepted frame. Chunks are silence-cut and normally span seconds;
/// this bound (~17 min of 16 kHz f32 audio) mainly guards against reading a
/// garbage length prefix from a desynced stream.
pub const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

/// In-process seam on both sides of the socket: capture feeds the client's
/// transport with it, and the daemon feeds its inference worker with it.
pub enum Msg {
    Start(SessionConfig),
    Chunk(Vec<f32>),
    Flush,
    Cancel,
    /// Rerun retained audio through the normal pipeline without recording a
    /// new dictation. The transport drives it as one session while idle and
    /// answers on `reply`; callers must send `config` with `no_history` set.
    /// Never crosses the wire: the transport expands it into Start/Chunk
    /// frames, so the daemon matches on it only for exhaustiveness.
    Reprocess(ReprocessRequest),
}

/// A retained clip plus where its rerun goes.
pub struct ReprocessRequest {
    /// Whole clip at 16 kHz mono, as retained (silences included).
    pub samples: Vec<f32>,
    /// Session settings for the rerun; `no_history` must be set.
    pub config: SessionConfig,
    /// Receives exactly one answer: the rerun, or why it never ran.
    pub reply: std::sync::mpsc::Sender<ReprocessResult>,
}

/// A finished rerun: its text plus the pipeline timings around it. Empty text
/// is a successful empty rerun, not an error; callers decide how to present
/// it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReprocessOutcome {
    pub text: String,
    pub asr_ms: u64,
    pub polish_ms: u64,
}

/// The transport's one answer to a [`Msg::Reprocess`]: `Err` carries a
/// human-readable reason (a dictation in flight, an unreachable daemon).
pub type ReprocessResult = Result<ReprocessOutcome, String>;

/// Root for everything diktafon stores: models, socket, daemon log.
/// `DIKTAFON_DATA_DIR` overrides it; otherwise the platform data dir
/// (`~/Library/Application Support/diktafon` on macOS; `$XDG_DATA_HOME` else
/// `~/.local/share`, plus `/diktafon`, on Linux).
/// Deliberately a data dir, not a cache dir: macOS may purge caches
/// under disk pressure, and multi-GB models should not be purgeable.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DIKTAFON_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .expect("no platform data directory")
        .join("diktafon")
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Where the daemon listens locally; shared so client and daemon agree without
/// the client depending on the daemon crate. `DIKTAFOND_SOCKET` overrides it
/// (winning even over a `DIKTAFON_DATA_DIR`-derived path), mainly for tests.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("DIKTAFOND_SOCKET") {
        return PathBuf::from(path);
    }
    data_dir().join("diktafond.sock")
}

/// Per-session settings the daemon forwards into the models.
///
/// Field order is the wire format: append new fields at the end, never reorder.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct SessionConfig {
    /// ISO 639-1 language hint for the ASR model, e.g. "en".
    pub language: String,
    /// S1-mini control line selecting styling, structure, and context.
    pub control_line: String,
    /// Free-form user instructions for Apple Intelligence polishing.
    pub apple_prompt: String,
    /// Opaque recording filename minted by the client, stored verbatim in the
    /// history entry so a retained WAV can be linked back to its dictation.
    /// Empty when retention is off. Not a model knob: the daemon reads it only
    /// to stamp history.
    pub recording: String,
    /// Skip the history append for this session. Retranscriptions run through
    /// the normal pipeline but must not masquerade as new dictations, so the
    /// client sets this and the daemon transcribes and polishes without
    /// recording.
    pub no_history: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            language: "en".into(),
            control_line: "[Styling: semi-formal] [Structure: prose] [Context: general]".into(),
            apple_prompt: DEFAULT_APPLE_PROMPT.into(),
            recording: String::new(),
            no_history: false,
        }
    }
}

/// Client → daemon messages.
///
/// Variant order is the wire format: append new variants at the end, never
/// reorder or remove.
#[derive(Encode, Decode, Debug, PartialEq)]
pub enum ClientMsg {
    /// First message on a new connection; the daemon replies with its own
    /// `Hello`, or `Error` on a version mismatch.
    Hello {
        version: u32,
        models: ModelSelection,
    },
    /// Begin a dictation session.
    Start(SessionConfig),
    /// 16 kHz mono f32 samples of one silence-cut chunk.
    Chunk(Vec<f32>),
    /// End the session; the daemon replies with `Final`.
    Flush,
    /// Discard the session without polishing; the daemon replies with
    /// `Aborted`.
    Cancel,
}

/// Daemon → client messages.
///
/// Variant order is the wire format: append new variants at the end, never
/// reorder or remove.
#[derive(Encode, Decode, Debug, PartialEq)]
pub enum DaemonMsg {
    /// Handshake reply carrying the daemon's protocol version.
    Hello {
        version: u32,
        models: ModelSelection,
    },
    /// Raw transcript of one chunk, sent as soon as it is transcribed.
    Partial(String),
    /// Polished text for the whole session, sent after `Flush`.
    Final(String),
    /// During the handshake this ends the connection; afterwards it ends only
    /// the current session and the connection stays usable.
    Error(String),
    /// Ack of `Cancel`; the client must discard any `Partial` or `Final` that
    /// arrived after it sent `Cancel` but before this ack.
    Aborted,
    /// The daemon is fetching a model it is missing; sent between `Hello` and
    /// `Ready` while the client waits. (v2)
    DownloadProgress {
        model: String,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    /// Startup is complete and the connection now serves sessions. Sent once
    /// after the handshake: immediately on a warm daemon, or after the last
    /// `DownloadProgress` on a cold one. (v2)
    Ready,
    /// All chunks are transcribed and the polish pass started; `Final` follows.
    /// (v3: the version was bumped because a resident v2-era daemon serving a
    /// newer client would otherwise emit a variant the client cannot decode.)
    Polishing,
}

pub fn write_frame<T: Encode>(writer: &mut impl Write, msg: &T) -> Result<()> {
    let payload =
        bincode::encode_to_vec(msg, bincode::config::standard()).context("encoding frame")?;
    if payload.len() > MAX_FRAME_LEN as usize {
        bail!("frame of {} bytes exceeds MAX_FRAME_LEN", payload.len());
    }
    write_prefixed(writer, &payload).context("writing frame")?;
    Ok(())
}

fn write_prefixed(writer: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Read one frame. `Ok(None)` means the peer closed the stream cleanly between
/// frames; a close mid-frame is an error.
pub fn read_frame<T: Decode<()>>(reader: &mut impl Read) -> Result<Option<T>> {
    let mut len_bytes = [0u8; 4];
    let mut filled = 0;
    while filled < len_bytes.len() {
        match reader.read(&mut len_bytes[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => bail!("stream closed mid frame length"),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e).context("reading frame length"),
        }
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        bail!("frame length {len} exceeds MAX_FRAME_LEN, stream is desynced");
    }
    let mut payload = vec![0u8; len as usize];
    reader
        .read_exact(&mut payload)
        .context("reading frame payload")?;
    let (msg, consumed) = bincode::decode_from_slice(&payload, bincode::config::standard())
        .context("decoding frame")?;
    if consumed != payload.len() {
        bail!("frame has {} trailing bytes", payload.len() - consumed);
    }
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn roundtrip<T: Encode + Decode<()>>(msg: &T) -> T {
        let mut buf = Vec::new();
        write_frame(&mut buf, msg).unwrap();
        read_frame(&mut Cursor::new(buf)).unwrap().unwrap()
    }

    #[test]
    fn client_messages_roundtrip() {
        let msgs = vec![
            ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                models: ModelSelection::default(),
            },
            ClientMsg::Start(SessionConfig {
                language: "en".into(),
                control_line: "[Styling: semi-formal]".into(),
                apple_prompt: "Keep product names unchanged.".into(),
                recording: "recording-2026-01-01T00-00-00.000Z.wav".into(),
                no_history: true,
            }),
            ClientMsg::Chunk(vec![0.0, -0.5, 0.25]),
            ClientMsg::Flush,
            ClientMsg::Cancel,
        ];
        for msg in msgs {
            assert_eq!(roundtrip(&msg), msg);
        }
    }

    #[test]
    fn daemon_messages_roundtrip() {
        let msgs = vec![
            DaemonMsg::Hello {
                version: PROTOCOL_VERSION,
                models: ModelSelection::default(),
            },
            DaemonMsg::Partial("hello world".into()),
            DaemonMsg::Final("Hello, world.".into()),
            DaemonMsg::Error("model exploded".into()),
            DaemonMsg::Aborted,
        ];
        for msg in msgs {
            assert_eq!(roundtrip(&msg), msg);
        }
    }

    #[test]
    fn frames_stream_back_to_back_then_clean_eof() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &ClientMsg::Chunk(vec![1.0; 1000])).unwrap();
        write_frame(&mut buf, &ClientMsg::Flush).unwrap();
        let mut cursor = Cursor::new(buf);
        assert!(matches!(
            read_frame::<ClientMsg>(&mut cursor).unwrap(),
            Some(ClientMsg::Chunk(_))
        ));
        assert_eq!(
            read_frame::<ClientMsg>(&mut cursor).unwrap(),
            Some(ClientMsg::Flush)
        );
        assert_eq!(read_frame::<ClientMsg>(&mut cursor).unwrap(), None);
    }

    /// Freezes the wire format: fails if enum variants are reordered or the
    /// bincode config changes, both of which silently break the protocol.
    #[test]
    fn hello_frame_bytes_are_stable() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                models: ModelSelection::default(),
            },
        )
        .unwrap();
        let mut expected = vec![40, 0, 0, 0, 0, 7, 22];
        expected.extend_from_slice(b"canary-1b-flash-q5-k-m");
        expected.push(14);
        expected.extend_from_slice(b"s1-mini-q4-k-m");
        assert_eq!(buf, expected);
    }

    /// Same freeze for the v2 startup variants.
    #[test]
    fn v2_frame_bytes_are_stable() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &DaemonMsg::DownloadProgress {
                model: "s1".into(),
                downloaded_bytes: 1,
                total_bytes: 2,
            },
        )
        .unwrap();
        // Variant index 5, then string length 2 + "s1", then the two varints.
        assert_eq!(buf, vec![6, 0, 0, 0, 5, 2, b's', b'1', 1, 2]);

        let mut buf = Vec::new();
        write_frame(&mut buf, &DaemonMsg::Ready).unwrap();
        assert_eq!(buf, vec![1, 0, 0, 0, 6]);

        let mut buf = Vec::new();
        write_frame(&mut buf, &DaemonMsg::Polishing).unwrap();
        assert_eq!(buf, vec![1, 0, 0, 0, 7]);
    }

    /// Same freeze for `Start`: the v6/v7 field appends (`recording`,
    /// `no_history`) must only ever extend the tail. Reordering fields or
    /// changing the string encoding silently desyncs old daemons instead of
    /// tripping the version retire, so this fails loudly instead.
    #[test]
    fn start_frame_bytes_are_stable() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &ClientMsg::Start(SessionConfig {
                language: "en".into(),
                control_line: "x".into(),
                apple_prompt: "y".into(),
                recording: String::new(),
                no_history: false,
            }),
        )
        .unwrap();
        // Variant index 1, then each string as a one-byte length plus bytes,
        // then the empty recording and the false flag.
        let expected = vec![10, 0, 0, 0, 1, 2, b'e', b'n', 1, b'x', 1, b'y', 0, 0];
        assert_eq!(buf, expected);
        // And the frozen bytes still decode.
        let decoded: ClientMsg = read_frame(&mut Cursor::new(buf)).unwrap().unwrap();
        assert!(matches!(decoded, ClientMsg::Start(_)));
    }

    #[test]
    fn oversized_length_prefix_is_rejected() {
        let mut buf = (MAX_FRAME_LEN + 1).to_le_bytes().to_vec();
        buf.extend([0u8; 8]);
        let err = read_frame::<ClientMsg>(&mut Cursor::new(buf)).unwrap_err();
        assert!(err.to_string().contains("desynced"), "{err}");
    }

    #[test]
    fn truncated_payload_is_an_error() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &ClientMsg::Flush).unwrap();
        buf.pop();
        assert!(read_frame::<ClientMsg>(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn unknown_variant_is_an_error() {
        let buf = vec![1, 0, 0, 0, 200];
        assert!(read_frame::<ClientMsg>(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn trailing_bytes_are_an_error() {
        let flush_variant_index = 3;
        let buf = vec![2, 0, 0, 0, flush_variant_index, 99];
        let err = read_frame::<ClientMsg>(&mut Cursor::new(buf)).unwrap_err();
        assert!(err.to_string().contains("trailing"), "{err}");
    }
}
