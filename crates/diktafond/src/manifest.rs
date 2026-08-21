//! What the daemon needs on disk and where to get it. Every URL is pinned to
//! an immutable HuggingFace revision (`resolve/<commit-sha>`), so the bytes
//! always match the recorded size and sha256; the hash is the trust anchor
//! that makes resumed downloads safe.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::fetch::{RemoteFile, fetch};

struct ModelFile {
    /// Path relative to the models dir, e.g. "cohere-int8/tokens.txt".
    dest: &'static str,
    url: &'static str,
    size: u64,
    sha256: &'static str,
}

/// Directory models are only loadable once complete; their files stage in
/// `<dir>.downloading` until every one is present.
const DIRECTORY_MODELS: [&str; 1] = ["cohere-int8"];

const MODEL_FILES: [ModelFile; 5] = [
    ModelFile {
        dest: "cohere-int8/cohere-decoder.int8.onnx",
        url: "https://huggingface.co/tristanripke/cohere-transcribe-onnx-int8/resolve/9ecc3a5e64b132ab094bada232650e49e4340ad2/cohere-decoder.int8.onnx",
        size: 153_250_705,
        sha256: "8372ca6c8ff4db8b916ca3592f5c757a715e691b9edec751ba19b29fc854baf9",
    },
    ModelFile {
        dest: "cohere-int8/cohere-encoder.int8.onnx",
        url: "https://huggingface.co/tristanripke/cohere-transcribe-onnx-int8/resolve/9ecc3a5e64b132ab094bada232650e49e4340ad2/cohere-encoder.int8.onnx",
        size: 3_118_156,
        sha256: "58386cad715aa0ab30aaa118a479e43115380c114bd180178a0d110434991a54",
    },
    ModelFile {
        dest: "cohere-int8/cohere-encoder.int8.onnx.data",
        url: "https://huggingface.co/tristanripke/cohere-transcribe-onnx-int8/resolve/9ecc3a5e64b132ab094bada232650e49e4340ad2/cohere-encoder.int8.onnx.data",
        size: 2_732_687_328,
        sha256: "c115cacd07bef2c5d6bbfa800bb38e6f025ecbfbd220b81b711f0eef8cc28578",
    },
    ModelFile {
        dest: "cohere-int8/tokens.txt",
        url: "https://huggingface.co/tristanripke/cohere-transcribe-onnx-int8/resolve/9ecc3a5e64b132ab094bada232650e49e4340ad2/tokens.txt",
        size: 223_821,
        sha256: "5e74bb2f65da624256b9d97fef197a282ce7d14811e2f7b1b97c25c89b93dfcb",
    },
    ModelFile {
        dest: "s1-mini-q4_k_m.gguf",
        url: "https://huggingface.co/superwhisper/s1-mini-GGUF/resolve/8eab4779866f477ae6e7f237ca45fc2c65153f50/s1-mini-q4_k_m.gguf",
        size: 484_219_808,
        sha256: "3b41ebe2502cbd03e811d5d16b022f5ab551eda58d62597d152f89535003c634",
    },
];

/// Ensure every manifest file is present under `models_dir`, downloading what
/// is missing. `progress` receives `(file, downloaded_bytes, total_bytes)`.
pub fn ensure_models(models_dir: &Path, progress: &mut dyn FnMut(&str, u64, u64)) -> Result<()> {
    ensure_files(models_dir, &MODEL_FILES, &DIRECTORY_MODELS, progress)
}

fn ensure_files(
    models_dir: &Path,
    files: &[ModelFile],
    directory_models: &[&str],
    progress: &mut dyn FnMut(&str, u64, u64),
) -> Result<()> {
    fs::create_dir_all(models_dir)?;
    for file in files {
        let target = match download_path(models_dir, file.dest) {
            Some(path) => path,
            // Already present (final location, or verified in staging).
            None => continue,
        };
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        println!(
            "downloading {} ({} MB)...",
            file.dest,
            file.size / 1_000_000
        );
        fetch(
            &RemoteFile {
                url: file.url.to_string(),
                size: file.size,
                sha256: file.sha256.to_string(),
            },
            &target,
            &mut |done, total| progress(file.dest, done, total),
        )
        .with_context(|| format!("downloading {}", file.dest))?;
    }
    promote_completed_directories(models_dir, files, directory_models)
}

/// Where `dest` should be downloaded to, or `None` if it already exists.
/// Directory-model files stage in `<dir>.downloading`; a file already there
/// was sha-verified when its `.partial` finalized, so it is never re-fetched.
/// A file missing from an already-promoted directory is repaired in place
/// (each fetch is individually atomic), never staged, since promotion would
/// discard the staging dir.
fn download_path(models_dir: &Path, dest: &str) -> Option<PathBuf> {
    let final_path = models_dir.join(dest);
    if final_path.exists() {
        return None;
    }
    let path = match dest.split_once('/') {
        Some((dir, name)) if !models_dir.join(dir).exists() => {
            models_dir.join(format!("{dir}.downloading")).join(name)
        }
        _ => final_path,
    };
    (!path.exists()).then_some(path)
}

fn promote_completed_directories(
    models_dir: &Path,
    files: &[ModelFile],
    directory_models: &[&str],
) -> Result<()> {
    for dir in directory_models {
        let staging = models_dir.join(format!("{dir}.downloading"));
        let final_dir = models_dir.join(dir);
        if final_dir.exists() {
            if staging.exists() {
                // Stale leftovers from a run that raced or crashed after
                // promotion; the final dir is the verified one.
                let _ = fs::remove_dir_all(&staging);
            }
            continue;
        }
        if !staging.exists() {
            continue;
        }
        let prefix = format!("{dir}/");
        let members: Vec<_> = files
            .iter()
            .filter(|f| f.dest.starts_with(&prefix))
            .collect();
        // A directory model with no manifest members is drift, not "complete".
        let complete = !members.is_empty()
            && members
                .iter()
                .all(|f| staging.join(&f.dest[prefix.len()..]).exists());
        if complete {
            fs::rename(&staging, &final_dir)
                .with_context(|| format!("promoting completed {dir}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_server::{Behavior, serve, test_body};
    use sha2::{Digest, Sha256};

    fn models_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dkt-manifest-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn model_file(dest: &'static str, url: String, body: &[u8]) -> ModelFile {
        ModelFile {
            dest,
            url: url.leak(),
            size: body.len() as u64,
            sha256: hex::encode(Sha256::digest(body)).leak(),
        }
    }

    #[test]
    fn present_files_are_not_refetched() {
        let dir = models_dir("present");
        fs::create_dir_all(dir.join("m")).unwrap();
        fs::write(dir.join("m/a.bin"), b"x").unwrap();
        fs::write(dir.join("single.bin"), b"y").unwrap();
        // Unreachable URL proves no request is even attempted.
        let files = [
            model_file("m/a.bin", "http://127.0.0.1:1/a".into(), b"x"),
            model_file("single.bin", "http://127.0.0.1:1/b".into(), b"y"),
        ];
        ensure_files(&dir, &files, &["m"], &mut |_, _, _| {}).unwrap();
    }

    #[test]
    fn downloads_missing_single_file() {
        let body = test_body();
        let (url, _) = serve(body.clone(), Behavior::Normal);
        let dir = models_dir("single");
        let files = [model_file("model.bin", url, &body)];
        ensure_files(&dir, &files, &[], &mut |_, _, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("model.bin")).unwrap(), body);
    }

    #[test]
    fn directory_model_stages_then_promotes() {
        let body = test_body();
        let (url_a, _) = serve(body.clone(), Behavior::Normal);
        let (url_b, _) = serve(body.clone(), Behavior::Normal);
        let dir = models_dir("dir");
        let files = [
            model_file("m/a.bin", url_a, &body),
            model_file("m/b.bin", url_b, &body),
        ];
        ensure_files(&dir, &files, &["m"], &mut |_, _, _| {}).unwrap();
        assert!(dir.join("m/a.bin").exists() && dir.join("m/b.bin").exists());
        assert!(!dir.join("m.downloading").exists());
    }

    #[test]
    fn resumes_directory_model_without_refetching_done_files() {
        let body = test_body();
        let (url_b, requests_b) = serve(body.clone(), Behavior::Normal);
        let dir = models_dir("dir-resume");
        // a.bin already completed into staging by an earlier run.
        fs::create_dir_all(dir.join("m.downloading")).unwrap();
        fs::write(dir.join("m.downloading/a.bin"), &body).unwrap();
        let files = [
            model_file("m/a.bin", "http://127.0.0.1:1/a".into(), &body),
            model_file("m/b.bin", url_b, &body),
        ];
        ensure_files(&dir, &files, &["m"], &mut |_, _, _| {}).unwrap();
        assert!(dir.join("m/a.bin").exists() && dir.join("m/b.bin").exists());
        assert_eq!(requests_b.lock().unwrap().len(), 1);
    }

    /// A file missing from a promoted directory is repaired in place; staging
    /// would be discarded by the stale-staging cleanup.
    #[test]
    fn repairs_missing_file_inside_promoted_directory() {
        let body = test_body();
        let (url_b, _) = serve(body.clone(), Behavior::Normal);
        let dir = models_dir("repair");
        fs::create_dir_all(dir.join("m")).unwrap();
        fs::write(dir.join("m/a.bin"), &body).unwrap();
        let files = [
            model_file("m/a.bin", "http://127.0.0.1:1/a".into(), &body),
            model_file("m/b.bin", url_b, &body),
        ];
        ensure_files(&dir, &files, &["m"], &mut |_, _, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("m/b.bin")).unwrap(), body);
        assert!(!dir.join("m.downloading").exists());
    }

    #[test]
    fn stale_staging_is_removed_when_final_exists() {
        let dir = models_dir("stale");
        fs::create_dir_all(dir.join("m")).unwrap();
        fs::write(dir.join("m/a.bin"), b"x").unwrap();
        fs::create_dir_all(dir.join("m.downloading")).unwrap();
        fs::write(dir.join("m.downloading/a.bin"), b"old").unwrap();
        let files = [model_file("m/a.bin", "http://127.0.0.1:1/a".into(), b"x")];
        ensure_files(&dir, &files, &["m"], &mut |_, _, _| {}).unwrap();
        assert!(!dir.join("m.downloading").exists());
        assert_eq!(fs::read(dir.join("m/a.bin")).unwrap(), b"x");
    }
}
