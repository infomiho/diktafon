//! Audible push-to-talk feedback: a cue when the mic goes live (the moment
//! that is otherwise invisible), and cues for the rare cancel/error paths.
//! Deliberately no stop sound; releasing the key already has visible feedback.
//! Cues are from uisfx.com's Minimal pack (CC0, see resources/sounds/LICENSE).
//!
//! Played through NSSound rather than an audio library holding its own output
//! stream. Opening the microphone makes macOS switch a shared Bluetooth
//! headset to call mode, and a stream opened around that moment plays back
//! broken; measured against `afplay`, which stays clean through the same
//! window, so the route is fine and the fault was in driving it ourselves.
//! AppKit follows the route change for us.

use anyhow::{Context, Result};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::NSData;

#[derive(Clone, Copy, Debug)]
pub enum Cue {
    /// The mic is live; speech is being captured from this moment.
    Start,
    /// The session was discarded.
    Cancel,
    /// The session failed.
    Error,
}

pub struct Sounds {
    start: Retained<NSSound>,
    cancel: Retained<NSSound>,
    error: Retained<NSSound>,
}

impl Sounds {
    pub fn new() -> Result<Self> {
        let load = |name: &str, bytes: &'static [u8]| -> Result<Retained<NSSound>> {
            let data = NSData::with_bytes(bytes);
            NSSound::initWithData(NSSound::alloc(), &data)
                .with_context(|| format!("loading {name}"))
        };
        Ok(Self {
            start: load("start.mp3", include_bytes!("../resources/sounds/start.mp3"))?,
            cancel: load(
                "cancel.mp3",
                include_bytes!("../resources/sounds/cancel.mp3"),
            )?,
            error: load("error.mp3", include_bytes!("../resources/sounds/error.mp3"))?,
        })
    }

    pub fn play(&self, cue: Cue) {
        let sound = match cue {
            Cue::Start => &self.start,
            Cue::Cancel => &self.cancel,
            Cue::Error => &self.error,
        };
        // A cue still playing from a rapid previous session would otherwise
        // make this play() a no-op.
        if sound.isPlaying() {
            sound.stop();
        }
        sound.play();
    }
}
