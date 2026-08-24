//! `history.jsonl`: every finished dictation, one JSON object per line. The
//! daemon appends, the client's History pane and `--stats` read, so the format
//! lives here rather than being agreed across two crates. Plaintext of
//! everything ever dictated, so treat the file as sensitive.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One finished dictation. The metrics default so lines written by older
/// daemons still parse.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct HistoryEntry {
    /// RFC3339 UTC, as produced by [`now_rfc3339`].
    pub at: String,
    pub raw: String,
    pub polished: String,
    #[serde(default)]
    pub chunks: usize,
    #[serde(default)]
    pub audio_secs: f32,
    #[serde(default)]
    pub asr_ms: u64,
    #[serde(default)]
    pub polish_ms: u64,
}

impl HistoryEntry {
    pub fn now(raw: &str, polished: &str) -> Self {
        Self {
            at: now_rfc3339(),
            raw: raw.to_string(),
            polished: polished.to_string(),
            chunks: 0,
            audio_secs: 0.0,
            asr_ms: 0,
            polish_ms: 0,
        }
    }
}

/// The one producer of timestamps for everything diktafon records, so a
/// reader only ever has one format to parse.
pub fn now_rfc3339() -> String {
    humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string()
}

pub fn path() -> PathBuf {
    crate::data_dir().join("history.jsonl")
}

pub fn append(entry: &HistoryEntry) -> Result<()> {
    append_to(&path(), entry)
}

pub fn append_to(path: &Path, entry: &HistoryEntry) -> Result<()> {
    // One write_all per entry: a crash can then only lose a whole line, never
    // merge two entries into one unparseable one.
    let mut line = serde_json::to_vec(entry).context("serializing history entry")?;
    line.push(b'\n');
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.write_all(&line).context("writing history entry")?;
    Ok(())
}

/// Every entry, oldest first. Unparseable lines are skipped rather than
/// failing the whole read.
pub fn read_all() -> Vec<HistoryEntry> {
    parse(&contents(), usize::MAX)
}

/// The freshest `limit` entries, newest first. Only the tail is parsed: the
/// file grows without bound, and a pane showing twenty entries should not pay
/// for every dictation ever recorded.
pub fn recent(limit: usize) -> Vec<HistoryEntry> {
    let mut entries = parse(&contents(), limit);
    entries.reverse();
    entries
}

fn contents() -> String {
    std::fs::read_to_string(path()).unwrap_or_default()
}

fn parse(contents: &str, tail: usize) -> Vec<HistoryEntry> {
    let lines: Vec<&str> = contents.lines().collect();
    lines[lines.len().saturating_sub(tail)..]
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_roundtrip_with_awkward_text() {
        let path = std::env::temp_dir().join(format!("dkt-history-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut entry = HistoryEntry::now("he said \"stop\"\nnew line", "He said \"stop\".");
        entry.chunks = 2;
        entry.audio_secs = 3.5;
        append_to(&path, &entry).unwrap();
        append_to(&path, &HistoryEntry::now("second", "Second.")).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: HistoryEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.raw, "he said \"stop\"\nnew line");
        assert_eq!(parsed.chunks, 2);
        assert!(parsed.at.ends_with('Z'));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn only_the_tail_is_parsed_and_it_comes_back_newest_first() {
        let lines: String = (0..5)
            .map(|i| {
                let entry = HistoryEntry::now(&format!("raw {i}"), &format!("polished {i}"));
                format!("{}\n", serde_json::to_string(&entry).unwrap())
            })
            .collect();

        let all = parse(&lines, usize::MAX);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].polished, "polished 0");

        let tail = parse(&lines, 2);
        assert_eq!(tail.len(), 2, "asked for the last two");
        assert_eq!(tail[0].polished, "polished 3");
        assert_eq!(tail[1].polished, "polished 4");

        // Asking for more than exists is not an error.
        assert_eq!(parse(&lines, 50).len(), 5);
        assert!(parse("", 10).is_empty());
    }

    #[test]
    fn a_torn_line_does_not_lose_the_rest() {
        let good = serde_json::to_string(&HistoryEntry::now("a", "A")).unwrap();
        let contents = format!("{good}\n{{ truncated\n{good}\n");
        assert_eq!(parse(&contents, usize::MAX).len(), 2);
    }
}
