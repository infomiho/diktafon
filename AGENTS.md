# diktafon

Local-only macOS dictation: hold Option+Space, speak, release; transcribed (Cohere Transcribe) and polished (S1-mini) text is pasted into the frontmost app.

- Tasks are managed in Beads: `bd ready` to pick work, `bd show <id>` for details, update status as you go.
- Rust 1.97+ (gpui's floor, enforced via `rust-version`); `rustup update stable` if the build fails deep inside gpui.
- Target architecture: always client/server. `diktafon` client (hotkey, mic capture, silence chunking, paste, UI) and `diktafond` daemon (resident models) speak one streaming protocol, over a Unix socket locally or WebSocket remotely. See the M2 epic in Beads.
- Models live in `~/Library/Application Support/diktafon/models/`. Model selection rationale and benchmark method: `docs/benchmarks.md`. A 5-clip eval set with confirmed ground truth is in `~/Library/Application Support/diktafon/eval-own/`.
- S1-mini requires its exact prompt format (system prompt, control line, empty think block); see `crates/diktafond/src/llm.rs`.
- Running needs macOS permissions: microphone, and Accessibility for the synthesized Cmd+V. From a terminal they attach to that terminal; `scripts/bundle.sh` builds `target/diktafon.app` so they attach to the app itself (ad-hoc signature, so a rebuild re-prompts).
