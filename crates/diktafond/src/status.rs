//! Mirrors the daemon's state into `status.json` next to the socket, so UI
//! surfaces (the client's menu bar) can read it without a protocol roundtrip;
//! see [`diktafon_protocol::status_path`] for why a file.

use serde::Serialize;

#[derive(Serialize)]
struct Status {
    pid: u32,
    models_loaded: bool,
    asr_model: &'static str,
    llm_model: &'static str,
}

/// Rewrite the status file; called on every model load and unload. Failures
/// are logged, not fatal: status is best-effort decoration.
pub fn write(models_loaded: bool) {
    let status = Status {
        pid: std::process::id(),
        models_loaded,
        asr_model: "cohere-transcribe-int8",
        llm_model: "s1-mini-q4_k_m",
    };
    let path = diktafon_protocol::status_path();
    let result = (|| -> anyhow::Result<()> {
        let json = serde_json::to_vec(&status)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("writing {} failed: {e:#}", path.display());
    }
}

pub fn remove() {
    let _ = std::fs::remove_file(diktafon_protocol::status_path());
}
