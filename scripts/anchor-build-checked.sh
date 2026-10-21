#!/usr/bin/env bash
# `anchor build` exits 0 even when the SBF toolchain reports a stack frame over
# the 4,096-byte limit. This wrapper fails the build if such a warning appears
# (same approach as the vault's script of the same name), then prints the .so sha256.
#
# It checks the DEFAULT build (real admin keys, target/deploy/) and then also
# builds-and-checks the LOCALNET build (public test keys, target/test-deploy/,
# via scripts/build-test-so.sh). Set DEFAULT_ONLY=1 to skip the localnet build.
#
# Usage: [DEFAULT_ONLY=1] scripts/anchor-build-checked.sh [extra `anchor build` args]
set -uo pipefail
cd "$(dirname "$0")/.."

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# --locked (audit F-39): the build must use Cargo.lock exactly. `anchor build` cannot pass `--locked` (it forwards its cargo arguments to
# `cargo build-sbf` AND to the IDL step `cargo test`, and one of them rejects the flag), so the lock file is checked before and after:
# `cargo metadata --locked` fails if Cargo.lock needs an update, and the file must be byte-identical after the build.
cargo metadata --locked --offline --format-version 1 >/dev/null || { echo "Cargo.lock is out of date (cargo metadata --locked failed)" >&2; exit 1; }
lock_before="$(shasum -a 256 Cargo.lock | cut -d' ' -f1)"
anchor build "$@" 2>&1 | tee "$log"
status="${PIPESTATUS[0]}"
if [ "$status" -ne 0 ]; then
  echo "anchor build failed (exit $status)" >&2
  exit "$status"
fi

if [ "$(shasum -a 256 Cargo.lock | cut -d' ' -f1)" != "$lock_before" ]; then
  echo "ERROR: the build changed Cargo.lock (it must build with the lock file as committed)" >&2
  exit 1
fi

if grep -qE 'overflows the maximum allowed frame|Stack offset' "$log"; then
  echo >&2
  echo "ERROR: the build reported a stack-frame overflow (exceeds 4096 bytes):" >&2
  grep -E 'overflows the maximum allowed frame|Stack offset' "$log" >&2
  echo "Box the large accounts (Box<Account<..>>) or shrink the frame." >&2
  exit 1
fi

so="target/deploy/setl8_lev_trading.so"
echo "default (REAL keys) $so sha256: $(shasum -a 256 "$so" | cut -d' ' -f1)"
echo "OK: no stack-frame overflow reported (default build)."

if [ "${DEFAULT_ONLY:-0}" != 1 ]; then
  scripts/build-test-so.sh
fi
