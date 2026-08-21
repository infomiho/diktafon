use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use transcribe_rs::vad::{SileroVad, SmoothedVad, Vad};

use diktafon_protocol::{Msg, TARGET_RATE};

/// Also the level-meter frame interval, so ~30 FPS.
const MONITOR_TICK: Duration = Duration::from_millis(33);

// Handy's tuned Silero setup: 30ms frames, onset after 2 speech frames, 450ms
// of pre-onset audio kept, 450ms hangover before a speech segment is declared
// over.
const SPEECH_THRESHOLD: f32 = 0.3;
const ONSET_FRAMES: usize = 2;
const PREFILL_FRAMES: usize = 15;
const HANGOVER_FRAMES: usize = 15;

/// Segments shorter than this are held and merged with the next speech segment
/// instead of paying a per-chunk ASR roundtrip.
const MIN_CHUNK_SAMPLES: usize = TARGET_RATE as usize * 3 / 2;

pub const LEVEL_BAR_COUNT: usize = 16;
/// Live level-meter bars in 0..=1, written by the recording monitor at each
/// tick and read by the pill's render loop.
pub type LevelBars = Arc<Mutex<[f32; LEVEL_BAR_COUNT]>>;

pub struct Recorder {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    channels: usize,
    rate: u32,
    vad_model: PathBuf,
    levels: LevelBars,
}

pub struct Session {
    stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    monitor: JoinHandle<()>,
}

impl Recorder {
    pub fn new(vad_model: PathBuf, levels: LevelBars) -> Result<Self> {
        let device = cpal::default_host()
            .default_input_device()
            .context("no input device")?;
        let config = device.default_input_config()?;
        let channels = config.channels() as usize;
        let rate = config.sample_rate().0;
        Ok(Self { device, config, channels, rate, vad_model, levels })
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
        // Fresh VAD per session: Silero keeps LSTM state across frames, and
        // loading the 1.8MB model is fast enough to not delay recording.
        let silero = SileroVad::new(&self.vad_model, SPEECH_THRESHOLD)
            .with_context(|| format!("loading VAD model {}", self.vad_model.display()))?;
        let mut chunker = VadChunker::new(Box::new(silero));
        let mut resampler = StreamResampler::new(self.rate, TARGET_RATE);

        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let stream = self.build_stream(buffer.clone())?;
        stream.play()?;

        let stop = Arc::new(AtomicBool::new(false));
        *self.levels.lock().unwrap() = [0.0; LEVEL_BAR_COUNT];
        let monitor = thread::spawn({
            let buffer = buffer.clone();
            let stop = stop.clone();
            let levels = self.levels.clone();
            move || {
                let mut meter = LevelMeter::new();
                let mut frame_tail: Vec<f32> = Vec::new();
                loop {
                    thread::sleep(MONITOR_TICK);
                    let done = stop.load(Ordering::Relaxed);
                    let fresh = resampler.drain(&buffer.lock().unwrap());
                    meter.push(&fresh);
                    *levels.lock().unwrap() = meter.compute();
                    frame_tail.extend(fresh);
                    let frame_size = chunker.frame_size();
                    let mut frames = frame_tail.chunks_exact(frame_size);
                    for frame in &mut frames {
                        if let Some(chunk) = chunker.push_frame(frame) {
                            let _ = chunk_tx.send(Msg::Chunk(chunk));
                        }
                    }
                    frame_tail = frames.remainder().to_vec();
                    if done {
                        if let Some(chunk) = chunker.finish(&frame_tail) {
                            let _ = chunk_tx.send(Msg::Chunk(chunk));
                        }
                        let _ = chunk_tx.send(Msg::Flush);
                        *levels.lock().unwrap() = [0.0; LEVEL_BAR_COUNT];
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

/// Cuts the sample stream into speech chunks using a smoothed VAD: a chunk is
/// pre-onset prefill + speech + hangover, emitted when the VAD leaves speech.
/// Silence between speech segments never reaches the ASR model, which
/// hallucinates phrases like "Thank you." on it.
struct VadChunker {
    vad: SmoothedVad,
    pending: Vec<f32>,
    /// Frames processed since the VAD last left speech; saturated when it never
    /// was in speech. Bounds how much of the next onset's prefill is new audio.
    frames_since_speech: usize,
}

impl VadChunker {
    fn new(inner: Box<dyn Vad>) -> Self {
        Self {
            vad: SmoothedVad::new(inner, PREFILL_FRAMES, HANGOVER_FRAMES, ONSET_FRAMES),
            pending: Vec::new(),
            frames_since_speech: usize::MAX,
        }
    }

    fn frame_size(&self) -> usize {
        self.vad.frame_size()
    }

    /// Feed one frame; returns a finished chunk when a long-enough speech
    /// segment just ended. Short segments are held to merge with the next one.
    fn push_frame(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        let was_in_speech = self.vad.in_speech();
        let in_speech = self.vad.is_speech(frame).unwrap_or_else(|e| {
            eprintln!("VAD error: {e}");
            was_in_speech
        });
        if in_speech {
            if !was_in_speech {
                let prefill = self.vad.drain_prefill();
                // The prefill ring also filled during the previous segment's
                // hangover, so its oldest frames may already have been emitted;
                // re-adding them would duplicate audio.
                let overlap_frames =
                    (PREFILL_FRAMES - 1).saturating_sub(self.frames_since_speech);
                let skip = (overlap_frames * self.frame_size()).min(prefill.len());
                self.pending.extend_from_slice(&prefill[skip..]);
            }
            self.pending.extend_from_slice(frame);
            return None;
        }
        if was_in_speech {
            self.frames_since_speech = 0;
            if self.pending.len() >= MIN_CHUNK_SAMPLES {
                return Some(std::mem::take(&mut self.pending));
            }
        } else {
            self.frames_since_speech = self.frames_since_speech.saturating_add(1);
        }
        None
    }

    /// End of session: emit whatever speech is pending, including a partial
    /// frame the stream ended on.
    fn finish(&mut self, frame_tail: &[f32]) -> Option<Vec<f32>> {
        let mut pending = std::mem::take(&mut self.pending);
        if self.vad.in_speech() {
            pending.extend_from_slice(frame_tail);
        }
        (!pending.is_empty()).then_some(pending)
    }
}

/// Handy's mic-calibrated level meter: an FFT over the most recent ~32ms of
/// 16kHz audio, 16 log-spaced buckets across 400-4000Hz, each mapped from a
/// -68..-30dB range with gain and a softening power curve.
struct LevelMeter {
    fft: Arc<dyn rustfft::Fft<f32>>,
    hann: Vec<f32>,
    recent: std::collections::VecDeque<f32>,
    /// Meter ballistics: instant attack, gradual decay, so bars fall smoothly
    /// instead of strobing between ticks.
    smoothed: [f32; LEVEL_BAR_COUNT],
}

const FFT_WINDOW: usize = 512;
const LEVEL_MIN_DB: f32 = -68.0;
const LEVEL_MAX_DB: f32 = -30.0;
const LEVEL_GAIN: f32 = 1.3;
const LEVEL_POWER: f32 = 0.7;
const LEVEL_FREQ_MIN: f32 = 400.0;
const LEVEL_FREQ_MAX: f32 = 4000.0;
/// Per-tick decay factor of a falling bar.
const LEVEL_DECAY: f32 = 0.78;

impl LevelMeter {
    fn new() -> Self {
        let fft = rustfft::FftPlanner::new().plan_fft_forward(FFT_WINDOW);
        let hann = (0..FFT_WINDOW)
            .map(|i| (std::f32::consts::PI * i as f32 / FFT_WINDOW as f32).sin().powi(2))
            .collect();
        Self {
            fft,
            hann,
            recent: std::collections::VecDeque::with_capacity(FFT_WINDOW),
            smoothed: [0.0; LEVEL_BAR_COUNT],
        }
    }

    fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            if self.recent.len() == FFT_WINDOW {
                self.recent.pop_front();
            }
            self.recent.push_back(sample);
        }
    }

    fn compute(&mut self) -> [f32; LEVEL_BAR_COUNT] {
        let mut bars = [0.0; LEVEL_BAR_COUNT];
        if self.recent.len() < FFT_WINDOW {
            // No spectrum yet; keep the bars falling instead of freezing.
            for smoothed in &mut self.smoothed {
                *smoothed *= LEVEL_DECAY;
                if *smoothed < 0.01 {
                    *smoothed = 0.0;
                }
            }
            return self.smoothed;
        }
        let mut spectrum: Vec<rustfft::num_complex::Complex<f32>> = self
            .recent
            .iter()
            .zip(&self.hann)
            .map(|(sample, window)| rustfft::num_complex::Complex::new(sample * window, 0.0))
            .collect();
        self.fft.process(&mut spectrum);

        let bin_hz = TARGET_RATE as f32 / FFT_WINDOW as f32;
        let span_ratio = LEVEL_FREQ_MAX / LEVEL_FREQ_MIN;
        for (i, bar) in bars.iter_mut().enumerate() {
            let lo = LEVEL_FREQ_MIN * span_ratio.powf(i as f32 / LEVEL_BAR_COUNT as f32);
            let hi = LEVEL_FREQ_MIN * span_ratio.powf((i + 1) as f32 / LEVEL_BAR_COUNT as f32);
            let lo_bin = (lo / bin_hz) as usize;
            let hi_bin = ((hi / bin_hz) as usize).clamp(lo_bin + 1, FFT_WINDOW / 2);
            let mean = spectrum[lo_bin..hi_bin].iter().map(|c| c.norm()).sum::<f32>()
                / (hi_bin - lo_bin) as f32
                / (FFT_WINDOW as f32 / 2.0);
            let db = 20.0 * mean.max(1e-9).log10();
            let norm = ((db - LEVEL_MIN_DB) / (LEVEL_MAX_DB - LEVEL_MIN_DB)).clamp(0.0, 1.0);
            *bar = (norm * LEVEL_GAIN).min(1.0).powf(LEVEL_POWER);
        }
        for (smoothed, raw) in self.smoothed.iter_mut().zip(bars) {
            *smoothed = if raw > *smoothed { raw } else { *smoothed * LEVEL_DECAY };
            if *smoothed < 0.01 {
                *smoothed = 0.0;
            }
        }
        self.smoothed
    }
}

/// Incremental linear resampler over a growing source buffer; output position
/// is tracked globally so successive `drain` calls stay continuous.
struct StreamResampler {
    ratio: f64,
    produced: usize,
}

impl StreamResampler {
    fn new(from: u32, to: u32) -> Self {
        Self { ratio: from as f64 / to as f64, produced: 0 }
    }

    /// Resample everything available in `src` that has not been produced yet.
    fn drain(&mut self, src: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        loop {
            let pos = self.produced as f64 * self.ratio;
            let idx = pos as usize;
            if idx + 1 >= src.len() {
                return out;
            }
            let frac = (pos - idx as f64) as f32;
            out.push(src[idx] + (src[idx + 1] - src[idx]) * frac);
            self.produced += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transcribe_rs::vad::EnergyVad;

    /// Chunker driven by the RMS-based EnergyVad, so the smoothing and cut
    /// logic is testable without the Silero model file.
    fn test_chunker() -> VadChunker {
        VadChunker::new(Box::new(EnergyVad::new(480, 0.01)))
    }

    fn noise(secs: f32) -> Vec<f32> {
        (0..(secs * TARGET_RATE as f32) as usize)
            .map(|i| if i % 2 == 0 { 0.1 } else { -0.1 })
            .collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * TARGET_RATE as f32) as usize]
    }

    fn feed(chunker: &mut VadChunker, samples: &[f32]) -> Vec<Vec<f32>> {
        samples
            .chunks_exact(chunker.frame_size())
            .filter_map(|frame| chunker.push_frame(frame))
            .collect()
    }

    #[test]
    fn cuts_after_speech_ends() {
        let mut chunker = test_chunker();
        let mut samples = noise(2.0);
        samples.extend(silence(1.0));
        samples.extend(noise(2.0));
        let chunks = feed(&mut chunker, &samples);
        assert_eq!(chunks.len(), 1);
        let secs = chunks[0].len() as f32 / TARGET_RATE as f32;
        // Speech plus up to prefill (well under one second here) and hangover.
        assert!((2.0..=2.7).contains(&secs), "chunk of {secs}s");
        // The trailing speech has no silence after it yet; finish flushes it.
        assert!(chunker.finish(&[]).is_some());
    }

    #[test]
    fn short_segment_is_held_and_merged() {
        let mut chunker = test_chunker();
        let mut samples = noise(0.5);
        samples.extend(silence(1.0));
        samples.extend(noise(1.5));
        samples.extend(silence(1.0));
        let chunks = feed(&mut chunker, &samples);
        assert_eq!(chunks.len(), 1);
        let secs = chunks[0].len() as f32 / TARGET_RATE as f32;
        assert!(secs >= 2.0, "merged chunk of {secs}s");
        assert!(chunker.finish(&[]).is_none());
    }

    /// Speech resuming shortly after a segment ended must not re-emit the
    /// previous segment's hangover audio through the new onset's prefill.
    #[test]
    fn quick_resume_does_not_duplicate_hangover_audio() {
        let frame = 480;
        let mut chunker = test_chunker();
        let mut samples = noise(80.0 * frame as f32 / TARGET_RATE as f32);
        samples.extend(vec![0.0; 17 * frame]);
        samples.extend(noise(80.0 * frame as f32 / TARGET_RATE as f32));
        samples.extend(vec![0.0; 40 * frame]);
        let chunks = feed(&mut chunker, &samples);
        assert_eq!(chunks.len(), 2);
        // Second chunk: 80 speech frames + 15 hangover + the few silence
        // frames of non-overlapping prefill. With the overlap re-emitted it
        // would be ~109 frames.
        let frames = chunks[1].len() / frame;
        assert!((95..=100).contains(&frames), "second chunk has {frames} frames");
    }

    #[test]
    fn silence_yields_nothing() {
        let mut chunker = test_chunker();
        assert!(feed(&mut chunker, &silence(3.0)).is_empty());
        assert!(chunker.finish(&[]).is_none());
    }

    #[test]
    fn level_meter_tracks_a_tone_and_silence() {
        let mut meter = LevelMeter::new();
        let tone: Vec<f32> = (0..FFT_WINDOW)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / TARGET_RATE as f32).sin() * 0.5)
            .collect();
        meter.push(&tone);
        let bars = meter.compute();
        let loudest =
            bars.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap();
        // 1 kHz lands in log bucket 6 of 400..4000 Hz across 16 buckets.
        assert!((5..=7).contains(&loudest), "loudest bucket {loudest}: {bars:?}");
        assert!(bars[loudest] > 0.5, "{bars:?}");

        let mut silent = LevelMeter::new();
        silent.push(&vec![0.0; FFT_WINDOW]);
        assert_eq!(silent.compute(), [0.0; LEVEL_BAR_COUNT]);
    }

    #[test]
    fn resampler_is_continuous_across_drains() {
        let src: Vec<f32> = (0..96_000).map(|i| i as f32).collect();
        let mut all_at_once = StreamResampler::new(96_000, TARGET_RATE);
        let expected = all_at_once.drain(&src);

        let mut incremental = StreamResampler::new(96_000, TARGET_RATE);
        let mut out = incremental.drain(&src[..10_000]);
        out.extend(incremental.drain(&src[..50_000]));
        out.extend(incremental.drain(&src));

        assert_eq!(out, expected);
        let expected_len = src.len() / 6;
        assert!(expected.len().abs_diff(expected_len) <= 1, "{} samples", expected.len());
    }
}

#[cfg(test)]
mod silero_tests {
    use super::*;

    /// Needs the real model and eval clips in Application Support; run with
    /// `cargo test -p diktafon -- --ignored`.
    #[test]
    #[ignore = "loads the real Silero model"]
    fn detects_speech_segments_in_eval_clip() {
        let silero = SileroVad::new(
            diktafon_protocol::models_dir().join("silero_vad_v4.onnx"),
            SPEECH_THRESHOLD,
        )
        .unwrap();
        let mut chunker = VadChunker::new(Box::new(silero));

        let wav = std::fs::read(diktafon_protocol::data_dir().join("eval-own/01.wav")).unwrap();
        let samples: Vec<f32> = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
            .collect();
        let clip_secs = samples.len() as f32 / TARGET_RATE as f32;

        let mut chunks: Vec<Vec<f32>> = samples
            .chunks_exact(chunker.frame_size())
            .filter_map(|frame| chunker.push_frame(frame))
            .collect();
        chunks.extend(chunker.finish(&[]));

        assert!(!chunks.is_empty(), "no speech detected in a 21s spoken clip");
        let speech_secs: f32 =
            chunks.iter().map(|c| c.len() as f32).sum::<f32>() / TARGET_RATE as f32;
        assert!(
            speech_secs > clip_secs * 0.5 && speech_secs <= clip_secs,
            "{speech_secs}s of speech in a {clip_secs}s clip"
        );
    }
}
