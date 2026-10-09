#!/usr/bin/env bash
# `anchor build` exits 0 even when the SBF toolchain reports a stack frame over
# the 4,096-byte limit. This wrapper fails the build if such a warning appears
# (same approach as the vault's script of the same name), then prints the .so sha256.
# Usage: scripts/anchor-build-checked.sh [extra `anchor build` args]
set -uo pipefail
cd "$(dirname "$0")/.."

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

anchor build "$@" 2>&1 | tee "$log"
status="${PIPESTATUS[0]}"
if [ "$status" -ne 0 ]; then
  echo "anchor build failed (exit $status)" >&2
  exit "$status"
fi

if grep -qE 'overflows the maximum allowed frame|Stack offset' "$log"; then
  echo >&2
  echo "ERROR: the build reported a stack-frame overflow (exceeds 4096 bytes):" >&2
  grep -E 'overflows the maximum allowed frame|Stack offset' "$log" >&2
  echo "Box the large accounts (Box<Account<..>>) or shrink the frame." >&2
  exit 1
fi

so="target/deploy/setl8_lev_trading.so"
echo "$so sha256: $(shasum -a 256 "$so" | cut -d' ' -f1)"
echo "OK: no stack-frame overflow reported."
