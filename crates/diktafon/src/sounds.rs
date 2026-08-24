//! Audible push-to-talk feedback: a cue when the mic goes live (the moment
//! that is otherwise invisible), and cues for the rare cancel/error paths.
//! Deliberately no stop sound; releasing the key already has visible feedback. Cues are from uisfx.com's Minimal pack (CC0, see
//! resources/sounds/LICENSE), decoded once at startup into cached buffers and
//! played fire-and-forget.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source};
use std::cell::RefCell;
use std::io::Cursor;

/// Name and format of an output device, compared to notice the device or its
/// format changing under an open stream.
type OutputId = (String, u32, u16);

fn current_output() -> Option<OutputId> {
    let device = cpal::default_host().default_output_device()?;
    let name = device.name().ok()?;
    let config = device.default_output_config().ok()?;
    Some((name, config.sample_rate().0, config.channels()))
}

fn open_sink() -> Result<MixerDeviceSink> {
    let mut sink = DeviceSinkBuilder::open_default_sink().context("opening audio output")?;
    // Reopening on a device change would otherwise log on every swap.
    sink.log_on_drop(false);
    Ok(sink)
}

pub enum Cue {
    /// The mic is live; speech is being captured from this moment.
    Start,
    /// The session was discarded.
    Cancel,
    /// The session failed.
    Error,
}

pub struct Sounds {
    stream: RefCell<MixerDeviceSink>,
    /// The output the stream was opened against; a mismatch means the stream
    /// is stale and must be rebuilt before it is used again.
    opened: RefCell<Option<OutputId>>,
    start: SamplesBuffer,
    cancel: SamplesBuffer,
    error: SamplesBuffer,
}

impl Sounds {
    pub fn new() -> Result<Self> {
        let stream = open_sink()?;
        let decode = |name: &str, bytes: &'static [u8]| -> Result<SamplesBuffer> {
            Ok(Decoder::try_from(Cursor::new(bytes))
                .with_context(|| format!("decoding {name}"))?
                .record())
        };
        Ok(Self {
            start: decode("start.mp3", include_bytes!("../resources/sounds/start.mp3"))?,
            cancel: decode(
                "cancel.mp3",
                include_bytes!("../resources/sounds/cancel.mp3"),
            )?,
            error: decode("error.mp3", include_bytes!("../resources/sounds/error.mp3"))?,
            stream: RefCell::new(stream),
            opened: RefCell::new(current_output()),
        })
    }

    /// The output device rodio opened, for diagnosing cue playback.
    pub fn describe(&self) -> String {
        format!("{:?}", self.stream.borrow().config())
    }

    /// macOS puts a Bluetooth headset into call mode when the microphone
    /// opens, which changes the output's format (48kHz stereo to 24kHz) under
    /// the long-lived stream: the cue that plays at exactly that moment comes
    /// out broken. Selecting a different output device leaves the same stale
    /// stream behind. Rebuilding it costs a few milliseconds and only when the
    /// output actually changed.
    fn refresh_stream(&self) {
        let current = current_output();
        if current.is_none() || current == *self.opened.borrow() {
            return;
        }
        match open_sink() {
            Ok(fresh) => {
                *self.stream.borrow_mut() = fresh;
                *self.opened.borrow_mut() = current;
            }
            Err(e) => eprintln!("reopening audio output failed: {e:#}"),
        }
    }

    pub fn play(&self, cue: Cue) {
        self.refresh_stream();
        let buffer = match cue {
            Cue::Start => &self.start,
            Cue::Cancel => &self.cancel,
            Cue::Error => &self.error,
        };
        self.stream.borrow().mixer().add(buffer.clone());
    }
}
