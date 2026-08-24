#!/usr/bin/env bash
# The exact checks .github/workflows/ci.yml runs, so a green run here means a
# green run there. Keep the two in step.
set -euo pipefail

cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
