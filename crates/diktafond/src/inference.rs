use anyhow::{Context, Result, anyhow};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use diktafon_protocol::{DaemonMsg, ModelSelection, Msg, SessionConfig, TARGET_RATE};

use crate::llm::Polisher;
use crate::manifest::{ModelBackend, model, model_path};

/// Observer for model residency changes; the daemon points it at the status
/// file, benchmarks and tests pass `None`.
pub type Residency = Option<Box<dyn Fn(bool, Option<&str>, Option<&str>) + Send>>;

/// How often the worker wakes to check for idleness.
const IDLE_POLL: Duration = Duration::from_secs(30);

/// The resident models cost gigabytes of RAM; after this long without any
/// message the daemon exits and is respawned on demand (`DIKTAFOND_IDLE_SECS`
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
    asr: Box<dyn Transcriber>,
    asr_languages: Vec<String>,
    asr_backend: String,
    asr_device: String,
    polisher: PolishingBackend,
}

enum PolishingBackend {
    S1(Polisher),
    AppleIntelligence,
}

impl PolishingBackend {
    fn load(models_dir: &Path, selection: &ModelSelection) -> Result<Self> {
        match model(&selection.polishing)?.backend {
            ModelBackend::LlamaCpp => {
                let path = model_path(models_dir, &selection.polishing)?;
                Ok(Self::S1(Polisher::load(&path).context("loading S1-mini")?))
            }
            ModelBackend::AppleFoundationModels => Ok(Self::AppleIntelligence),
            backend => Err(anyhow!("unsupported polishing backend {backend:?}")),
        }
    }

    fn polish(&self, transcript: &str, control_line: &str) -> Result<String> {
        match self {
            Self::S1(polisher) => polisher.polish(transcript, control_line),
            Self::AppleIntelligence => crate::apple_intelligence::polish(transcript, control_line),
        }
    }
}

trait Transcriber: Send {
    fn transcribe(&mut self, samples: &[f32], language: &str) -> Result<String>;
}

struct TranscribeCppTranscriber(transcribe_cpp::Session);

impl Transcriber for TranscribeCppTranscriber {
    fn transcribe(&mut self, samples: &[f32], language: &str) -> Result<String> {
        let options = transcribe_cpp::RunOptions {
            task: transcribe_cpp::Task::Transcribe,
            pnc: transcribe_cpp::Pnc::Default,
            language: Some(language.to_string()),
            ..Default::default()
        };
        self.0
            .run(samples, &options)
            .map(|transcript| transcript.text)
            .map_err(|error| anyhow!("{error}"))
    }
}

fn load_models(models_dir: &Path, selection: &ModelSelection) -> Result<Models> {
    let start = Instant::now();
    let asr_path = model_path(models_dir, &selection.transcription)?;
    let asr_model = model(&selection.transcription)?;
    let (asr, asr_backend, asr_device): (Box<dyn Transcriber>, _, _) = match asr_model.backend {
        ModelBackend::TranscribeCpp => {
            let options = transcribe_cpp::ModelOptions {
                backend: transcribe_cpp::Backend::Auto,
                device: None,
            };
            let model = transcribe_cpp::Model::load_with(&asr_path, &options)
                .context("loading transcribe.cpp model")?;
            let backend = model.backend();
            let device = model
                .device()
                .map(|device| device.name)
                .unwrap_or_else(|_| "unknown device".into());
            println!(
                "ASR {} loaded through {} on {}",
                model.arch(),
                backend,
                device
            );
            (
                Box::new(TranscribeCppTranscriber(
                    model.session().context("creating transcribe.cpp session")?,
                )),
                backend,
                device,
            )
        }
        backend => return Err(anyhow!("unsupported transcription backend {backend:?}")),
    };
    let asr_loaded = start.elapsed();
    let polisher = PolishingBackend::load(models_dir, selection)?;
    // Split reported because only the ASR gates the start of transcription;
    // the polish model is not needed until the session is flushed.
    println!(
        "models loaded in {:.2?} (asr {:.2?}, llm {:.2?})",
        start.elapsed(),
        asr_loaded,
        start.elapsed() - asr_loaded
    );
    Ok(Models {
        asr,
        asr_languages: asr_model.languages.clone(),
        asr_backend,
        asr_device,
        polisher,
    })
}

/// Upper bound on transcribing the queued chunks plus one polish pass. Hit when
/// the worker thread died or is far behind; `finish` errors instead of blocking
/// forever, and a late reply from a slow worker is discarded, not misdelivered.
const FINISH_TIMEOUT: Duration = Duration::from_secs(60);

/// Very short clips make ASR models invent text; Handy's guard zero-pads
/// anything under [`MIN_CLIP`] out to [`PADDED_CLIP`] before transcription.
const MIN_CLIP: usize = TARGET_RATE as usize;
const PADDED_CLIP: usize = TARGET_RATE as usize * 5 / 4;

/// Padding stops the model erroring on a stub clip but not inventing words
/// for it: sessions holding under a second of real speech transcribed to
/// "Thank you." seven times in this user's history, plus "Bum bum bum." and
/// "Come with them.", each pasted into whatever had focus. A tap of the
/// hotkey, or a cough, is not a dictation, so the whole session is dropped
/// rather than answered with invented text. Genuine one-word dictations
/// ("Okay.", "Commit and push.") measured 1.45s and up.
const MIN_SESSION_SAMPLES: usize = TARGET_RATE as usize;

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
    /// disables it (benchmarks, tests). `residency` is told whenever the
    /// models are loaded (true) or unloaded (false).
    pub fn spawn(
        models_dir: &Path,
        selection: ModelSelection,
        history: Option<std::path::PathBuf>,
        residency: Residency,
    ) -> Result<Self> {
        let (chunk_tx, chunk_rx) = mpsc::channel::<Msg>();
        let (events_tx, events_rx) = mpsc::channel::<DaemonMsg>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let models_dir = models_dir.to_path_buf();

        thread::spawn(move || {
            let notify_residency = |loaded: bool, models: Option<&Models>| {
                if let Some(callback) = &residency {
                    callback(
                        loaded,
                        models.map(|models| models.asr_backend.as_str()),
                        models.map(|models| models.asr_device.as_str()),
                    );
                }
            };
            let mut models = match load_models(&models_dir, &selection) {
                Ok(models) => {
                    let _ = ready_tx.send(Ok(()));
                    notify_residency(true, Some(&models));
                    models
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
            let mut session_error: Option<String> = None;
            // Real speech in the session, before any padding.
            let mut speech_samples = 0usize;
            loop {
                let msg = match chunk_rx.recv_timeout(idle_poll) {
                    Ok(msg) => msg,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Never exit mid-session, however long the pause.
                        if !in_session && last_activity.elapsed() >= idle_unload {
                            // Exiting IS the unload: dropping the models in
                            // place leaves ~950MB cached by macOS's malloc,
                            // and the allocator knobs that release it cost 2x
                            // ASR latency. The client respawns the daemon on
                            // the next session, which had to load models
                            // anyway; SIGTERM takes the same cleanup path as
                            // a user quit.
                            println!("idle for {idle_unload:?}; exiting to free model memory");
                            let _ = signal_hook::low_level::raise(signal_hook::consts::SIGTERM);
                            return;
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };
                last_activity = Instant::now();
                match msg {
                    Msg::Start(new_config) => {
                        in_session = true;
                        parts.clear();
                        audio_secs = 0.0;
                        asr_ms = 0;
                        session_error = None;
                        speech_samples = 0;
                        config = new_config;
                    }
                    Msg::Chunk(mut samples) => {
                        speech_samples += samples.len();
                        pad_short_clip(&mut samples);
                        let secs = samples.len() as f32 / TARGET_RATE as f32;
                        let start = Instant::now();
                        let result = if models
                            .asr_languages
                            .iter()
                            .any(|code| code == &config.language)
                        {
                            let asr = &mut models.asr;
                            catch_panic("ASR", || asr.transcribe(&samples, &config.language))
                        } else {
                            Err(anyhow!(
                                "transcription model does not support language {:?}",
                                config.language
                            ))
                        };
                        match result {
                            Ok(r) => {
                                println!(
                                    "  chunk {:>4.1}s, ASR {:.2?}: {}",
                                    secs,
                                    start.elapsed(),
                                    r
                                );
                                audio_secs += secs;
                                asr_ms += start.elapsed().as_millis() as u64;
                                let _ = events_tx.send(DaemonMsg::Partial(r.clone()));
                                parts.push(r);
                            }
                            Err(e) => {
                                eprintln!("ASR error: {e}");
                                session_error.get_or_insert_with(|| e.to_string());
                            }
                        }
                    }
                    Msg::Flush => {
                        if let Some(error) = session_error.take() {
                            parts.clear();
                            audio_secs = 0.0;
                            asr_ms = 0;
                            speech_samples = 0;
                            in_session = false;
                            let _ = events_tx.send(DaemonMsg::Error(error));
                            continue;
                        }
                        let chunks = parts.len();
                        let too_short = speech_samples < MIN_SESSION_SAMPLES;
                        let raw = std::mem::take(&mut parts).join(" ");
                        if too_short {
                            println!(
                                "  {:.2}s of speech is below the {:.2}s minimum; dropping{}",
                                speech_samples as f32 / TARGET_RATE as f32,
                                MIN_SESSION_SAMPLES as f32 / TARGET_RATE as f32,
                                if raw.trim().is_empty() {
                                    String::new()
                                } else {
                                    format!(" invented {raw:?}")
                                }
                            );
                            audio_secs = 0.0;
                            asr_ms = 0;
                            in_session = false;
                            let _ = events_tx.send(DaemonMsg::Final(String::new()));
                            continue;
                        }
                        let mut polish_ms = 0u64;
                        let text = if raw.trim().is_empty() {
                            String::new()
                        } else {
                            let _ = events_tx.send(DaemonMsg::Polishing);
                            let start = Instant::now();
                            let polisher = &models.polisher;
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
                            let mut entry = diktafon_protocol::HistoryEntry::now(&raw, &text);
                            entry.chunks = chunks;
                            entry.audio_secs = audio_secs;
                            entry.asr_ms = asr_ms;
                            entry.polish_ms = polish_ms;
                            entry.transcription_model = Some(selection.transcription.clone());
                            entry.polishing_model = Some(selection.polishing.clone());
                            if let Err(e) =
                                diktafon_protocol::history::append_to(history_path, &entry)
                            {
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
                        session_error = None;
                        audio_secs = 0.0;
                        asr_ms = 0;
                        speech_samples = 0;
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

    struct FakeTranscriber;

    impl Transcriber for FakeTranscriber {
        fn transcribe(&mut self, samples: &[f32], language: &str) -> Result<String> {
            Ok(format!("{language}:{}", samples.len()))
        }
    }

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

    #[test]
    fn transcription_backend_receives_pcm_and_language() {
        let mut backend: Box<dyn Transcriber> = Box::new(FakeTranscriber);
        assert_eq!(backend.transcribe(&[0.0; 160], "en").unwrap(), "en:160");
    }

    #[test]
    #[ignore = "loads the real Canary and S1 models"]
    fn canary_model_pair_loads() {
        let selection = ModelSelection {
            transcription: "canary-1b-flash-q5-k-m".into(),
            ..ModelSelection::default()
        };
        Inference::spawn(&diktafon_protocol::models_dir(), selection, None, None).unwrap();
    }
}
