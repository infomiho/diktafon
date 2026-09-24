//! ASR-only benchmark over a directory of eval clips, reporting per-clip
//! inference time, real-time factor and, when the directory has a
//! `manifest.json` of reference transcripts, word error rate.
//!
//! Usage: `asr_bench [MODEL_ID] [LANGUAGE] [CLIP_DIR]`. Defaults to the
//! default transcription model, `en`, and `eval-own` in the data dir.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;

#[derive(Deserialize)]
struct ReferenceClip {
    file: String,
    text: String,
}

const COMBINING_MARKS: std::ops::RangeInclusive<char> = '\u{0300}'..='\u{036F}';

fn wav_samples(path: &Path) -> Vec<f32> {
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

fn is_wav(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
}

fn wav_clips(dir: &Path) -> Vec<PathBuf> {
    let mut clips: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("reading clip directory")
        .map(|entry| entry.expect("reading clip directory entry").path())
        .filter(|path| is_wav(path))
        .collect();
    clips.sort();
    clips
}

fn load_references(dir: &Path) -> Option<HashMap<String, String>> {
    let json = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let clips: Vec<ReferenceClip> = serde_json::from_str(&json).expect("parsing manifest.json");
    for clip in &clips {
        let has_combining_mark = clip.text.chars().any(|c| COMBINING_MARKS.contains(&c));
        assert!(
            !has_combining_mark,
            "reference for {} has combining marks, convert manifest.json to NFC",
            clip.file
        );
    }
    let references = clips.into_iter().map(|clip| (clip.file, clip.text));
    Some(references.collect())
}

fn normalized_words(text: &str) -> Vec<String> {
    let spaced: String = text
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    spaced.split_whitespace().map(String::from).collect()
}

fn word_edits(reference: &[String], hypothesis: &[String]) -> usize {
    let mut row: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, reference_word) in reference.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, hypothesis_word) in hypothesis.iter().enumerate() {
            let substitution = diagonal + usize::from(reference_word != hypothesis_word);
            let deletion = row[j + 1] + 1;
            let insertion = row[j] + 1;
            diagonal = row[j + 1];
            row[j + 1] = substitution.min(deletion).min(insertion);
        }
    }
    row[hypothesis.len()]
}

fn percent(edits: usize, words: usize) -> f32 {
    100.0 * edits as f32 / words as f32
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model_id = args
        .next()
        .unwrap_or_else(|| diktafond::manifest::DEFAULT_TRANSCRIPTION_MODEL.into());
    let language = args.next().unwrap_or_else(|| "en".into());
    let clip_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| diktafon_protocol::data_dir().join("eval-own"));

    let clips = wav_clips(&clip_dir);
    assert!(!clips.is_empty(), "no .wav clips in {}", clip_dir.display());
    let references = load_references(&clip_dir).unwrap_or_default();

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
        "{model_id} ({language}) loaded through {} on {} in {:.2?}",
        model.backend(),
        model.device().map(|device| device.name).unwrap_or_default(),
        load_start.elapsed()
    );
    let mut session = model.session().expect("creating session");

    let run_options = transcribe_cpp::RunOptions {
        language: Some(language),
        task: transcribe_cpp::Task::Transcribe,
        pnc: transcribe_cpp::Pnc::Default,
        ..Default::default()
    };
    let mut total_audio = 0.0f32;
    let mut total_infer = 0.0f32;
    let mut total_edits = 0usize;
    let mut total_reference_words = 0usize;
    for clip in &clips {
        let name = clip.file_name().unwrap().to_string_lossy();
        let samples = wav_samples(clip);
        let secs = samples.len() as f32 / 16_000.0;
        let start = Instant::now();
        let result = session.run(&samples, &run_options).expect("transcribing");
        let infer = start.elapsed().as_secs_f32();
        total_audio += secs;
        total_infer += infer;
        let realtime = secs / infer;
        println!(
            "{name}: {secs:.1}s audio, {infer:.2}s infer, {realtime:.1}x RT\n{}",
            result.text
        );
        if let Some(reference) = references.get(name.as_ref()) {
            let reference_words = normalized_words(reference);
            let hypothesis_words = normalized_words(&result.text);
            let edits = word_edits(&reference_words, &hypothesis_words);
            let word_count = reference_words.len();
            let wer = percent(edits, word_count);
            println!("  WER {wer:.1}% ({edits}/{word_count})");
            total_edits += edits;
            total_reference_words += word_count;
        }
    }
    let total_realtime = total_audio / total_infer;
    println!("total: {total_audio:.1}s audio in {total_infer:.2}s, {total_realtime:.1}x RT");
    if total_reference_words > 0 {
        let total_wer = percent(total_edits, total_reference_words);
        println!("WER: {total_wer:.1}% ({total_edits}/{total_reference_words})");
    }
}
