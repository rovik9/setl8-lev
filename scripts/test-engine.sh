#!/usr/bin/env bash
# Dependency-free checks: the engine, journal-types, verifier and evaluator crates build and test offline.
# `cargo test --offline` runs the whole workspace; the verifier and the evaluator are also named so a failure there is obvious.
# The evaluator's engine-only campaign runs 2,000 paths here (`ENGINE_PATHS=100000` is the gate).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --offline --locked
cargo test --offline --locked -p setl8-lev-verifier
cargo test --offline --locked -p setl8-lev-evaluator
