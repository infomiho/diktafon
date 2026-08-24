# diktafon

Local-only macOS dictation: hold Option+Space, speak, release; transcribed (Cohere Transcribe) and polished (S1-mini) text is pasted into the frontmost app.

- Before committing, run `scripts/ci-checks.sh`: it is the exact set CI runs (`cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`). Weaker local checks miss lints in test and example targets. `git config core.hooksPath scripts/hooks` installs it as a pre-commit hook.
- Tasks are managed in Beads: `bd ready` to pick work, `bd show <id>` for details, update status as you go.
- Rust 1.97+ (gpui's floor, enforced via `rust-version`); `rustup update stable` if the build fails deep inside gpui.
- Target architecture: always client/server. `diktafon` client (hotkey, mic capture, silence chunking, paste, UI) and `diktafond` daemon (resident models) speak one streaming protocol, over a Unix socket locally or WebSocket remotely. See the M2 epic in Beads.
- Data dir `~/Library/Application Support/diktafon/` (override: `DIKTAFON_DATA_DIR`) holds the models, the daemon socket/log, and `history.jsonl` (every dictation in plaintext, so treat it as sensitive). Models live in `models/` inside it. Model selection rationale and benchmark method: `docs/benchmarks.md`. A 5-clip eval set with confirmed ground truth is in `~/Library/Application Support/diktafon/eval-own/`.
- S1-mini requires its exact prompt format (system prompt, control line, empty think block); see `crates/diktafond/src/llm.rs`.
- Running needs macOS permissions: microphone, and Accessibility for the synthesized Cmd+V. From a terminal they attach to that terminal; `scripts/bundle.sh` builds `target/diktafon.app` so they attach to the app itself, signed with the machine's Apple Development identity so grants survive rebuilds (ad-hoc fallback re-prompts).
