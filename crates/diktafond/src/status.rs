//! Mirrors the daemon's state into `status.json` next to the socket, so UI
//! surfaces (the client's menu bar) can read it without a protocol roundtrip;
//! see [`diktafon_protocol::status_path`] for why a file.

use serde::Serialize;

#[derive(Serialize)]
struct Status {
    pid: u32,
    models_loaded: bool,
    transcription_model_id: String,
    polishing_model_id: String,
    asr_model: String,
    llm_model: String,
    asr_backend: Option<String>,
    asr_device: Option<String>,
    polishing_availability: Option<String>,
}

/// Rewrite the status file; called on every model load and unload. Failures
/// are logged, not fatal: status is best-effort decoration.
pub fn write(
    path: &std::path::Path,
    models_loaded: bool,
    selection: &diktafon_protocol::ModelSelection,
    asr_backend: Option<&str>,
    asr_device: Option<&str>,
) {
    let asr_model = crate::manifest::model(&selection.transcription)
        .map(|model| model.name.clone())
        .unwrap_or_else(|_| selection.transcription.clone());
    let llm_model = crate::manifest::model(&selection.polishing)
        .map(|model| model.name.clone())
        .unwrap_or_else(|_| selection.polishing.clone());
    let status = Status {
        pid: std::process::id(),
        models_loaded,
        transcription_model_id: selection.transcription.clone(),
        polishing_model_id: selection.polishing.clone(),
        asr_model,
        llm_model,
        asr_backend: asr_backend.map(str::to_string),
        asr_device: asr_device.map(str::to_string),
        polishing_availability: Some(
            crate::apple_intelligence::availability()
                .label()
                .to_string(),
        ),
    };
    let result = (|| -> anyhow::Result<()> {
        let json = serde_json::to_vec(&status)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("writing {} failed: {e:#}", path.display());
    }
}

pub fn remove(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}
