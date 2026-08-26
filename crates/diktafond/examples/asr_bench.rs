//! ASR-only benchmark over the eval clips, reporting per-clip inference time
//! and real-time factor. Pass a transcription catalog ID to select a model.

use std::time::Instant;

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
    let model_id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| diktafond::manifest::DEFAULT_TRANSCRIPTION_MODEL.into());
    let models_dir = diktafon_protocol::models_dir();
    let load_start = Instant::now();
    let model_path = diktafond::manifest::model_path(&models_dir, &model_id)
        .expect("catalog transcription model");
    let model = transcribe_cpp::Model::load_with(
        &model_path,
        &transcribe_cpp::ModelOptions {
            backend: transcribe_cpp::Backend::Auto,
            device: None,
        },
    )
    .expect("loading");
    println!(
        "{model_id} loaded through {} on {} in {:.2?}",
        model.backend(),
        model.device().map(|device| device.name).unwrap_or_default(),
        load_start.elapsed()
    );
    let mut session = model.session().expect("creating session");

    let eval = diktafon_protocol::data_dir().join("eval-own");
    let mut total_audio = 0.0f32;
    let mut total_infer = 0.0f32;
    for clip in ["01.wav", "02.wav", "03.wav", "04.wav", "05.wav"] {
        let samples = wav_samples(&eval.join(clip));
        let secs = samples.len() as f32 / 16_000.0;
        let start = Instant::now();
        let result = session
            .run(
                &samples,
                &transcribe_cpp::RunOptions {
                    language: Some("en".into()),
                    task: transcribe_cpp::Task::Transcribe,
                    pnc: transcribe_cpp::Pnc::Default,
                    ..Default::default()
                },
            )
            .expect("transcribing");
        let infer = start.elapsed().as_secs_f32();
        total_audio += secs;
        total_infer += infer;
        println!(
            "{clip}: {secs:.1}s audio, {infer:.2}s infer, {:.1}x RT\n{}",
            secs / infer,
            result.text
        );
    }
    println!(
        "total: {total_audio:.1}s audio in {total_infer:.2}s, {:.1}x RT",
        total_audio / total_infer
    );
}
