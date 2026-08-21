//! Every user-tunable setting in one place. Still compile-time constants; a
//! config file can replace this later without touching the consumers.

use diktafon_protocol::SessionConfig;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};

pub struct Config {
    /// Push-to-talk hotkey.
    pub hotkey_modifiers: Modifiers,
    pub hotkey_code: Code,
    /// ISO 639-1 language hint for the ASR model.
    pub language: &'static str,
    /// S1-mini control line selecting styling, structure, and context.
    pub control_line: &'static str,
    /// Silero speech probability threshold.
    pub speech_threshold: f32,
    /// Consecutive speech frames (30ms each) before speech onset.
    pub onset_frames: usize,
    /// Pre-onset audio kept, in frames.
    pub prefill_frames: usize,
    /// Non-speech frames before a speech segment is declared over.
    pub hangover_frames: usize,
    /// Speech segments shorter than this are merged with the next one instead
    /// of paying a per-chunk ASR roundtrip.
    pub min_chunk_secs: f32,
}

/// Handy's tuned Silero values; language and control line match the daemon's
/// own defaults.
pub const CONFIG: Config = Config {
    hotkey_modifiers: Modifiers::ALT,
    hotkey_code: Code::Space,
    language: "en",
    control_line: "[Styling: semi-formal] [Structure: prose] [Context: general]",
    speech_threshold: 0.3,
    onset_frames: 2,
    prefill_frames: 15,
    hangover_frames: 15,
    min_chunk_secs: 1.5,
};

impl Config {
    pub fn hotkey(&self) -> HotKey {
        HotKey::new(Some(self.hotkey_modifiers), self.hotkey_code)
    }

    pub fn session(&self) -> SessionConfig {
        SessionConfig {
            language: self.language.into(),
            control_line: self.control_line.into(),
        }
    }
}
