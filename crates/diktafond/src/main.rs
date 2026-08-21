use anyhow::Result;

fn main() -> Result<()> {
    // ggml's Metal backend asserts on residency-set teardown (seen at exit and
    // guarded there with _exit); disabling residency sets avoids the whole
    // class, including model drops from idle unload. Must precede Metal init;
    // safe: no other threads exist yet.
    unsafe { std::env::set_var("GGML_METAL_NO_RESIDENCY", "1") };
    diktafond::daemon::run(
        &diktafon_protocol::models_dir(),
        &diktafon_protocol::socket_path(),
    )
}
