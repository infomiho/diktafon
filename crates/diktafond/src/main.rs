use anyhow::Result;
use std::path::PathBuf;

fn models_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set"))
        .join("Library/Application Support/diktafon/models")
}

fn main() -> Result<()> {
    let socket = match std::env::var_os("DIKTAFOND_SOCKET") {
        Some(path) => PathBuf::from(path),
        None => diktafon_protocol::socket_path(),
    };
    diktafond::daemon::run(&models_dir(), &socket)
}
