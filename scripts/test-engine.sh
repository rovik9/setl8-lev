#!/usr/bin/env bash
# Dependency-free checks: the engine and journal-types crates build and test offline.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --offline
