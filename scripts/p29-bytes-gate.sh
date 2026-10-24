#!/usr/bin/env bash
# P29 byte-equality gate (handover/runtime-hosting-design.md 10.1): runs the three campaigns in CHECK mode against
# handover/audit/p29-baseline.json and prints PASS/FAIL with timings. Every step of the runtime-hosting work and the
# news-spread evaluator work runs exactly this. Exit status 0 only if every digest equals the baseline.
#
#   scripts/p29-bytes-gate.sh            check (default)
#   P29_MODE=write scripts/p29-bytes-gate.sh   (re)write the baseline: ONLY from the unchanged code, by the prompt that says so
#
# Campaigns: engine-only 100,000 paths seed 1; engine-only 10,000 paths with SETL8_LAUNCH_COSTS=1 (the owner-cost gate);
# the differential's 10,000 paths (every batch posted through the real program in LiteSVM; needs target/test-deploy/*.so).
set -uo pipefail
cd "$(dirname "$0")/.."
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
export P29_MODE="${P29_MODE:-check}"
fail=0
now() { date +%s; }

run() { # name, label, env..., -- cmd...
  local name="$1" label="$2"; shift 2
  local t0; t0=$(now)
  local out; out=$(env "$@" 2>&1); local rc=$?
  local secs=$(( $(now) - t0 ))
  echo "$out" | grep -E "^P29 |P29 BYTE GATE FAILED|P29:" | sed "s/^/    /"
  if [ $rc -eq 0 ]; then echo "[PASS] $label (${secs}s)"; else echo "[FAIL] $label (${secs}s)"; echo "$out" | tail -25; fail=1; fi
}

echo "P29 byte gate, mode: $P29_MODE, baseline: handover/audit/p29-baseline.json"
T0=$(now)
run engine_100000 "engine-only 100,000 paths, seed 1" \
  -u SETL8_LAUNCH_COSTS P29_NAME=engine_100000 P29_PATHS=100000 P29_SEED=1 \
  cargo test --release --offline --locked -p setl8-lev-evaluator --test p29_bytes_gate p29_bytes_gate_campaign -- --ignored --nocapture
run engine_launch_10000 "engine-only 10,000 paths, owner launch costs (SETL8_LAUNCH_COSTS=1)" \
  SETL8_LAUNCH_COSTS=1 P29_NAME=engine_launch_10000 P29_PATHS=10000 P29_SEED=1 \
  cargo test --release --offline --locked -p setl8-lev-evaluator --test p29_bytes_gate p29_bytes_gate_campaign -- --ignored --nocapture
if [ ! -f target/test-deploy/setl8_lev_trading.so ]; then scripts/build-test-so.sh >/dev/null 2>&1; fi
run differential_10000 "differential 10,000 paths through the real program (LiteSVM)" \
  -u SETL8_LAUNCH_COSTS P29_NAME=differential_10000 P29_PATHS=10000 P29_SEED=1 \
  cargo test --release --offline --locked --manifest-path tests-rs/Cargo.toml --test p29_bytes_gate_diff -- --ignored --nocapture
echo "total $(( $(now) - T0 ))s"
if [ $fail -eq 0 ]; then echo "P29 BYTE GATE: PASS"; else echo "P29 BYTE GATE: FAIL"; fi
exit $fail
