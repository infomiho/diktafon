//! One-shot playback of a retained recording through the system output, for
//! the History pane. Same route as the feedback cues (NSSound over in-memory
//! WAV bytes), so playback follows route changes like everything else
//! audible. Lives on the gpui main thread inside the settings window; AppKit
//! owns the object.

use anyhow::{Context, Result};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::NSData;
use std::path::Path;

/// At most one recording plays at a time; starting another stops the first.
/// The History list tracks *which* recording is playing; the player only
/// holds the sound.
pub struct RecordingPlayer {
    current: Option<Retained<NSSound>>,
}

impl RecordingPlayer {
    pub fn new() -> Self {
        Self { current: None }
    }

    /// Whether the current sound is still producing audio. A finished clip
    /// reports itself, so the row can drop its playing state on a timer.
    pub fn is_playing(&self) -> bool {
        self.current.as_ref().is_some_and(|sound| sound.isPlaying())
    }

    /// Play the WAV at `path`, stopping whatever is playing first. Errors
    /// leave the player stopped: a row claiming a clip that never started is
    /// worse than silence.
    pub fn play(&mut self, path: &Path) -> Result<()> {
        self.stop();
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let data = NSData::with_bytes(&bytes);
        let sound = NSSound::initWithData(NSSound::alloc(), &data).context("decoding audio")?;
        sound.play();
        self.current = Some(sound);
        Ok(())
    }

    /// Stop playback, if any. Never fails.
    pub fn stop(&mut self) {
        if let Some(sound) = self.current.take()
            && sound.isPlaying()
        {
            sound.stop();
        }
    }
}

impl Default for RecordingPlayer {
    fn default() -> Self {
        Self::new()
    }
}
