use anyhow::{anyhow, Context, Result};
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use transcribe_rs::onnx::cohere::{CohereModel, CohereParams};
use transcribe_rs::onnx::Quantization;

use diktafon_protocol::{Msg, TARGET_RATE};

use crate::llm::Polisher;

/// Upper bound on transcribing the queued chunks plus one polish pass. Hit when
/// the worker thread died or is far behind; `finish` errors instead of blocking
/// forever, and a late reply from a slow worker is discarded, not misdelivered.
const FINISH_TIMEOUT: Duration = Duration::from_secs(60);

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
/// triggering one polish pass over the joined parts. This chunks-in/text-out
/// boundary is the seam where a remote server backend can replace the local
/// worker later.
pub struct Inference {
    pub chunk_tx: mpsc::Sender<Msg>,
    final_rx: mpsc::Receiver<String>,
    /// Replies still owed by sessions whose `finish` timed out. Each `Flush`
    /// produces exactly one reply in FIFO order, so this many must be discarded
    /// before the current session's text; otherwise a late reply from a slow
    /// worker would be pasted into the next session.
    stale_results: Cell<usize>,
}

impl Inference {
    pub fn spawn(models_dir: &Path) -> Result<Self> {
        let (chunk_tx, chunk_rx) = mpsc::channel::<Msg>();
        let (final_tx, final_rx) = mpsc::channel::<String>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let models_dir = models_dir.to_path_buf();

        thread::spawn(move || {
            let loaded = (|| -> Result<(CohereModel, Polisher)> {
                let asr = CohereModel::load(&models_dir.join("cohere-int8"), &Quantization::Int8)
                    .context("loading ASR model")?;
                let polisher =
                    Polisher::load(&models_dir.join("s1-mini-q4_k_m.gguf")).context("loading LLM")?;
                Ok((asr, polisher))
            })();
            let (mut asr, polisher) = match loaded {
                Ok(models) => {
                    let _ = ready_tx.send(Ok(()));
                    models
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            let mut parts: Vec<String> = Vec::new();
            for msg in chunk_rx {
                match msg {
                    Msg::Chunk(samples) => {
                        let secs = samples.len() as f32 / TARGET_RATE as f32;
                        let start = Instant::now();
                        let result = catch_panic("ASR", || {
                            asr.transcribe_with(
                                &samples,
                                &CohereParams {
                                    language: Some("en".to_string()),
                                    ..Default::default()
                                },
                            )
                            .map_err(|e| anyhow!("{e}"))
                        });
                        match result {
                            Ok(r) => {
                                println!("  chunk {:>4.1}s, ASR {:.2?}: {}", secs, start.elapsed(), r.text);
                                parts.push(r.text);
                            }
                            Err(e) => eprintln!("ASR error: {e}"),
                        }
                    }
                    Msg::Flush => {
                        let raw = std::mem::take(&mut parts).join(" ");
                        let text = if raw.trim().is_empty() {
                            String::new()
                        } else {
                            let start = Instant::now();
                            let polished = catch_panic("polish", || polisher.polish(&raw))
                                .unwrap_or_else(|e| {
                                    eprintln!("polish error, using raw text: {e}");
                                    raw
                                });
                            println!("  polish {:.2?}", start.elapsed());
                            polished
                        };
                        let _ = final_tx.send(text);
                    }
                }
            }
        });

        ready_rx.recv().context("inference thread died during load")??;
        Ok(Self { chunk_tx, final_rx, stale_results: Cell::new(0) })
    }

    /// Wait for the session flushed by `Session::stop` to finish transcribing
    /// and polishing.
    pub fn finish(&self) -> Result<String> {
        loop {
            let text = match self.final_rx.recv_timeout(FINISH_TIMEOUT) {
                Ok(text) => text,
                Err(e) => {
                    self.stale_results.set(self.stale_results.get() + 1);
                    return Err(e).context("inference worker did not respond, it may have crashed");
                }
            };
            if self.stale_results.get() == 0 {
                return Ok(text);
            }
            self.stale_results.set(self.stale_results.get() - 1);
            eprintln!("discarding late result from a timed-out session");
        }
    }
}
