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
    #[serde(default)]
    pub transcription_model: Option<String>,
    #[serde(default)]
    pub polishing_model: Option<String>,
    /// Filename of the retained WAV under `recordings/`, when one was kept for
    /// this dictation. Absent for entries written before retention existed and
    /// for sessions whose audio was not retained, so old lines still parse.
    #[serde(default)]
    pub recording: Option<String>,
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
            transcription_model: None,
            polishing_model: None,
            recording: None,
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

/// Remove the newest entry matching `target` by timestamp and both texts,
/// returning whether one was dropped. Entries carry no id, but a timestamp
/// plus both transcripts is unique in practice; newest wins because History
/// lists newest first. Transcripts are otherwise append-only; this exists
/// for History entry deletion, which removes the dictation outright.
pub fn remove_matching(path: &Path, target: &HistoryEntry) -> Result<bool> {
    rewrite_matching(path, target, None)
}

/// Replace the newest entry matching `target` with `replacement`,
/// returning whether one was replaced. The replacement keeps its position:
/// callers adopting a rerun pass the entry back with new transcripts (and
/// models), preserving timestamp, metrics, and the recording link.
pub fn replace_matching(
    path: &Path,
    target: &HistoryEntry,
    replacement: HistoryEntry,
) -> Result<bool> {
    rewrite_matching(path, target, Some(replacement))
}

/// Rewrite the newest entry matching `target` (by timestamp and both
/// texts) to `replacement`, or drop it when `replacement` is `None`.
/// Returns whether a line changed. The rewrite is atomic (temp file plus
/// rename) and preserves lines that do not parse: only a positively matched
/// entry is ever touched.
fn rewrite_matching(
    path: &Path,
    target: &HistoryEntry,
    replacement: Option<HistoryEntry>,
) -> Result<bool> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut lines: Vec<String> = raw.lines().map(str::to_owned).collect();
    let found = lines.iter().rposition(|line| {
        serde_json::from_str::<HistoryEntry>(line).is_ok_and(|entry| {
            entry.at == target.at && entry.raw == target.raw && entry.polished == target.polished
        })
    });
    let Some(ix) = found else {
        return Ok(false);
    };
    match replacement {
        Some(replacement) => {
            lines[ix] =
                serde_json::to_string(&replacement).context("serializing replacement entry")?;
        }
        None => {
            lines.remove(ix);
        }
    }
    let mut tmp = path.to_path_buf();
    tmp.set_extension("tmp");
    let mut contents = lines.join("\n");
    if !lines.is_empty() {
        contents.push('\n');
    }
    std::fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(true)
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
        entry.transcription_model = Some("transcriber".into());
        entry.polishing_model = Some("polisher".into());
        append_to(&path, &entry).unwrap();
        append_to(&path, &HistoryEntry::now("second", "Second.")).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: HistoryEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.raw, "he said \"stop\"\nnew line");
        assert_eq!(parsed.chunks, 2);
        assert_eq!(parsed.transcription_model.as_deref(), Some("transcriber"));
        assert_eq!(parsed.polishing_model.as_deref(), Some("polisher"));
        assert!(parsed.at.ends_with('Z'));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn old_entries_have_no_model_provenance() {
        let entry: HistoryEntry = serde_json::from_str(
            r#"{"at":"2026-08-26T00:00:00Z","raw":"raw","polished":"polished"}"#,
        )
        .unwrap();
        assert_eq!(entry.transcription_model, None);
        assert_eq!(entry.polishing_model, None);
    }

    #[test]
    fn a_recording_reference_roundtrips_and_defaults_absent() {
        let mut entry = HistoryEntry::now("raw", "Raw.");
        assert_eq!(entry.recording, None, "no recording until one is retained");
        entry.recording = Some("recording-2026-01-01T00-00-00.000Z.wav".into());
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: HistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.recording.as_deref(),
            Some("recording-2026-01-01T00-00-00.000Z.wav")
        );

        // A line written before retention existed carries no recording.
        let old: HistoryEntry =
            serde_json::from_str(r#"{"at":"2026-08-26T00:00:00Z","raw":"r","polished":"p"}"#)
                .unwrap();
        assert_eq!(old.recording, None);
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

    fn remove_fixture(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dkt-history-remove-{tag}-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn dated(at: &str, raw: &str, polished: &str) -> HistoryEntry {
        let mut entry = HistoryEntry::now(raw, polished);
        entry.at = at.into();
        entry
    }

    #[test]
    fn removing_drops_only_the_newest_match() {
        let path = remove_fixture("drop");
        let old = dated("2026-09-01T00:00:00Z", "same", "Same.");
        let new = dated("2026-09-02T00:00:00Z", "same", "Same.");
        let other = dated("2026-09-03T00:00:00Z", "other", "Other.");
        for entry in [&old, &new, &other] {
            append_to(&path, entry).unwrap();
        }
        assert!(remove_matching(&path, &new).unwrap());
        let rest = read_all_from(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].at, old.at, "the older twin survives");
        assert_eq!(rest[1].at, other.at);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn replacing_swaps_texts_in_place() {
        let path = remove_fixture("replace");
        let first = dated("2026-09-01T00:00:00Z", "raw one", "One.");
        let mut second = dated("2026-09-02T00:00:00Z", "raw two", "Two.");
        second.recording = Some("recording-a.wav".into());
        second.audio_secs = 4.5;
        for entry in [&first, &second] {
            append_to(&path, entry).unwrap();
        }
        let mut adopted = second.clone();
        adopted.raw = "raw two fixed".into();
        adopted.polished = "Two, fixed.".into();
        adopted.transcription_model = Some("new-asr".into());
        assert!(replace_matching(&path, &second, adopted).unwrap());
        let rest = read_all_from(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(rest.len(), 2);
        // Position kept; timestamp, metrics, and recording link untouched.
        assert_eq!(rest[1].at, second.at);
        assert_eq!(rest[1].raw, "raw two fixed");
        assert_eq!(rest[1].polished, "Two, fixed.");
        assert_eq!(rest[1].transcription_model.as_deref(), Some("new-asr"));
        assert_eq!(rest[1].recording.as_deref(), Some("recording-a.wav"));
        assert_eq!(rest[1].audio_secs, 4.5);
        assert_eq!(rest[0].polished, "One.");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn replacing_without_a_match_changes_nothing() {
        let path = remove_fixture("replace-miss");
        let kept = dated("2026-09-01T00:00:00Z", "kept", "Kept.");
        append_to(&path, &kept).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let miss = dated("2026-09-02T00:00:00Z", "absent", "Absent.");
        let replacement = dated("2026-09-02T00:00:00Z", "new", "New.");
        assert!(!replace_matching(&path, &miss, replacement).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn removing_without_a_match_leaves_the_file_alone() {
        let path = remove_fixture("miss");
        let kept = dated("2026-09-01T00:00:00Z", "kept", "Kept.");
        append_to(&path, &kept).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let miss = dated("2026-09-02T00:00:00Z", "absent", "Absent.");
        assert!(!remove_matching(&path, &miss).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        // A missing file is nothing to delete, not an error.
        let absent =
            std::env::temp_dir().join(format!("dkt-history-absent-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&absent);
        assert!(!remove_matching(&absent, &miss).unwrap());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn removing_preserves_torn_lines() {
        let path = remove_fixture("torn");
        let kept = dated("2026-09-01T00:00:00Z", "kept", "Kept.");
        let gone = dated("2026-09-02T00:00:00Z", "gone", "Gone.");
        append_to(&path, &kept).unwrap();
        append_to(&path, &gone).unwrap();
        std::fs::write(
            &path,
            format!("{{ truncated\n{}", std::fs::read_to_string(&path).unwrap()),
        )
        .unwrap();
        assert!(remove_matching(&path, &gone).unwrap());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("{ truncated"),
            "unparseable lines survive"
        );
        assert_eq!(read_all_from(&contents).len(), 1);
        std::fs::remove_file(&path).unwrap();
    }
}
