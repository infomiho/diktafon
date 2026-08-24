//! One dictation, from keypress to pasted text. The hotkey loop only tells
//! this module that the key went down, came up, or that Escape was pressed;
//! everything the gesture implies — arming the microphone, streaming to the
//! daemon, cues, pasting, timings, and the phase events the UI mirrors — is
//! decided here, so the lifecycle can be read in one place.

use crate::capture::{Recorder, Session};
use crate::config::SessionSettings;
use crate::dictation::PhaseEvent;
use crate::transport::DaemonClient;
use crate::{paste, sounds, stats};
use diktafon_protocol::Msg;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a mic may take to deliver its first samples; Bluetooth devices
/// can need hundreds of milliseconds.
const MIC_READY_TIMEOUT: Duration = Duration::from_millis(1500);

/// How a dictation ended, as recorded in the timings.
#[derive(Debug, PartialEq)]
enum Outcome {
    Pasted,
    /// Nothing to paste: silence, or too little speech to transcribe.
    Empty,
    Failed,
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Outcome::Pasted => "pasted",
            Outcome::Empty => "empty",
            Outcome::Failed => "error",
        }
    }
}

/// The in-flight dictation: the live capture session plus the instants the
/// timings are derived from, so a cancel discards both together.
struct Live {
    session: Session,
    pressed_at: Instant,
    mic_ready_ms: u64,
}

pub struct Dictations {
    recorder: Recorder,
    daemon: DaemonClient,
    /// `None` when the audio device could not be opened at all; dictation
    /// still works, silently.
    sounds: Option<sounds::Sounds>,
    settings: Arc<Mutex<SessionSettings>>,
    phases: futures::channel::mpsc::UnboundedSender<PhaseEvent>,
    /// Which physical key types "v" under the current layout, refreshed by the
    /// UI thread as each dictation arms.
    v_keycode: Arc<AtomicU32>,
    live: Option<Live>,
}

impl Dictations {
    pub fn new(
        recorder: Recorder,
        daemon: DaemonClient,
        settings: Arc<Mutex<SessionSettings>>,
        phases: futures::channel::mpsc::UnboundedSender<PhaseEvent>,
        v_keycode: Arc<AtomicU32>,
    ) -> Self {
        let sounds = match sounds::Sounds::new() {
            Ok(sounds) => Some(sounds),
            Err(e) => {
                eprintln!("feedback sounds disabled: {e:#}");
                None
            }
        };
        Self {
            recorder,
            daemon,
            sounds,
            settings,
            phases,
            v_keycode,
            live: None,
        }
    }

    /// Arm the microphone and start streaming. Ignored while a dictation is
    /// already in flight, so a repeated key-down cannot open a second one.
    pub fn press(&mut self) {
        if self.live.is_some() {
            return;
        }
        let pressed_at = Instant::now();
        // Before the recorder: the daemon may have to be spawned and load its
        // models, and that runs while the user is still speaking.
        let _ = self
            .daemon
            .chunk_tx
            .send(Msg::Start(self.settings.lock().unwrap().session()));
        let session = match self.recorder.start(self.daemon.chunk_tx.clone()) {
            Ok(session) => session,
            Err(e) => {
                // The daemon is already holding the session started above;
                // without a Cancel it would wait for audio never coming.
                let _ = self.daemon.chunk_tx.send(Msg::Cancel);
                eprintln!("failed to start recording: {e:#}");
                self.play(sounds::Cue::Error);
                self.ended(Some("Microphone unavailable".into()), false);
                return;
            }
        };
        self.emit(PhaseEvent::RecordingArmed);
        // Stream::play() returning does not mean samples flow yet; wait so
        // slow mics don't eat first words. A queued release is handled next.
        if !session.wait_until_live(MIC_READY_TIMEOUT) {
            eprintln!("microphone produced no samples; is another app holding it?");
            // stop() flushes the (empty) session to the daemon; consume its
            // result so it cannot be misdelivered to the next dictation.
            session.stop();
            let _ = self.daemon.finish();
            self.recorder.mark_stream_failed();
            self.play(sounds::Cue::Error);
            self.ended(Some("Microphone unavailable".into()), false);
            return;
        }
        self.play(sounds::Cue::Start);
        println!("recording...");
        self.live = Some(Live {
            session,
            pressed_at,
            mic_ready_ms: pressed_at.elapsed().as_millis() as u64,
        });
        self.emit(PhaseEvent::RecordingStarted);
    }

    /// Stop recording, wait for the transcript, and paste it.
    pub fn release(&mut self) {
        let Some(live) = self.live.take() else {
            return;
        };
        let stopped_at = Instant::now();
        live.session.stop();
        self.emit(PhaseEvent::RecordingStopped);

        let (error, outcome) = match self.daemon.finish() {
            Ok(text) if text.is_empty() => {
                println!("(no speech)");
                (None, Outcome::Empty)
            }
            Ok(text) => {
                println!(">>> {text}");
                let error = self.paste(&text);
                println!("stop-to-paste: {:.2?}", stopped_at.elapsed());
                let outcome = if error.is_some() {
                    Outcome::Failed
                } else {
                    Outcome::Pasted
                };
                (error, outcome)
            }
            Err(e) => {
                self.play(sounds::Cue::Error);
                eprintln!("inference error: {e:#}");
                (Some("Transcription failed".to_string()), Outcome::Failed)
            }
        };

        stats::append(&stats::Timing {
            at: diktafon_protocol::history::now_rfc3339(),
            mic_ready_ms: live.mic_ready_ms,
            recording_secs: recording_secs(live.pressed_at, live.mic_ready_ms, stopped_at),
            stop_to_paste_ms: stopped_at.elapsed().as_millis() as u64,
            cold_start: self.was_cold_start(live.pressed_at),
            outcome: outcome.label().into(),
        });
        // An empty transcript ends like a cancel: the pill plays its quiet
        // ending, keeping the success bloom to mean words actually landed.
        self.ended(error, outcome == Outcome::Empty);
    }

    /// Discard the dictation in flight. Does nothing when there is none.
    pub fn cancel(&mut self) {
        let Some(live) = self.live.take() else {
            return;
        };
        live.session.cancel();
        self.play(sounds::Cue::Cancel);
        println!("cancelled");
        self.ended(None, true);
    }

    fn paste(&self, text: &str) -> Option<String> {
        let keycode = self.v_keycode.load(Ordering::Relaxed) as u16;
        paste::insert(text, keycode).err().map(|e| {
            eprintln!("paste failed (Accessibility permission?): {e:#}");
            "Paste needs Accessibility".to_string()
        })
    }

    /// Whether the daemon was spawned for this dictation, so the wait included
    /// loading models. The marker is consumed either way, so an older spawn
    /// can never label a later dictation cold.
    fn was_cold_start(&self, pressed_at: Instant) -> bool {
        self.daemon
            .spawned_at
            .lock()
            .unwrap()
            .take()
            .is_some_and(|at| at >= pressed_at)
    }

    fn play(&self, cue: sounds::Cue) {
        if self.settings.lock().unwrap().sound_cues
            && let Some(sounds) = &self.sounds
        {
            sounds.play(cue);
        }
    }

    fn emit(&self, event: PhaseEvent) {
        let _ = self.phases.unbounded_send(event);
    }

    fn ended(&self, error: Option<String>, cancelled: bool) {
        self.emit(PhaseEvent::SessionEnded { error, cancelled });
    }
}

/// Speech time only: the wait for the microphone belongs to arming, not to
/// how long the user spoke.
fn recording_secs(pressed_at: Instant, mic_ready_ms: u64, stopped_at: Instant) -> f32 {
    ((stopped_at - pressed_at).as_secs_f32() - mic_ready_ms as f32 / 1000.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_labels_match_the_timings_format() {
        assert_eq!(Outcome::Pasted.label(), "pasted");
        assert_eq!(Outcome::Empty.label(), "empty");
        assert_eq!(Outcome::Failed.label(), "error");
    }

    #[test]
    fn recording_time_excludes_the_microphone_wait() {
        let pressed = Instant::now();
        let secs = recording_secs(pressed, 200, pressed + Duration::from_millis(1200));
        assert!((secs - 1.0).abs() < 0.01, "{secs}");

        // A release inside the arming wait is not negative speech.
        assert_eq!(
            recording_secs(pressed, 200, pressed + Duration::from_millis(100)),
            0.0
        );
    }
}
