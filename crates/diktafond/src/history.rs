//! Append-only dictation history, one JSON line per session, so a lost
//! dictation (paste into the wrong window, client crash) can be recovered
//! from `history.jsonl` in the data dir.

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::Write;
use std::path::Path;

#[derive(Serialize)]
pub struct Entry<'a> {
    /// RFC3339 UTC.
    pub at: String,
    pub raw: &'a str,
    pub polished: &'a str,
    pub chunks: usize,
    pub audio_secs: f32,
    pub asr_ms: u64,
    pub polish_ms: u64,
}

impl<'a> Entry<'a> {
    pub fn now(raw: &'a str, polished: &'a str) -> Self {
        Self {
            at: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
            raw,
            polished,
            chunks: 0,
            audio_secs: 0.0,
            asr_ms: 0,
            polish_ms: 0,
        }
    }
}

pub fn append(path: &Path, entry: &Entry) -> Result<()> {
    // One write_all per entry: a crash can then only lose a whole line, never
    // merge two entries into one unparseable one.
    let mut line = serde_json::to_vec(entry).context("serializing history entry")?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(&line).context("writing history entry")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_roundtrip_with_awkward_text() {
        let path = std::env::temp_dir().join(format!("dkt-history-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut entry = Entry::now("he said \"stop\"\nnew line", "He said \"stop\".");
        entry.chunks = 2;
        entry.audio_secs = 3.5;
        append(&path, &entry).unwrap();
        append(&path, &Entry::now("second", "Second.")).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["raw"], "he said \"stop\"\nnew line");
        assert_eq!(parsed["chunks"], 2);
        assert!(parsed["at"].as_str().unwrap().ends_with('Z'));

        std::fs::remove_file(&path).unwrap();
    }
}
