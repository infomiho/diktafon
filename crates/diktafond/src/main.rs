use anyhow::Result;
use std::path::PathBuf;

fn models_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set"))
        .join("Library/Application Support/diktafon/models")
}

fn main() -> Result<()> {
    diktafond::daemon::run(&models_dir(), &diktafon_protocol::socket_path())
}
