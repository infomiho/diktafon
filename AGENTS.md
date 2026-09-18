# diktafon

Local-only macOS dictation: hold Option+Space, speak, release; transcribed (Cohere Transcribe) and polished (S1-mini) text is pasted into the frontmost app.

- Before committing, run `scripts/ci-checks.sh`: it is the exact set CI runs (`cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`). Weaker local checks miss lints in test and example targets. `git config core.hooksPath scripts/hooks` installs it as a pre-commit hook.
- Tasks are managed in Beads: `bd ready` to pick work, `bd show <id>` for details, update status as you go.
- Rust 1.97+ (gpui's floor, enforced via `rust-version`); `rustup update stable` if the build fails deep inside gpui.
- Target architecture: always client/server. `diktafon` client (hotkey, mic capture, silence chunking, paste, UI) and `diktafond` daemon (resident models) speak one streaming protocol, over a Unix socket locally or WebSocket remotely. See the M2 epic in Beads.
- Data dir `~/Library/Application Support/diktafon/` (override: `DIKTAFON_DATA_DIR`) holds the models, the daemon socket/log, and `history.jsonl` (every dictation in plaintext, so treat it as sensitive; the daemon log beside it holds the same text and is never rotated). Models live in `models/` inside it. Model selection rationale and benchmark method: `docs/benchmarks.md`. A 5-clip eval set with confirmed ground truth is in `~/Library/Application Support/diktafon/eval-own/`.
- S1-mini requires its exact prompt format (system prompt, control line, empty think block); see `crates/diktafond/src/llm.rs`.
- Running needs macOS permissions: microphone, and Accessibility for the synthesized Cmd+V. From a terminal they attach to that terminal; `scripts/bundle.sh` builds `target/diktafon.app` so they attach to the app itself, signed with the machine's Apple Development identity so grants survive rebuilds (ad-hoc fallback re-prompts).

## Releases

Pushing a `v*` tag triggers `.github/workflows/release.yml`, which builds `diktafon.app`, signs and notarizes a DMG, and runs `gh release create` for that tag. Do NOT create the GitHub release manually (`gh release create` or the web UI) after pushing a tag; the release already exists by the time CI finishes publishing, so the workflow fails with "a release with the same tag name already exists".

To cut a release: bump the workspace version, commit, tag `vX.Y.Z`, push the tag, and let CI publish. Versions must be plain `MAJOR.MINOR.PATCH`: `scripts/package-app.sh` derives the bundle's numeric `CFBundleVersion` from it and refuses suffixes.

`scripts/setup-release-signing.sh` stores the Developer ID certificate and the App Store Connect API key as repo secrets once. The workflow also calls the Coolify webhook (`COOLIFY_WEBHOOK`, `COOLIFY_TOKEN`) so the tag refreshes `diktafon.miho.dev`.

After publishing, the workflow runs `scripts/update-tap.sh`, which rewrites the version and sha256 of `Casks/diktafon.rb` in `infomiho/homebrew-tap` (a local checkout lives at `~/dev/homebrew-tap`) so `brew install --cask infomiho/tap/diktafon` serves the new build. It pushes with the `TAP_GITHUB_TOKEN` secret: a fine-grained personal access token with Contents read and write on that one repo, stored once with `gh secret set TAP_GITHUB_TOKEN`.

The workflow also signs the DMG with the Sparkle key from the `SPARKLE_PRIVATE_KEY` secret and publishes `appcast.xml` as a release asset. Installed copies read `https://github.com/infomiho/diktafon/releases/latest/download/appcast.xml` and update themselves through the embedded Sparkle framework (`crates/diktafon/src/updater.rs`). `scripts/setup-sparkle-key.sh` creates the key, stores the secret, and writes the public key into `crates/diktafon/resources/Info.plist`. Back the private key up: without it no installed copy accepts an update. The tag message becomes the release notes shown in the update window. Debug builds, `bundle.sh` bundles, and ad-hoc packaging runs keep the updater inert.

## Web

`web/` is a separate crate (its own workspace) that serves the landing page and release notes at `diktafon.miho.dev`. See `web/AGENTS.md`. Validate with `cargo fmt --manifest-path web/Cargo.toml -- --check`, `cargo clippy --manifest-path web/Cargo.toml --all-targets -- -D warnings`, and `cargo test --manifest-path web/Cargo.toml`.
