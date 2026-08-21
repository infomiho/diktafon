//! ASR-only benchmark over the eval clips, reporting per-clip inference time
//! and real-time factor. Run plain for the CPU int8 baseline, or with
//! `--features coreml -- coreml` for the CoreML execution provider.

use std::time::Instant;
use transcribe_rs::onnx::Quantization;
use transcribe_rs::onnx::cohere::{CohereModel, CohereParams};

fn wav_samples(path: &std::path::Path) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("reading wav");
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return bytes[pos + 8..pos + 8 + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
                .collect();
        }
        pos += 8 + size + (size & 1);
    }
    panic!("no data chunk");
}

fn main() {
    if std::env::args().any(|a| a == "coreml") {
        #[cfg(feature = "coreml")]
        transcribe_rs::accel::set_ort_accelerator(transcribe_rs::accel::OrtAccelerator::CoreMl);
        #[cfg(not(feature = "coreml"))]
        panic!("rebuild with --features coreml");
    }

    let models_dir = diktafon_protocol::models_dir();
    let load_start = Instant::now();
    let mut model =
        CohereModel::load(&models_dir.join("cohere-int8"), &Quantization::Int8).expect("loading");
    println!("model loaded in {:.2?}", load_start.elapsed());

    let eval = diktafon_protocol::data_dir().join("eval-own");
    let mut total_audio = 0.0f32;
    let mut total_infer = 0.0f32;
    for clip in ["01.wav", "02.wav", "03.wav", "04.wav", "05.wav"] {
        let samples = wav_samples(&eval.join(clip));
        let secs = samples.len() as f32 / 16_000.0;
        let start = Instant::now();
        let result = model
            .transcribe_with(
                &samples,
                &CohereParams {
                    language: Some("en".into()),
                    ..Default::default()
                },
            )
            .expect("transcribing");
        let infer = start.elapsed().as_secs_f32();
        total_audio += secs;
        total_infer += infer;
        println!(
            "{clip}: {secs:.1}s audio, {infer:.2}s infer, {:.1}x RT | {}",
            secs / infer,
            &result.text[..result.text.len().min(60)]
        );
    }
    println!(
        "total: {total_audio:.1}s audio in {total_infer:.2}s, {:.1}x RT",
        total_audio / total_infer
    );
}
