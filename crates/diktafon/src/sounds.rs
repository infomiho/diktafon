//! Audible push-to-talk feedback: a cue when the mic goes live (the moment
//! that is otherwise invisible), and cues for the rare cancel/error paths.
//! Deliberately no stop sound; releasing the key already has visible feedback.
//! Cues are from uisfx.com's Minimal pack (CC0, see resources/sounds/LICENSE),
//! decoded once at startup into cached buffers.
//!
//! Following Handy's audio_feedback: the output device is opened per cue and
//! held until the sound ends, rather than kept open for the process lifetime.
//! A long-lived stream goes stale the moment the device changes format under
//! it, which macOS does routinely: opening the microphone switches a Bluetooth
//! headset to call mode, so the same AirPods that were 48kHz stereo become
//! 24kHz. Opening at play time always matches the device as it is now, and
//! also follows the user switching outputs entirely.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, DeviceSinkBuilder, Player, Source};
use std::io::Cursor;
use std::time::{Duration, Instant};

/// Opening the microphone makes macOS switch a shared Bluetooth headset to
/// call mode, measured at ~145ms on AirPods Pro. Audio played mid-switch comes
/// out broken, so the start cue waits for it; the wait runs on the playback
/// thread and never delays the dictation itself.
const ROUTE_SWITCH: Duration = Duration::from_millis(400);
const ROUTE_POLL: Duration = Duration::from_millis(20);

fn output_rate() -> Option<(String, u32)> {
    let device = cpal::default_host().default_output_device()?;
    Some((
        device.name().ok()?,
        device.default_output_config().ok()?.sample_rate().0,
    ))
}

fn input_rate() -> Option<(String, u32)> {
    let device = cpal::default_host().default_input_device()?;
    Some((
        device.name().ok()?,
        device.default_input_config().ok()?.sample_rate().0,
    ))
}

/// A switch is pending only while the shared device still runs its output
/// faster than its input; once in call mode the rates match and this returns
/// immediately.
fn await_route_settled() {
    let (Some((out_name, out_rate)), Some((in_name, in_rate))) = (output_rate(), input_rate())
    else {
        return;
    };
    if out_name != in_name || out_rate <= in_rate {
        return;
    }
    let deadline = Instant::now() + ROUTE_SWITCH;
    while Instant::now() < deadline {
        std::thread::sleep(ROUTE_POLL);
        if output_rate().is_some_and(|(_, rate)| rate != out_rate) {
            // The format flipped; give the device a beat to stabilise.
            std::thread::sleep(ROUTE_POLL);
            return;
        }
    }
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
    start: SamplesBuffer,
    cancel: SamplesBuffer,
    error: SamplesBuffer,
}

impl Sounds {
    pub fn new() -> Result<Self> {
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
        })
    }

    /// The current default output, for diagnosing cue playback.
    pub fn describe() -> String {
        use cpal::traits::{DeviceTrait, HostTrait};
        let Some(device) = cpal::default_host().default_output_device() else {
            return "no default output".into();
        };
        let name = device.name().unwrap_or_else(|_| "?".into());
        match device.default_output_config() {
            Ok(config) => format!(
                "{name} ({} Hz, {} ch)",
                config.sample_rate().0,
                config.channels()
            ),
            Err(e) => format!("{name} (config unavailable: {e})"),
        }
    }

    /// Fire and forget: playing holds a thread for the length of the cue
    /// (~0.4s), which must not delay the dictation that triggered it.
    pub fn play(&self, cue: Cue) {
        let buffer = match &cue {
            Cue::Start => self.start.clone(),
            Cue::Cancel => self.cancel.clone(),
            Cue::Error => self.error.clone(),
        };
        let wait_for_route = matches!(cue, Cue::Start);
        std::thread::spawn(move || {
            if wait_for_route {
                await_route_settled();
            }
            if let Err(e) = play_on_default_device(buffer) {
                eprintln!("playing feedback sound failed: {e:#}");
            }
        });
    }
}

fn play_on_default_device(buffer: SamplesBuffer) -> Result<()> {
    let mut sink = DeviceSinkBuilder::open_default_sink().context("opening audio output")?;
    // The stream is dropped per cue; the default message would be noise.
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());
    player.append(buffer);
    // Dropping the stream early would cut the cue off mid-sound.
    player.sleep_until_end();
    Ok(())
}
