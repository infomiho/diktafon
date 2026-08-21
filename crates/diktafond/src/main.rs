use anyhow::Result;

fn main() -> Result<()> {
    diktafond::daemon::run(&diktafon_protocol::models_dir(), &diktafon_protocol::socket_path())
}
