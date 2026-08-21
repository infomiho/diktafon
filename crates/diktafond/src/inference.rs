use anyhow::{Context, Result, anyhow};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use transcribe_rs::onnx::Quantization;
use transcribe_rs::onnx::cohere::{CohereModel, CohereParams};

use diktafon_protocol::{DaemonMsg, Msg, SessionConfig, TARGET_RATE};

use crate::llm::Polisher;

/// How often the worker wakes to check for idleness.
const IDLE_POLL: Duration = Duration::from_secs(30);

/// The resident models cost gigabytes of RAM; after this long without any
/// message they are dropped and reloaded on demand (`DIKTAFOND_IDLE_SECS`
/// overrides, mainly for testing).
const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);

fn idle_unload_after() -> Duration {
    std::env::var("DIKTAFOND_IDLE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(IDLE_UNLOAD)
}

struct Models {
    asr: CohereModel,
    polisher: Polisher,
}

fn load_models(models_dir: &Path) -> Result<Models> {
    let start = Instant::now();
    let asr = CohereModel::load(&models_dir.join("cohere-int8"), &Quantization::Int8)
        .context("loading ASR model")?;
    let polisher =
        Polisher::load(&models_dir.join("s1-mini-q4_k_m.gguf")).context("loading LLM")?;
    println!("models loaded in {:.2?}", start.elapsed());
    Ok(Models { asr, polisher })
}

/// Upper bound on transcribing the queued chunks plus one polish pass. Hit when
/// the worker thread died or is far behind; `finish` errors instead of blocking
/// forever, and a late reply from a slow worker is discarded, not misdelivered.
const FINISH_TIMEOUT: Duration = Duration::from_secs(60);

/// Very short clips make ASR models invent text; Handy's guard zero-pads
/// anything under [`MIN_CLIP`] out to [`PADDED_CLIP`] before transcription.
const MIN_CLIP: usize = TARGET_RATE as usize;
const PADDED_CLIP: usize = TARGET_RATE as usize * 5 / 4;

fn pad_short_clip(samples: &mut Vec<f32>) {
    if !samples.is_empty() && samples.len() < MIN_CLIP {
        samples.resize(PADDED_CLIP, 0.0);
    }
}

/// Native inference (ONNX, llama.cpp) can panic. Uncaught, that kills the
/// worker thread and every later dictation hangs, so panics become errors.
fn catch_panic<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        Err(anyhow!("{what} panicked: {msg}"))
    })
}

/// Worker thread owning the resident ASR and polish models. Chunks stream in
/// during recording and are transcribed as they arrive; `Flush` ends a session,
/// triggering one polish pass over the joined parts. The worker reports back as
/// [`DaemonMsg`] events: a `Partial` per chunk, then `Final` (or `Aborted`
/// after a `Cancel`).
pub struct Inference {
    pub chunk_tx: mpsc::Sender<Msg>,
    /// Wrapped in a Mutex so the daemon's relay thread can receive events while
    /// another thread owns the `Inference`; there is only ever one consumer at
    /// a time.
    events_rx: Mutex<mpsc::Receiver<DaemonMsg>>,
    /// `Final`s still owed by sessions whose `finish` timed out. Each `Flush`
    /// produces exactly one `Final` in FIFO order, so this many must be
    /// discarded before the current session's text; otherwise a late reply from
    /// a slow worker would be pasted into the next session.
    stale_finals: AtomicUsize,
}

impl Inference {
    /// `history`: where finished sessions are appended for recovery; `None`
    /// disables it (benchmarks, tests).
    pub fn spawn(models_dir: &Path, history: Option<std::path::PathBuf>) -> Result<Self> {
        let (chunk_tx, chunk_rx) = mpsc::channel::<Msg>();
        let (events_tx, events_rx) = mpsc::channel::<DaemonMsg>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let models_dir = models_dir.to_path_buf();

        thread::spawn(move || {
            let mut models = match load_models(&models_dir) {
                Ok(models) => {
                    let _ = ready_tx.send(Ok(()));
                    Some(models)
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let idle_unload = idle_unload_after();
            let idle_poll = IDLE_POLL.min(idle_unload);
            let mut last_activity = Instant::now();
            let mut in_session = false;

            let mut config = SessionConfig::default();
            let mut parts: Vec<String> = Vec::new();
            let mut audio_secs = 0.0f32;
            let mut asr_ms = 0u64;
            loop {
                let msg = match chunk_rx.recv_timeout(idle_poll) {
                    Ok(msg) => msg,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Never unload mid-session, however long the pause.
                        if models.is_some() && !in_session && last_activity.elapsed() >= idle_unload
                        {
                            models = None;
                            println!("models unloaded after {idle_unload:?} idle");
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };
                last_activity = Instant::now();
                // Reload on any session traffic; Start arrives at hotkey press,
                // so a cold reload overlaps with the user speaking.
                if models.is_none() && !matches!(msg, Msg::Cancel) {
                    match load_models(&models_dir) {
                        Ok(loaded) => models = Some(loaded),
                        Err(e) => {
                            eprintln!("reloading models failed: {e:#}");
                            if let Msg::Flush = msg {
                                // One result per Flush, but as the error it is,
                                // not as silence.
                                let _ = events_tx.send(DaemonMsg::Error(format!(
                                    "reloading models failed: {e:#}"
                                )));
                            }
                            continue;
                        }
                    }
                }
                match msg {
                    Msg::Start(new_config) => {
                        in_session = true;
                        parts.clear();
                        audio_secs = 0.0;
                        asr_ms = 0;
                        config = new_config;
                    }
                    Msg::Chunk(mut samples) => {
                        let asr = &mut models.as_mut().expect("loaded above").asr;
                        pad_short_clip(&mut samples);
                        let secs = samples.len() as f32 / TARGET_RATE as f32;
                        let start = Instant::now();
                        let result = catch_panic("ASR", || {
                            asr.transcribe_with(
                                &samples,
                                &CohereParams {
                                    language: Some(config.language.clone()),
                                    ..Default::default()
                                },
                            )
                            .map_err(|e| anyhow!("{e}"))
                        });
                        match result {
                            Ok(r) => {
                                println!(
                                    "  chunk {:>4.1}s, ASR {:.2?}: {}",
                                    secs,
                                    start.elapsed(),
                                    r.text
                                );
                                audio_secs += secs;
                                asr_ms += start.elapsed().as_millis() as u64;
                                let _ = events_tx.send(DaemonMsg::Partial(r.text.clone()));
                                parts.push(r.text);
                            }
                            Err(e) => eprintln!("ASR error: {e}"),
                        }
                    }
                    Msg::Flush => {
                        let chunks = parts.len();
                        let raw = std::mem::take(&mut parts).join(" ");
                        let mut polish_ms = 0u64;
                        let text = if raw.trim().is_empty() {
                            String::new()
                        } else {
                            let _ = events_tx.send(DaemonMsg::Polishing);
                            let start = Instant::now();
                            let polisher = &models.as_ref().expect("loaded above").polisher;
                            let polished = catch_panic("polish", || {
                                polisher.polish(&raw, &config.control_line)
                            })
                            .unwrap_or_else(|e| {
                                eprintln!("polish error, using raw text: {e}");
                                raw.clone()
                            });
                            polish_ms = start.elapsed().as_millis() as u64;
                            println!("  polish {:.2?}", start.elapsed());
                            polished
                        };
                        // Gate on raw, not polished: an empty polish of real
                        // speech is exactly the lost dictation this recovers.
                        if let Some(history_path) = &history
                            && !raw.trim().is_empty()
                        {
                            let mut entry = crate::history::Entry::now(&raw, &text);
                            entry.chunks = chunks;
                            entry.audio_secs = audio_secs;
                            entry.asr_ms = asr_ms;
                            entry.polish_ms = polish_ms;
                            if let Err(e) = crate::history::append(history_path, &entry) {
                                eprintln!("recording history failed: {e:#}");
                            }
                        }
                        audio_secs = 0.0;
                        asr_ms = 0;
                        in_session = false;
                        let _ = events_tx.send(DaemonMsg::Final(text));
                    }
                    Msg::Cancel => {
                        in_session = false;
                        parts.clear();
                        audio_secs = 0.0;
                        asr_ms = 0;
                        config = SessionConfig::default();
                        let _ = events_tx.send(DaemonMsg::Aborted);
                    }
                }
            }
        });

        ready_rx
            .recv()
            .context("inference thread died during load")??;
        Ok(Self {
            chunk_tx,
            events_rx: Mutex::new(events_rx),
            stale_finals: AtomicUsize::new(0),
        })
    }

    /// Receive the next worker event, for callers that relay `Partial`s as they
    /// arrive. Must not be mixed with `finish` on the same instance.
    pub fn recv_event(&self, timeout: Duration) -> Result<DaemonMsg, mpsc::RecvTimeoutError> {
        self.events_rx
            .lock()
            .expect("event receiver poisoned")
            .recv_timeout(timeout)
    }

    /// Wait for the session flushed by `Session::stop` to finish transcribing
    /// and polishing, skipping intermediate events.
    pub fn finish(&self) -> Result<String> {
        loop {
            let event = match self.recv_event(FINISH_TIMEOUT) {
                Ok(event) => event,
                Err(e) => {
                    self.stale_finals.fetch_add(1, Ordering::Relaxed);
                    return Err(e).context("inference worker did not respond, it may have crashed");
                }
            };
            let DaemonMsg::Final(text) = event else {
                continue;
            };
            if self.stale_finals.load(Ordering::Relaxed) == 0 {
                return Ok(text);
            }
            self.stale_finals.fetch_sub(1, Ordering::Relaxed);
            eprintln!("discarding late result from a timed-out session");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_clips_are_padded_and_others_untouched() {
        let mut short = vec![0.5; MIN_CLIP / 2];
        pad_short_clip(&mut short);
        assert_eq!(short.len(), PADDED_CLIP);
        assert_eq!(short[0], 0.5);
        assert_eq!(*short.last().unwrap(), 0.0);

        let mut long = vec![0.5; MIN_CLIP * 2];
        pad_short_clip(&mut long);
        assert_eq!(long.len(), MIN_CLIP * 2);

        let mut empty: Vec<f32> = Vec::new();
        pad_short_clip(&mut empty);
        assert!(empty.is_empty());
    }
}
