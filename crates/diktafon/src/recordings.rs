//! A bounded, local rolling archive of dictation audio, kept for
//! debuggability: a poor transcription can be heard again, re-transcribed with
//! a different model, or held as tuning data. Each released (never canceled)
//! dictation is saved as a 16 kHz mono WAV captured after resampling and before
//! VAD, so the clip holds exactly what the microphone heard including the
//! silences chunking dropped. Only the newest [`KEEP`] clips are retained.
//!
//! The audio exists only here on the client: the daemon receives silence-cut
//! chunks, never the continuous stream, so the client writes the file and hands
//! the daemon an opaque filename to link the history entry back to it.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// How many of the newest recordings to retain. Each successful save prunes the
/// oldest beyond this, so the archive is a bounded rolling window.
const KEEP: usize = 20;

const PREFIX: &str = "recording-";
const EXTENSION: &str = "wav";

/// The directory holding retained recordings, inside the data dir.
fn dir() -> PathBuf {
    diktafon_protocol::data_dir().join("recordings")
}

/// Mint a unique, filesystem-safe, time-sortable filename for a new recording.
/// Decided on the client at press so the same name can reach the daemon (which
/// links it into the history entry) and the file the client writes at release.
pub fn new_name() -> String {
    let now = chrono::Utc::now();
    format!(
        "{PREFIX}{}.{EXTENSION}",
        now.format("%Y-%m-%dT%H-%M-%S%.3fZ")
    )
}

/// Save `samples` (16 kHz mono f32) as `name`, then prune the oldest recordings
/// beyond [`KEEP`]. Empty audio is skipped: there is nothing to retain, and a
/// header-only file would only confuse the History pane. Returns the path
/// written, or `None` when skipped. A failure here must never block a paste, so
/// callers log and ignore it.
pub fn save(name: &str, samples: &[f32]) -> Result<Option<PathBuf>> {
    save_in(&dir(), name, samples)
}

fn save_in(dir: &Path, name: &str, samples: &[f32]) -> Result<Option<PathBuf>> {
    if samples.is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join(name);
    // Write to a sibling temp then rename, so a crash can never leave a
    // half-written WAV the History pane would try to play.
    let tmp = dir.join(format!("{name}.part"));
    std::fs::write(&tmp, encode_wav(samples))
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &dest).with_context(|| format!("renaming to {}", dest.display()))?;
    prune_in(dir, KEEP)?;
    Ok(Some(dest))
}

/// Drop the oldest recordings in `dir` so at most `keep` remain. Order is by
/// filename, which embeds the mint time, so "oldest" is deterministic and needs
/// no filesystem timestamps.
fn prune_in(dir: &Path, keep: usize) -> Result<()> {
    let mut names = list_in(dir)?;
    if names.len() <= keep {
        return Ok(());
    }
    names.sort();
    let excess = names.len() - keep;
    for name in names.into_iter().take(excess) {
        let path = dir.join(&name);
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!("pruning {} failed: {e:#}", path.display());
        }
    }
    Ok(())
}

/// Filenames of the retained recordings in `dir`. A missing directory is an
/// empty archive, not an error.
fn list_in(dir: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        if let Some(name) = entry.ok().and_then(|entry| recording_name(&entry.path())) {
            names.push(name);
        }
    }
    Ok(names)
}

/// The recording filename if `path` is a retained recording, else `None`. The
/// `.part` temp a save writes is filtered out by the extension check.
fn recording_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_string();
    (path.extension().and_then(|ext| ext.to_str()) == Some(EXTENSION) && name.starts_with(PREFIX))
        .then_some(name)
}

/// The path of a retained recording, or `None` when `name` is not a name this
/// module could have minted. History entries predate validation and the file
/// is hand-editable, so a ref must never become a path unchecked: `../` would
/// otherwise escape the archive.
pub fn path(name: &str) -> Option<PathBuf> {
    join_in(&dir(), name)
}

/// Whether the retained audio for `name` is actually on disk. A history entry
/// can outlive its file: the user may delete it, or remove it externally.
pub fn exists(name: &str) -> bool {
    exists_in(&dir(), name)
}

/// Join a validated recording name onto `dir`. The single validation point
/// for every operation that touches the archive: names that could not have
/// been minted (`../`, absolute paths, wrong prefix or extension) never
/// become paths.
fn join_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.file_name().and_then(|file| file.to_str()) != Some(name) {
        return None;
    }
    recording_name(path).map(|_| dir.join(name))
}

fn exists_in(dir: &Path, name: &str) -> bool {
    join_in(dir, name).is_some_and(|path| path.is_file())
}

/// Delete one recording, leaving its transcript history untouched. `Ok(false)`
/// when there is nothing to delete: an already-missing file is the state the
/// caller wanted, so it is not an error.
pub fn delete(name: &str) -> Result<bool> {
    delete_from(&dir(), name)
}

fn delete_from(dir: &Path, name: &str) -> Result<bool> {
    let Some(path) = join_in(dir, name) else {
        return Ok(false);
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("deleting {}", path.display())),
    }
}

/// Decode a retained WAV back to 16 kHz mono f32 samples for
/// retranscription. Only the shape this module writes is accepted: the `fmt`
/// chunk must declare 16 kHz mono s16, and a `data` chunk that overclaims the
/// file errors instead of panicking.
pub fn samples(path: &Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut pos = 12;
    let mut format_ok = false;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = bytes.get(pos + 8..pos + 8 + size);
        if &bytes[pos..pos + 4] == b"fmt " {
            let body =
                body.with_context(|| format!("truncated fmt chunk in {}", path.display()))?;
            let rate = u32::from_le_bytes(body.get(4..8).unwrap_or(&[0; 4]).try_into().unwrap());
            let channels =
                u16::from_le_bytes(body.get(2..4).unwrap_or(&[0; 2]).try_into().unwrap());
            let bits = u16::from_le_bytes(body.get(14..16).unwrap_or(&[0; 2]).try_into().unwrap());
            format_ok = rate == diktafon_protocol::TARGET_RATE && channels == 1 && bits == 16;
        }
        if &bytes[pos..pos + 4] == b"data" {
            anyhow::ensure!(
                format_ok,
                "unexpected WAV format in {}; expected 16kHz mono s16",
                path.display()
            );
            let body = body.with_context(|| format!("truncated data in {}", path.display()))?;
            return Ok(body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair) as f32 / 32768.0)
                .collect());
        }
        pos += 8 + size + (size & 1);
    }
    anyhow::bail!("no data chunk in {}", path.display())
}
/// exact inverse of the decoder in `bench.rs`, so retained clips interoperate
/// with the bundled eval clips and the file-transcription paths.
fn encode_wav(samples: &[f32]) -> Vec<u8> {
    const CHANNELS: u16 = 1;
    const BITS: u16 = 16;
    let sample_rate = diktafon_protocol::TARGET_RATE;
    let bytes_per_sample = usize::from(BITS / 8);
    let data_len = (samples.len() * bytes_per_sample) as u32;
    let byte_rate = sample_rate * u32::from(CHANNELS) * u32::from(BITS / 8);
    let block_align = CHANNELS * (BITS / 8);

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&BITS.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &sample in samples {
        let quantized = (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&quantized.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch dir that removes itself: without this every test run would
    /// leave its fixtures behind in the temp dir, one set per pid.
    struct TempDir(PathBuf);

    impl std::ops::Deref for TempDir {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(tag: &str) -> TempDir {
        let dir = TempDir(
            std::env::temp_dir().join(format!("dkt-recordings-{tag}-{}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).unwrap();
        dir
    }

    #[test]
    fn samples_roundtrip_through_the_wav_within_quantization() {
        let dir = temp_dir("roundtrip");
        let original: Vec<f32> = (0..1600)
            .map(|i| (i as f32 / 1600.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        let path = save_in(&dir, "recording-a.wav", &original)
            .unwrap()
            .expect("saved");
        let decoded = samples(&path).unwrap();
        assert_eq!(decoded.len(), original.len());
        for (original, restored) in original.iter().zip(&decoded) {
            assert!(
                (original - restored).abs() < 1e-3,
                "{original} vs {restored}"
            );
        }
    }

    #[test]
    fn the_header_declares_16k_mono_s16() {
        let wav = encode_wav(&[0.0; 10]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1, "mono");
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            diktafon_protocol::TARGET_RATE
        );
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16, "bits");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]),
            20,
            "10 samples * 2 bytes"
        );
    }

    #[test]
    fn empty_audio_is_skipped() {
        let dir = temp_dir("empty");
        assert!(save_in(&dir, "recording-x.wav", &[]).unwrap().is_none());
        assert!(list_in(&dir).unwrap().is_empty());
    }

    #[test]
    fn a_save_writes_a_playable_file_and_leaves_no_temp() {
        let dir = temp_dir("save");
        let path = save_in(&dir, "recording-a.wav", &[0.0, 0.5, -0.25])
            .unwrap()
            .expect("non-empty audio is saved");
        assert!(path.exists());
        assert!(!dir.join("recording-a.wav.part").exists());
        assert_eq!(samples(&path).unwrap().len(), 3);
    }

    #[test]
    fn pruning_keeps_only_the_newest_keep() {
        let dir = temp_dir("prune");
        // Filenames embed a time-sortable stamp, so lexical order is age order.
        for second in 0..(KEEP + 5) {
            let name = format!("recording-2026-01-01T00-00-{second:02}.000Z.wav");
            save_in(&dir, &name, &[0.1; 4]).unwrap();
        }
        let remaining = list_in(&dir).unwrap();
        assert_eq!(remaining.len(), KEEP);
        // The five oldest seconds (00..04) are gone; 05 and the newest survive.
        assert!(!remaining.iter().any(|n| n.contains("T00-00-00.")));
        assert!(!remaining.iter().any(|n| n.contains("T00-00-04.")));
        assert!(remaining.iter().any(|n| n.contains("T00-00-05.")));
        assert!(
            remaining
                .iter()
                .any(|n| n.contains(&format!("T00-00-{:02}.", KEEP + 4)))
        );
    }

    #[test]
    fn corrupt_clips_error_instead_of_panicking() {
        let dir = temp_dir("decode");
        let saved = save_in(&dir, "recording-a.wav", &[0.0, 0.5, -0.25])
            .unwrap()
            .expect("saved");
        let decoded = samples(&saved).unwrap();
        assert_eq!(decoded.len(), 3);
        assert!((decoded[1] - 0.5).abs() < 1e-3);
        assert!(samples(&dir.join("absent.wav")).is_err());
        std::fs::write(dir.join("junk.wav"), b"not a wav").unwrap();
        assert!(samples(&dir.join("junk.wav")).is_err());
        // A data chunk overclaiming the file must error, not slice-panic.
        let mut overclaimed = std::fs::read(&saved).unwrap();
        let huge = (u32::MAX / 2).to_le_bytes();
        overclaimed[40..44].copy_from_slice(&huge);
        std::fs::write(dir.join("over.wav"), &overclaimed).unwrap();
        assert!(samples(&dir.join("over.wav")).is_err());
        // A clip at the wrong rate is refused rather than fed to ASR as 16k.
        let mut wrong_rate = std::fs::read(&saved).unwrap();
        wrong_rate[24..28].copy_from_slice(&8000u32.to_le_bytes());
        std::fs::write(dir.join("rate.wav"), &wrong_rate).unwrap();
        assert!(samples(&dir.join("rate.wav")).is_err());
    }

    #[test]
    fn a_minted_name_is_safe_and_well_formed() {
        let name = new_name();
        assert!(name.starts_with(PREFIX), "{name}");
        assert!(name.ends_with(&format!(".{EXTENSION}")), "{name}");
        assert!(!name.contains(':'), "colons are unsafe: {name}");
    }

    #[test]
    fn part_files_are_not_counted_as_recordings() {
        let dir = temp_dir("part");
        std::fs::write(dir.join("recording-a.wav"), encode_wav(&[0.0])).unwrap();
        std::fs::write(dir.join("recording-a.wav.part"), b"half").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(list_in(&dir).unwrap(), vec!["recording-a.wav".to_string()]);
    }

    #[test]
    fn names_outside_the_archive_are_rejected() {
        assert!(path("recording-a.wav").is_some());
        // A history entry is hand-editable, so traversal, absolute paths, and
        // wrong names must never become paths.
        assert_eq!(path("../evil.wav"), None);
        assert_eq!(path("/tmp/recording-a.wav"), None);
        assert_eq!(path("recording-a.wav.part"), None);
        assert_eq!(path("notes.txt"), None);
        assert_eq!(path("other.wav"), None);
        assert_eq!(path(""), None);
    }

    #[test]
    fn deleting_is_idempotent_and_missing_is_not_an_error() {
        let dir = temp_dir("delete");
        save_in(&dir, "recording-a.wav", &[0.1; 4]).unwrap();
        assert!(exists_in(&dir, "recording-a.wav"));
        assert!(delete_from(&dir, "recording-a.wav").unwrap());
        assert!(!exists_in(&dir, "recording-a.wav"));
        // Twice deleted, never existed, or invalid: all the wanted state.
        assert!(!delete_from(&dir, "recording-a.wav").unwrap());
        assert!(!delete_from(&dir, "recording-never.wav").unwrap());
        assert!(!delete_from(&dir, "../evil.wav").unwrap());
    }
}
