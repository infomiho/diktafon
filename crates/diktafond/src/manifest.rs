//! Bundled model catalog and provisioning. The JSON is release metadata, not
//! runtime configuration: it is compiled into the daemon and validated before
//! any path is joined or download begins.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use crate::fetch::{RemoteFile, fetch};

pub use diktafon_protocol::{DEFAULT_POLISHING_MODEL, DEFAULT_TRANSCRIPTION_MODEL};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ModelCategory {
    Transcription,
    Polishing,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    LlamaCpp,
    TranscribeCpp,
    AppleFoundationModels,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Capabilities {
    pub streaming: bool,
    pub translation: bool,
    pub language_detection: bool,
    pub timestamps: bool,
    pub local_only: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelFile {
    /// Path relative to the models dir.
    #[serde(rename = "path")]
    pub dest: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub category: ModelCategory,
    pub name: String,
    pub backend: ModelBackend,
    #[allow(dead_code)]
    pub languages: Vec<String>,
    #[allow(dead_code)]
    pub capabilities: Capabilities,
    #[allow(dead_code)]
    pub license: String,
    #[allow(dead_code)]
    pub notices: Vec<String>,
    pub directory: Option<String>,
    #[allow(dead_code)]
    pub minimum_macos: Option<u32>,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Deserialize)]
struct Catalog {
    version: u32,
    models: Vec<Model>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let catalog: Catalog = serde_json::from_str(diktafon_protocol::MODEL_CATALOG_JSON)
        .expect("bundled models.json must parse");
    validate_catalog(&catalog).expect("bundled models.json must be valid");
    catalog
});

pub fn model(id: &str) -> Result<&'static Model> {
    CATALOG
        .models
        .iter()
        .find(|model| model.id == id)
        .with_context(|| format!("unknown model {id:?}"))
}

pub fn model_path(models_dir: &Path, id: &str) -> Result<PathBuf> {
    let model = model(id)?;
    if let Some(directory) = &model.directory {
        return Ok(models_dir.join(directory));
    }
    ensure!(
        model.files.len() == 1,
        "model {id:?} has no single load path"
    );
    Ok(models_dir.join(&model.files[0].dest))
}

pub fn validate_selection(selection: &diktafon_protocol::ModelSelection) -> Result<()> {
    for (id, category) in [
        (&selection.transcription, ModelCategory::Transcription),
        (&selection.polishing, ModelCategory::Polishing),
    ] {
        let selected = model(id)?;
        ensure!(
            selected.category == category,
            "model {id:?} belongs to the wrong category"
        );
    }
    Ok(())
}

fn validate_catalog(catalog: &Catalog) -> Result<()> {
    const LICENSES: &[&str] = &[
        "apache-2.0",
        "cc-by-4.0",
        "s1-mini-license",
        "system-provided",
    ];
    const NOTICES: &[&str] = &[
        "apple-foundation-models",
        "cohere-transcribe",
        "handy-canary-gguf",
        "handy-cohere-gguf",
        "llama-cpp",
        "nvidia-canary",
        "qwen3",
        "s1-mini",
        "transcribe-cpp",
    ];
    ensure!(
        catalog.version == 1,
        "unsupported catalog version {}",
        catalog.version
    );
    ensure!(!catalog.models.is_empty(), "catalog contains no models");

    let mut ids = HashSet::new();
    let mut destinations = HashSet::new();
    for model in &catalog.models {
        ensure!(!model.id.is_empty(), "model ID is empty");
        ensure!(
            model
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
            "model ID {:?} is not a lowercase slug",
            model.id
        );
        ensure!(ids.insert(&model.id), "duplicate model ID {:?}", model.id);
        ensure!(
            !model.name.trim().is_empty(),
            "model {:?} has no name",
            model.id
        );
        ensure!(
            LICENSES.contains(&model.license.as_str()),
            "model {:?} has unknown license {:?}",
            model.id,
            model.license
        );
        ensure!(
            !model.notices.is_empty(),
            "model {:?} has no notices",
            model.id
        );
        for notice in &model.notices {
            ensure!(
                NOTICES.contains(&notice.as_str()),
                "model {:?} has unknown notice {:?}",
                model.id,
                notice
            );
        }
        ensure!(
            !model.languages.is_empty(),
            "model {:?} has no languages",
            model.id
        );

        let backend_matches_category = matches!(
            (model.category, model.backend),
            (ModelCategory::Transcription, ModelBackend::TranscribeCpp)
                | (
                    ModelCategory::Polishing,
                    ModelBackend::LlamaCpp | ModelBackend::AppleFoundationModels
                )
        );
        ensure!(
            backend_matches_category,
            "model {:?} has a backend incompatible with its category",
            model.id
        );

        let system_model = model.backend == ModelBackend::AppleFoundationModels;
        if system_model {
            ensure!(
                model.files.is_empty(),
                "system model {:?} has files",
                model.id
            );
            ensure!(
                model.directory.is_none(),
                "system model {:?} has a directory",
                model.id
            );
            ensure!(
                model.minimum_macos.is_some(),
                "system model {:?} has no minimum macOS",
                model.id
            );
        } else {
            ensure!(
                !model.files.is_empty(),
                "downloadable model {:?} has no files",
                model.id
            );
        }

        if let Some(directory) = &model.directory {
            validate_relative_path(directory)
                .with_context(|| format!("model {:?} directory", model.id))?;
            let prefix = format!("{directory}/");
            ensure!(
                model
                    .files
                    .iter()
                    .all(|file| file.dest.starts_with(&prefix)),
                "model {:?} has a file outside directory {:?}",
                model.id,
                directory
            );
        }

        for file in &model.files {
            validate_relative_path(&file.dest)
                .with_context(|| format!("model {:?} file path", model.id))?;
            ensure!(
                destinations.insert(&file.dest),
                "duplicate model destination {:?}",
                file.dest
            );
            ensure!(file.size > 0, "model file {:?} has zero size", file.dest);
            ensure!(
                file.sha256.len() == 64 && file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "model file {:?} has an invalid sha256",
                file.dest
            );
            ensure!(
                file.url.starts_with("https://") && pinned_revision(&file.url).is_some(),
                "model file {:?} URL is not pinned to a commit",
                file.dest
            );
        }
    }

    for (id, category) in [
        (DEFAULT_TRANSCRIPTION_MODEL, ModelCategory::Transcription),
        (DEFAULT_POLISHING_MODEL, ModelCategory::Polishing),
    ] {
        let selected = catalog
            .models
            .iter()
            .find(|model| model.id == id)
            .with_context(|| format!("default model {id:?} is absent"))?;
        ensure!(
            selected.category == category,
            "default model {id:?} has the wrong category"
        );
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<()> {
    ensure!(!path.is_empty(), "path is empty");
    let path = Path::new(path);
    ensure!(!path.is_absolute(), "path is absolute");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "path contains a non-normal component"
    );
    Ok(())
}

fn pinned_revision(url: &str) -> Option<&str> {
    let revision = url.split("/resolve/").nth(1)?.split('/').next()?;
    (revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(revision)
}

/// Ensure every manifest file is present under `models_dir`, downloading what
/// is missing. `progress` receives `(file, downloaded_bytes, total_bytes)`.
pub fn ensure_selected_models(
    models_dir: &Path,
    selection: &diktafon_protocol::ModelSelection,
    progress: &mut dyn FnMut(&str, u64, u64),
) -> Result<()> {
    validate_selection(selection)?;
    let selected = [
        model(&selection.transcription)?,
        model(&selection.polishing)?,
    ];
    let files: Vec<_> = selected
        .iter()
        .copied()
        .flat_map(|model| model.files.iter().cloned())
        .collect();
    let directories: Vec<_> = selected
        .iter()
        .filter_map(|model| model.directory.as_deref())
        .collect();
    ensure_files(models_dir, &files, &directories, progress)
}

fn ensure_files(
    models_dir: &Path,
    files: &[ModelFile],
    directory_models: &[&str],
    progress: &mut dyn FnMut(&str, u64, u64),
) -> Result<()> {
    fs::create_dir_all(models_dir)?;
    for file in files {
        let target = match download_path(models_dir, &file.dest) {
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
                url: file.url.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
            },
            &target,
            &mut |done, total| progress(&file.dest, done, total),
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

    fn model_file(dest: &str, url: String, body: &[u8]) -> ModelFile {
        ModelFile {
            dest: dest.to_string(),
            url,
            size: body.len() as u64,
            sha256: hex::encode(Sha256::digest(body)),
        }
    }

    fn bundled_value() -> serde_json::Value {
        serde_json::from_str(diktafon_protocol::MODEL_CATALOG_JSON).unwrap()
    }

    fn validate_value(value: serde_json::Value) -> Result<()> {
        let catalog: Catalog = serde_json::from_value(value)?;
        validate_catalog(&catalog)
    }

    #[test]
    fn bundled_catalog_is_valid_and_resolves_load_paths() {
        validate_value(bundled_value()).unwrap();
        let root = Path::new("/models");
        assert_eq!(
            model_path(root, DEFAULT_TRANSCRIPTION_MODEL).unwrap(),
            root.join("canary-1b-flash-Q5_K_M.gguf")
        );
        assert_eq!(
            model_path(root, DEFAULT_POLISHING_MODEL).unwrap(),
            root.join("s1-mini-q4_k_m.gguf")
        );
        let canary = model("canary-1b-flash-q5-k-m").unwrap();
        assert_eq!(canary.backend, ModelBackend::TranscribeCpp);
        assert_eq!(canary.languages, ["en", "de", "es", "fr"]);
        assert_eq!(canary.files.len(), 1);
        assert_eq!(canary.files[0].size, 769_563_424);
        assert_eq!(
            canary.files[0].sha256,
            "7eed3cac92f255a4adbd518c58663d3fbf65984d2619189e593f2d374b05c601"
        );
        let canary_v2 = model("canary-1b-v2-q5-k-m").unwrap();
        assert_eq!(canary_v2.backend, ModelBackend::TranscribeCpp);
        assert!(canary_v2.languages.iter().any(|language| language == "hr"));
        assert_eq!(
            model_path(root, "canary-1b-v2-q5-k-m").unwrap(),
            root.join("canary-1b-v2-Q5_K_M.gguf")
        );
    }

    #[test]
    fn catalog_rejects_version_identity_and_destination_drift() {
        let mut value = bundled_value();
        value["version"] = 2.into();
        assert!(validate_value(value).is_err());

        let mut value = bundled_value();
        value["models"][1]["id"] = value["models"][0]["id"].clone();
        assert!(validate_value(value).is_err());

        let mut value = bundled_value();
        value["models"][1]["files"][0]["path"] = value["models"][0]["files"][0]["path"].clone();
        assert!(validate_value(value).is_err());
    }

    #[test]
    fn catalog_requires_known_license_and_notice_ids() {
        let mut value = bundled_value();
        value["models"][0]["license"] = "unknown".into();
        assert!(validate_value(value).is_err());

        let mut value = bundled_value();
        value["models"][0]["notices"] = serde_json::json!([]);
        assert!(validate_value(value).is_err());

        let mut value = bundled_value();
        value["models"][0]["notices"] = serde_json::json!(["unknown"]);
        assert!(validate_value(value).is_err());
    }

    #[test]
    fn catalog_rejects_unsafe_paths_and_unpinned_files() {
        for path in ["/tmp/model.gguf", "../model.gguf", "models/../model.gguf"] {
            let mut value = bundled_value();
            value["models"][1]["files"][0]["path"] = path.into();
            assert!(validate_value(value).is_err(), "accepted {path:?}");
        }

        for field in ["size", "sha256", "url"] {
            let mut value = bundled_value();
            value["models"][1]["files"][0][field] = match field {
                "size" => 0.into(),
                "sha256" => "bad".into(),
                "url" => "https://example.com/model.gguf".into(),
                _ => unreachable!(),
            };
            assert!(validate_value(value).is_err(), "accepted bad {field}");
        }
    }

    #[test]
    fn catalog_rejects_unknown_or_incompatible_backends() {
        let mut value = bundled_value();
        value["models"][0]["backend"] = "shell_command".into();
        let parsed: Result<Catalog, _> = serde_json::from_value(value);
        assert!(parsed.is_err());

        let mut value = bundled_value();
        value["models"][0]["backend"] = "llama_cpp".into();
        assert!(validate_value(value).is_err());
    }

    #[test]
    fn system_models_require_no_files_and_a_minimum_os() {
        let mut value = bundled_value();
        validate_value(value.clone()).unwrap();

        let apple_index = value["models"]
            .as_array()
            .unwrap()
            .iter()
            .position(|model| model["id"] == "apple-intelligence")
            .unwrap();
        value["models"][apple_index]
            .as_object_mut()
            .unwrap()
            .remove("minimum_macos");
        assert!(validate_value(value).is_err());
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
