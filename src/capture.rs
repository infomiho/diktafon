use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::inference::Msg;

pub const TARGET_RATE: u32 = 16_000;

const FRAME_MS: usize = 30;
const SILENCE_MS: usize = 500;
const MIN_CHUNK_MS: usize = 1500;
const RMS_THRESHOLD: f32 = 0.01;
const MONITOR_TICK: Duration = Duration::from_millis(100);

pub struct Recorder {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    channels: usize,
    rate: u32,
}

pub struct Session {
    stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    monitor: JoinHandle<()>,
}

impl Recorder {
    pub fn new() -> Result<Self> {
        let device = cpal::default_host()
            .default_input_device()
            .context("no input device")?;
        let config = device.default_input_config()?;
        let channels = config.channels() as usize;
        let rate = config.sample_rate().0;
        Ok(Self { device, config, channels, rate })
    }

    pub fn describe(&self) -> String {
        format!(
            "{} ({} Hz, {} ch)",
            self.device.name().unwrap_or_else(|_| "unknown".into()),
            self.rate,
            self.channels
        )
    }

    pub fn start(&self, chunk_tx: mpsc::Sender<Msg>) -> Result<Session> {
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let stream = self.build_stream(buffer.clone())?;
        stream.play()?;

        let stop = Arc::new(AtomicBool::new(false));
        let monitor = thread::spawn({
            let buffer = buffer.clone();
            let stop = stop.clone();
            let rate = self.rate;
            move || {
                let mut start = 0usize;
                loop {
                    thread::sleep(MONITOR_TICK);
                    let done = stop.load(Ordering::Relaxed);
                    let chunk = {
                        let buf = buffer.lock().unwrap();
                        if done {
                            buf[start..].to_vec()
                        } else if let Some(cut) = find_cut(&buf[start..], rate) {
                            let chunk = buf[start..start + cut].to_vec();
                            start += cut;
                            chunk
                        } else {
                            continue;
                        }
                    };
                    if has_speech(&chunk, rate) {
                        let _ = chunk_tx.send(Msg::Chunk(resample_linear(&chunk, rate, TARGET_RATE)));
                    }
                    if done {
                        let _ = chunk_tx.send(Msg::Flush);
                        break;
                    }
                }
            }
        });

        Ok(Session { stream, stop, monitor })
    }

    fn build_stream(&self, buffer: Arc<Mutex<Vec<f32>>>) -> Result<cpal::Stream> {
        let err_fn = |e| eprintln!("stream error: {e}");
        let stream_config: cpal::StreamConfig = self.config.clone().into();
        let channels = self.channels;
        let stream = match self.config.sample_format() {
            cpal::SampleFormat::F32 => self.device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| push_mono(&buffer, data, channels),
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => self.device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    let floats: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                    push_mono(&buffer, &floats, channels);
                },
                err_fn,
                None,
            )?,
            format => anyhow::bail!("unsupported sample format {format:?}"),
        };
        Ok(stream)
    }
}

impl Session {
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        self.monitor.join().ok();
        drop(self.stream);
    }
}

fn push_mono(buffer: &Arc<Mutex<Vec<f32>>>, data: &[f32], channels: usize) {
    let mut buf = buffer.lock().unwrap();
    buf.extend(
        data.chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32),
    );
}

/// Find a cut point: the chunk must contain speech, span at least
/// MIN_CHUNK_MS, and end in at least SILENCE_MS of silence.
fn find_cut(samples: &[f32], rate: u32) -> Option<usize> {
    let frame = rate as usize * FRAME_MS / 1000;
    if frame == 0 {
        return None;
    }
    let need_silent = SILENCE_MS / FRAME_MS;
    let min_frames = MIN_CHUNK_MS / FRAME_MS;
    let mut seen_speech = false;
    let mut silent_run = 0;
    for (i, f) in samples.chunks_exact(frame).enumerate() {
        let rms = (f.iter().map(|s| s * s).sum::<f32>() / frame as f32).sqrt();
        if rms < RMS_THRESHOLD {
            silent_run += 1;
        } else {
            seen_speech = true;
            silent_run = 0;
        }
        if seen_speech && silent_run >= need_silent && i + 1 >= min_frames {
            return Some((i + 1) * frame);
        }
    }
    None
}

fn has_speech(samples: &[f32], rate: u32) -> bool {
    let frame = rate as usize * FRAME_MS / 1000;
    samples
        .chunks_exact(frame.max(1))
        .any(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt() >= RMS_THRESHOLD)
}

fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx.min(input.len() - 1)];
            let b = input[(idx + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(secs: f32, rate: u32) -> Vec<f32> {
        (0..(secs * rate as f32) as usize)
            .map(|i| if i % 2 == 0 { 0.1 } else { -0.1 })
            .collect()
    }

    #[test]
    fn cuts_at_silence_after_speech() {
        let rate = 48_000;
        let mut samples = noise(2.0, rate);
        samples.extend(vec![0.0f32; rate as usize]);
        samples.extend(noise(2.0, rate));
        let cut = find_cut(&samples, rate).expect("should cut");
        let cut_secs = cut as f32 / rate as f32;
        assert!((2.4..=3.1).contains(&cut_secs), "cut at {cut_secs}s");
        assert!(find_cut(&samples[cut..], rate).is_none());
        assert!(has_speech(&samples[cut..], rate));
    }

    #[test]
    fn no_cut_without_speech() {
        let rate = 48_000;
        assert!(find_cut(&vec![0.0f32; rate as usize * 3], rate).is_none());
    }

    #[test]
    fn no_cut_mid_speech() {
        let rate = 48_000;
        assert!(find_cut(&noise(3.0, rate), rate).is_none());
    }
}
