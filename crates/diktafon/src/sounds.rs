//! Audible push-to-talk feedback: a cue when the mic goes live (the moment
//! that is otherwise invisible), and cues for the rare cancel/error paths.
//! Deliberately no stop sound; releasing the key already has visible feedback. Cues are from uisfx.com's Minimal pack (CC0, see
//! resources/sounds/LICENSE), decoded once at startup into cached buffers and
//! played fire-and-forget.

use anyhow::{Context, Result};
use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source};
use std::io::Cursor;

pub enum Cue {
    /// The mic is live; speech is being captured from this moment.
    Start,
    /// The session was discarded.
    Cancel,
    /// The session failed.
    Error,
}

pub struct Sounds {
    stream: MixerDeviceSink,
    start: SamplesBuffer,
    cancel: SamplesBuffer,
    error: SamplesBuffer,
}

impl Sounds {
    pub fn new() -> Result<Self> {
        let stream = DeviceSinkBuilder::open_default_sink().context("opening audio output")?;
        let decode = |name: &str, bytes: &'static [u8]| -> Result<SamplesBuffer> {
            Ok(Decoder::try_from(Cursor::new(bytes))
                .with_context(|| format!("decoding {name}"))?
                .record())
        };
        Ok(Self {
            start: decode("start.mp3", include_bytes!("../resources/sounds/start.mp3"))?,
            cancel: decode("cancel.mp3", include_bytes!("../resources/sounds/cancel.mp3"))?,
            error: decode("error.mp3", include_bytes!("../resources/sounds/error.mp3"))?,
            stream,
        })
    }

    pub fn play(&self, cue: Cue) {
        let buffer = match cue {
            Cue::Start => &self.start,
            Cue::Cancel => &self.cancel,
            Cue::Error => &self.error,
        };
        self.stream.mixer().add(buffer.clone());
    }
}
