//! Types shared across the client/daemon boundary. The full streaming
//! inference protocol will live here as it is defined (M2 epic).

/// Sample rate of the audio chunks the client delivers and the daemon's ASR
/// model expects.
pub const TARGET_RATE: u32 = 16_000;

/// Chunks stream in during a recording session; `Flush` ends the session and
/// requests the final polished text.
pub enum Msg {
    Chunk(Vec<f32>),
    Flush,
}
