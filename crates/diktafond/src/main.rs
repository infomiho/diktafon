use anyhow::Result;

fn main() -> Result<()> {
    // ggml's Metal backend asserts on residency-set teardown (seen at exit and
    // guarded there with _exit); disabling residency sets avoids the whole
    // class, including model drops from idle unload. Must precede Metal init;
    // safe: no other threads exist yet.
    unsafe { std::env::set_var("GGML_METAL_NO_RESIDENCY", "1") };
    // `diktafond --polish-file <txt>`: run one polish pass over a transcript
    // and report the sizes, for measuring the polish stage (its cost dominates
    // the wait) without recording audio.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--polish-file") {
        return polish_file(args.get(1).map(String::as_str));
    }
    diktafond::daemon::run(
        &diktafon_protocol::models_dir(),
        &diktafon_protocol::socket_path(),
    )
}

fn polish_file(path: Option<&str>) -> Result<()> {
    use anyhow::Context;
    let path = path.context("usage: --polish-file <transcript.txt>")?;
    let transcript = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    let polisher = diktafond::llm::Polisher::load(
        &diktafon_protocol::models_dir().join("s1-mini-q4_k_m.gguf"),
    )?;
    let control_line = diktafon_protocol::SessionConfig::default().control_line;
    let start = std::time::Instant::now();
    let polished = polisher.polish(transcript.trim(), &control_line)?;
    let (words_in, words_out) = (
        transcript.split_whitespace().count(),
        polished.split_whitespace().count(),
    );
    println!(
        "in {words_in} words, out {words_out} words ({:.0}%), {:.2?}",
        100.0 * words_out as f32 / words_in.max(1) as f32,
        start.elapsed()
    );
    println!(
        "--- tail: {}",
        polished
            .split_whitespace()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(())
}
