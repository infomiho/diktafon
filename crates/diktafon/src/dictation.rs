//! GPUI-side state of the dictation pipeline, driven by events from the
//! hotkey/capture thread and the daemon transport. The pill overlay renders
//! from this entity.

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{App, AppContext, Entity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Hotkey pressed; waiting for the mic to actually deliver samples.
    Arming,
    Recording,
    /// Hotkey released; remaining chunks are being transcribed.
    Transcribing,
    /// The daemon started the polish pass; the final text is imminent.
    Polishing,
}

pub enum PhaseEvent {
    RecordingArmed,
    RecordingStarted,
    RecordingStopped,
    PolishingStarted,
    /// A chunk finished transcribing while the session is still running.
    Partial(String),
    /// The session produced a final text, an error, or nothing.
    SessionEnded {
        error: Option<String>,
    },
    /// The daemon is still fetching a model (first run); sessions wait on it.
    DownloadProgress {
        model: String,
        percent: u8,
    },
    /// The daemon finished provisioning and can serve sessions.
    DownloadFinished,
}

pub struct Dictation {
    pub phase: Phase,
    /// Transcribed text accumulated so far in the current session.
    pub partial: String,
    pub last_error: Option<String>,
    /// Model download underway on the daemon, shown instead of the session
    /// content so a first run does not look like a hang.
    pub download: Option<Download>,
}

#[derive(Clone, PartialEq)]
pub struct Download {
    pub model: String,
    pub percent: u8,
}

impl Dictation {
    /// Create the entity and a receive loop that applies `events` to it,
    /// notifying observers on every change (Zed's repl-kernel pattern). An
    /// async channel means no idle wakeups and one observer effect per event,
    /// so even brief phases are observable.
    pub fn spawn(cx: &mut App, mut events: UnboundedReceiver<PhaseEvent>) -> Entity<Dictation> {
        let entity = cx.new(|_| Dictation {
            phase: Phase::Idle,
            partial: String::new(),
            last_error: None,
            download: None,
        });
        cx.spawn({
            let entity = entity.clone();
            async move |cx| {
                while let Some(event) = events.next().await {
                    entity.update(cx, |dictation, cx| {
                        dictation.apply(event);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
        entity
    }

    fn apply(&mut self, event: PhaseEvent) {
        self.phase = match event {
            PhaseEvent::RecordingArmed => Phase::Arming,
            PhaseEvent::RecordingStarted => {
                self.partial.clear();
                Phase::Recording
            }
            PhaseEvent::Partial(text) => {
                if !self.partial.is_empty() {
                    self.partial.push(' ');
                }
                self.partial.push_str(&text);
                self.phase
            }
            PhaseEvent::RecordingStopped => Phase::Transcribing,
            // Only a live session's polish matters; a stale frame from a
            // timed-out session must not flip Idle or a new Recording.
            PhaseEvent::PolishingStarted if self.phase == Phase::Transcribing => Phase::Polishing,
            PhaseEvent::PolishingStarted => self.phase,
            PhaseEvent::SessionEnded { error } => {
                self.last_error = error;
                Phase::Idle
            }
            PhaseEvent::DownloadProgress { model, percent } => {
                self.download = Some(Download { model, percent });
                self.phase
            }
            PhaseEvent::DownloadFinished => {
                self.download = None;
                self.phase
            }
        };
    }
}
