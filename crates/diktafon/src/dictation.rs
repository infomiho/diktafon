//! GPUI-side state of the dictation pipeline, driven by events from the
//! hotkey/capture thread and the daemon transport. The pill overlay renders
//! from this entity.

use futures::channel::mpsc::UnboundedReceiver;
use futures::StreamExt;
use gpui::{App, AppContext, Entity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Recording,
    /// Hotkey released; remaining chunks are being transcribed.
    Transcribing,
    /// The daemon started the polish pass; the final text is imminent.
    Polishing,
}

pub enum PhaseEvent {
    RecordingStarted,
    RecordingStopped,
    PolishingStarted,
    /// The session produced a final text, an error, or nothing.
    SessionEnded { error: Option<String> },
}

pub struct Dictation {
    pub phase: Phase,
    pub last_error: Option<String>,
}

impl Dictation {
    /// Create the entity and a receive loop that applies `events` to it,
    /// notifying observers on every change (Zed's repl-kernel pattern). An
    /// async channel means no idle wakeups and one observer effect per event,
    /// so even brief phases are observable.
    pub fn spawn(cx: &mut App, mut events: UnboundedReceiver<PhaseEvent>) -> Entity<Dictation> {
        let entity = cx.new(|_| Dictation { phase: Phase::Idle, last_error: None });
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
            PhaseEvent::RecordingStarted => Phase::Recording,
            PhaseEvent::RecordingStopped => Phase::Transcribing,
            // Only a live session's polish matters; a stale frame from a
            // timed-out session must not flip Idle or a new Recording.
            PhaseEvent::PolishingStarted if self.phase == Phase::Transcribing => Phase::Polishing,
            PhaseEvent::PolishingStarted => self.phase,
            PhaseEvent::SessionEnded { error } => {
                self.last_error = error;
                Phase::Idle
            }
        };
    }
}
