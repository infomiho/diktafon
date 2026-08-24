//! Client-side dictation timings: the two waits the user actually feels,
//! appended as one JSON line per completed session to `timings.jsonl` in the
//! data dir, and the `--stats` report over them plus the daemon's history.
//! The daemon owns `history.jsonl`; keeping the client's numbers in a
//! separate file preserves one writer per file.

use anyhow::{Context, Result};
use diktafon_protocol::HistoryEntry;
use std::io::Write;
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Timing {
    /// RFC3339 UTC.
    pub at: String,
    /// Hotkey press to the mic actually delivering samples.
    pub mic_ready_ms: u64,
    /// Mic live to hotkey release.
    pub recording_secs: f32,
    /// Hotkey release to the text landing (or the error surfacing).
    pub stop_to_paste_ms: u64,
    /// The daemon was auto-spawned during this session, so the wait includes
    /// model loading.
    pub cold_start: bool,
    /// "pasted", "empty", or "error".
    pub outcome: String,
}

pub fn timings_path() -> PathBuf {
    diktafon_protocol::data_dir().join("timings.jsonl")
}

pub fn append(timing: &Timing) {
    if let Err(e) = try_append(timing) {
        eprintln!("recording timings failed: {e:#}");
    }
}

fn try_append(timing: &Timing) -> Result<()> {
    let mut line = serde_json::to_vec(timing).context("serializing timing")?;
    line.push(b'\n');
    // One write_all per entry, like the daemon's history: a crash loses at
    // most a whole line.
    let path = timings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(&line).context("writing timing")?;
    Ok(())
}

/// `diktafon --stats`: percentiles of the perceived waits from `timings.jsonl`
/// and of the pipeline stages from the daemon's `history.jsonl`.
pub fn report() -> Result<()> {
    let timings = read_timings();
    let history = read_history();
    if timings.is_empty() && history.is_empty() {
        println!("No data yet. Dictate a few times, then run --stats again.");
        return Ok(());
    }

    if !timings.is_empty() {
        let cold = timings.iter().filter(|t| t.cold_start).count();
        println!(
            "Sessions: {} ({} cold starts, {:.0}%)",
            timings.len(),
            cold,
            100.0 * cold as f64 / timings.len() as f64
        );
        let warm_mic: Vec<f64> = timings
            .iter()
            .filter(|t| !t.cold_start)
            .map(|t| t.mic_ready_ms as f64)
            .collect();
        print_line("Press to mic live (warm)", &warm_mic, "ms");
        let cold_mic: Vec<f64> = timings
            .iter()
            .filter(|t| t.cold_start)
            .map(|t| t.mic_ready_ms as f64)
            .collect();
        print_line("Press to mic live (cold)", &cold_mic, "ms");
        let stop: Vec<f64> = timings
            .iter()
            .filter(|t| t.outcome == "pasted")
            .map(|t| t.stop_to_paste_ms as f64)
            .collect();
        print_line("Stop to paste", &stop, "ms");
    }

    if !history.is_empty() {
        println!("Pipeline ({} recorded dictations):", history.len());
        let rtf: Vec<f64> = history
            .iter()
            .filter(|e| e.audio_secs > 0.5 && e.asr_ms > 0)
            .map(|e| e.asr_ms as f64 / 1000.0 / e.audio_secs as f64)
            .collect();
        print_line("ASR realtime factor", &rtf, "x");
        let polish: Vec<f64> = history
            .iter()
            .filter(|e| e.polish_ms > 0)
            .map(|e| e.polish_ms as f64)
            .collect();
        print_line("Polish", &polish, "ms");
        let per_word: Vec<f64> = history
            .iter()
            .filter(|e| e.polish_ms > 0 && !e.polished.trim().is_empty())
            .map(|e| e.polish_ms as f64 / e.polished.split_whitespace().count() as f64)
            .collect();
        print_line("Polish per word", &per_word, "ms");
    }
    Ok(())
}

fn read_timings() -> Vec<Timing> {
    let Ok(content) = std::fs::read_to_string(timings_path()) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn read_history() -> Vec<HistoryEntry> {
    let Ok(content) = std::fs::read_to_string(diktafon_protocol::history_path()) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn print_line(label: &str, values: &[f64], unit: &str) {
    if values.is_empty() {
        return;
    }
    println!(
        "  {label:<26} p50 {:>7}  p95 {:>7}  (n={})",
        format_value(percentile(values, 0.5), unit),
        format_value(percentile(values, 0.95), unit),
        values.len()
    );
}

fn format_value(value: f64, unit: &str) -> String {
    match unit {
        "ms" if value >= 1000.0 => format!("{:.2}s", value / 1000.0),
        "ms" => format!("{value:.0}ms"),
        _ => format!("{value:.2}{unit}"),
    }
}

fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_pick_expected_ranks() {
        let values: Vec<f64> = (0..=100).map(f64::from).collect();
        assert_eq!(percentile(&values, 0.5), 50.0);
        assert_eq!(percentile(&values, 0.95), 95.0);
        assert_eq!(percentile(&[42.0], 0.95), 42.0);
    }

    #[test]
    fn timings_roundtrip() {
        let timing = Timing {
            at: "2026-08-24T10:00:00Z".into(),
            mic_ready_ms: 180,
            recording_secs: 4.2,
            stop_to_paste_ms: 1900,
            cold_start: false,
            outcome: "pasted".into(),
        };
        let line = serde_json::to_string(&timing).unwrap();
        let parsed: Timing = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed.stop_to_paste_ms, 1900);
        assert!(!parsed.cold_start);
    }
}
