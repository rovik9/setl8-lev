#!/usr/bin/env bash
# Supply-chain checks that need the network (audit finding F-39). RUN BY ROVIK ON HIS MAC, ONLINE. Nothing in this repo runs it.
#   cargo install cargo-audit cargo-deny      (once)
#   scripts/online-supply-chain-check.sh
# Checks the root workspace, tests-rs and tools/launch-config against the RustSec advisory database (cargo audit) and the licenses, bans,
# advisories and sources policy of deny.toml (cargo deny check; create deny.toml with `cargo deny init` the first time and review it).
# A yanked crate shows up as a cargo audit warning. The git dependency setl8-shared-interfaces is pinned by rev in Cargo.toml; `cargo deny`
# must list it as an allowed git source (deny.toml [sources] allow-git).
set -euo pipefail
cd "$(dirname "$0")/.."
for dir in . tests-rs tools/launch-config; do
  echo "== $dir: cargo audit"
  (cd "$dir" && cargo audit)
  echo "== $dir: cargo deny check"
  (cd "$dir" && cargo deny check)
done
echo "supply-chain checks finished: read every warning; yanked or unmaintained crates are decisions, not noise"
