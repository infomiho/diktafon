use anyhow::{Context, Result};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;
use transcribe_rs::onnx::cohere::{CohereModel, CohereParams};
use transcribe_rs::onnx::Quantization;

use crate::capture::TARGET_RATE;
use crate::llm::Polisher;

pub enum Msg {
    Chunk(Vec<f32>),
    Flush,
}

/// Worker thread owning the resident ASR and polish models. Chunks stream in
/// during recording and are transcribed as they arrive; `Flush` ends a session,
/// triggering one polish pass over the joined parts. This chunks-in/text-out
/// boundary is the seam where a remote server backend can replace the local
/// worker later.
pub struct Inference {
    pub chunk_tx: mpsc::Sender<Msg>,
    final_rx: mpsc::Receiver<String>,
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
                        match asr.transcribe_with(
                            &samples,
                            &CohereParams {
                                language: Some("en".to_string()),
                                ..Default::default()
                            },
                        ) {
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
                            let polished = polisher.polish(&raw).unwrap_or_else(|e| {
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
        Ok(Self { chunk_tx, final_rx })
    }

    /// Wait for the session flushed by `Session::stop` to finish transcribing
    /// and polishing.
    pub fn finish(&self) -> Result<String> {
        Ok(self.final_rx.recv()?)
    }
}
