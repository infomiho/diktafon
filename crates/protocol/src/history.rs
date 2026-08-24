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
    read_all_from(&contents())
}

/// The freshest `limit` entries that `keep` accepts, newest first. Counting
/// kept entries rather than lines is what makes the count a promise: a run of
/// empty or unreadable entries at the end of the file shortens the answer
/// otherwise. Only as much as needed is deserialized.
pub fn recent_matching(limit: usize, keep: impl Fn(&HistoryEntry) -> bool) -> Vec<HistoryEntry> {
    recent_from(&contents(), limit, keep)
}

fn contents() -> String {
    std::fs::read_to_string(path()).unwrap_or_default()
}

fn read_all_from(contents: &str) -> Vec<HistoryEntry> {
    contents
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn recent_from(
    contents: &str,
    limit: usize,
    keep: impl Fn(&HistoryEntry) -> bool,
) -> Vec<HistoryEntry> {
    let mut newest_first = Vec::new();
    for line in contents.lines().rev() {
        if newest_first.len() == limit {
            break;
        }
        if let Ok(entry) = serde_json::from_str::<HistoryEntry>(line)
            && keep(&entry)
        {
            newest_first.push(entry);
        }
    }
    newest_first
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

    /// `polished` per line, oldest first.
    fn file_of(polished: &[&str]) -> String {
        polished
            .iter()
            .map(|text| {
                let entry = HistoryEntry::now("raw", text);
                format!("{}\n", serde_json::to_string(&entry).unwrap())
            })
            .collect()
    }

    #[test]
    fn everything_reads_back_oldest_first() {
        let file = file_of(&["one", "two", "three"]);
        let all = read_all_from(&file);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].polished, "one");
        assert_eq!(all[2].polished, "three");
        assert!(read_all_from("").is_empty());
    }

    #[test]
    fn the_recent_ones_come_back_newest_first() {
        let file = file_of(&["one", "two", "three"]);
        let recent = recent_from(&file, 2, |_| true);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].polished, "three", "newest leads");
        assert_eq!(recent[1].polished, "two");

        // Asking for more than exists is not an error.
        assert_eq!(recent_from(&file, 50, |_| true).len(), 3);
        assert!(recent_from("", 10, |_| true).is_empty());
    }

    #[test]
    fn the_limit_counts_kept_entries_not_lines() {
        // A run of rejected entries at the end must not shorten the answer.
        let file = file_of(&["keep me", "keep me too", "", "", "", ""]);
        let kept = recent_from(&file, 2, |entry| !entry.polished.is_empty());
        assert_eq!(kept.len(), 2, "kept reading past the rejected tail");
        assert_eq!(kept[0].polished, "keep me too");
        assert_eq!(kept[1].polished, "keep me");
    }

    #[test]
    fn a_torn_line_does_not_lose_the_rest() {
        let good = serde_json::to_string(&HistoryEntry::now("a", "A")).unwrap();
        let contents = format!("{good}\n{{ truncated\n{good}\n");
        assert_eq!(read_all_from(&contents).len(), 2);
        assert_eq!(recent_from(&contents, 5, |_| true).len(), 2);
    }
}
