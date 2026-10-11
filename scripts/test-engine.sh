#!/usr/bin/env bash
# Dependency-free checks: the engine, journal-types and verifier crates build and test offline.
# `cargo test --offline` runs the whole workspace; the verifier is also named so a failure there is obvious.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --offline
cargo test --offline -p setl8-lev-verifier
