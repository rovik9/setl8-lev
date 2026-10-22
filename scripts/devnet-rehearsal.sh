#!/usr/bin/env bash
# DEVNET REHEARSAL of setl8-lev-trading with the real core vault. Devnet ONLY (plus a local validator for the selftest).
#
#   scripts/devnet-rehearsal.sh --cluster devnet <command> [args]
#   scripts/devnet-rehearsal.sh selftest [start|run|stop|all|measure]      # a LOCAL solana-test-validator, no SOL needed
#
# commands (devnet):
#   keys    generate the throwaway keys into ~/.setl8-devnet-lev (chmod 700), print their PUBLIC keys
#   build   build the two programs in scratch trees under ~/.setl8-devnet-lev (throwaway program ids, localnet feature, throwaway
#           ADMIN keys), the driver (tools/devnet-rehearsal) and the public verifier; prints the sha256 of everything
#   needs   print which keys still need how much SOL, and exit 3 if any
#   fund    airdrop devnet SOL to the throwaway keys (the public faucet is rate limited: it retries, then says how much is missing)
#   deploy  deploy both programs with `solana program deploy` (skips a program that is already deployed)
#   run     run the scenarios S0..S5 (idempotent and resumable: `run` again after the 1-day delay finishes the day-2 part)
#   status  print the state (public data only)
#   all     keys, build, fund, deploy, run
#
# Safety: refuses any cluster but `devnet`, any RPC URL that does not contain "devnet" or does contain "mainnet", and any node whose
# genesis hash is not devnet's. Every solana command passes an explicit --url and --keypair; your own solana config and wallet are never
# used. All keys are THROWAWAY keys generated into ~/.setl8-devnet-lev, outside every repo. The program is the LOCALNET build (mock proofs
# accepted, admin keys compiled in) re-keyed in a scratch copy with a throwaway program id AND throwaway admin keys: acceptable ONLY on
# devnet. The real-key default build is never deployed here (the real admin keys are not on this machine).
set -euo pipefail

cd "$(dirname "$0")/.."
REPO="$(pwd)"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"

D="${SETL8_DEVNET_LEV_DIR:-$HOME/.setl8-devnet-lev}"
VAULT_REPO="${SETL8_VAULT_REPO:-$HOME/Documents/setl8-vault}"
DEVNET_GENESIS="EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG"
TOOL_DIR="$REPO/tools/devnet-rehearsal"
TOOL="$TOOL_DIR/target/release/devnet-rehearsal"
VERIFY_BIN="$REPO/target/release/setl8-verify"
BV="$D/build-vault"
BL="$D/build-lev"
VAULT_SO="$BV/out/core_vault.so"
LEV_SO="$BL/out/setl8_lev_trading.so"
FROZEN_LOCALNET_SHA="43fdd9356c774ea343686e5f6bf781b998a01670664c2af6b8233bf97f22bc1b"

die() { echo "$*" >&2; exit 2; }

mode="devnet"
if [ "${1:-}" = "selftest" ]; then
  mode="selftest"
  SUB="${2:-all}"
  shift 2 || true
else
  [ "${1:-}" = "--cluster" ] && [ "${2:-}" = "devnet" ] || die "usage: $0 --cluster devnet <keys|build|needs|fund|deploy|run|status|all>   (devnet is the only cluster this script accepts)   |   $0 selftest [start|run|stop|all|measure]"
  CMD="${3:-}"
  shift 3 || true
fi

if [ "$mode" = "devnet" ]; then
  URL="${DEVNET_RPC_URL:-https://api.devnet.solana.com}"
  case "$URL" in
    *mainnet*) die "refusing: the RPC URL contains 'mainnet'" ;;
    *devnet*) ;;
    *) die "refusing: the RPC URL must contain 'devnet'" ;;
  esac
  CLUSTER="devnet"
  WORK="$D/work-devnet"
else
  URL="http://127.0.0.1:${SELFTEST_PORT:-8899}"
  CLUSTER="localnet"
  WORK="$D/work-localnet"
  # the LOCAL selftest runs a scratch build whose MINIMUM pass delay is 60 s (never deployed anywhere but the local validator), so the
  # day-2 part (finalize_pass, phase 3, the third reset) can be exercised in minutes; the devnet run uses the real minimum, 1 day
  BLS="$D/build-lev-selftest"
  LEV_SO="$BLS/out/setl8_lev_trading.so"
  EXTRA_ARGS="--pass-delay-secs 90"
fi

need_keys() {
  [ -d "$D" ] || die "no $D: run the keys command first"
  [ "$(stat -f '%Lp' "$D" 2>/dev/null || stat -c '%a' "$D")" = "700" ] || die "$D must be mode 700"
}

pub() { solana-keygen pubkey "$D/$1.json"; }

genesis_of() {
  curl -s -m 30 -X POST "$URL" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getGenesisHash"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"])'
}

genesis_ok() {
  local g; g=$(genesis_of) || die "cannot reach the cluster"
  if [ "$mode" = "devnet" ]; then
    [ "$g" = "$DEVNET_GENESIS" ] || die "refusing: the node's genesis hash is $g, not devnet's"
  fi
  echo "$g"
}

tool() { # forwards the common arguments to the driver
  local g; g=$(genesis_ok)
  "$TOOL" --rpc-url "$URL" --cluster "$CLUSTER" --expect-genesis "$g" --keys-dir "$D" --work-dir "$WORK" \
    --vault-program "$(pub program-vault)" --lev-program "$(pub program-lev)" \
    --vault-so "$VAULT_SO" --lev-so "$LEV_SO" --vault-commit "$(git -C "$VAULT_REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)" \
    --verify-bin "$VERIFY_BIN" --launch "$REPO/config/launch.json" ${EXTRA_ARGS:-} "$@"
}

cmd_keys() {
  umask 077; mkdir -p "$D"; chmod 700 "$D"
  [ -x "$TOOL" ] || cargo build --release --offline --manifest-path "$TOOL_DIR/Cargo.toml"
  # the driver writes missing keypair files itself and prints PUBLIC keys only
  "$TOOL" --rpc-url http://127.0.0.1:1 --cluster localnet --expect-genesis x --keys-dir "$D" --work-dir "$D/none" keys
}

# the pubkey literal of the constant NAME that sits on the line right after a `#[cfg(feature = "localnet")]` attribute
localnet_const() { # file name
  awk -v name="$2" '/^#\[cfg\(feature = "localnet"\)\]/{getline nxt; if (nxt ~ name) {print nxt; exit}}' "$1" | grep -o '"[A-Za-z0-9]*"' | tr -d '"'
}

swap_in() { # file old new
  sed -i.bak "s/$2/$3/g" "$1" && rm -f "$1.bak"
}

cmd_build() {
  need_keys
  [ -d "$VAULT_REPO/programs/core-vault" ] || die "the vault repo is not at $VAULT_REPO (read-only reference)"
  echo "== vault scratch build (vault HEAD $(git -C "$VAULT_REPO" rev-parse --short HEAD), throwaway id, throwaway admin keys)"
  rm -rf "$BV"; mkdir -p "$BV"
  (cd "$VAULT_REPO" && rsync -a --exclude target programs Cargo.toml Cargo.lock Anchor.toml "$BV/")
  local old_id; old_id=$(grep -o 'declare_id!("[A-Za-z0-9]*")' "$VAULT_REPO/programs/core-vault/src/lib.rs" | sed 's/declare_id!("\(.*\)")/\1/')
  local old_sl8 old_rov
  old_sl8=$(localnet_const "$VAULT_REPO/programs/core-vault/src/constants/admin.rs" SL8_ADMIN_PUBKEY)
  old_rov=$(localnet_const "$VAULT_REPO/programs/core-vault/src/constants/admin.rs" ROV_ADMIN_PUBKEY)
  [ -n "$old_id" ] && [ -n "$old_sl8" ] && [ -n "$old_rov" ] || die "cannot find the vault's id and localnet admin keys"
  swap_in "$BV/programs/core-vault/src/lib.rs" "$old_id" "$(pub program-vault)"
  swap_in "$BV/Anchor.toml" "$old_id" "$(pub program-vault)"
  swap_in "$BV/programs/core-vault/src/constants/admin.rs" "$old_sl8" "$(pub vault-admin-sl8)"
  swap_in "$BV/programs/core-vault/src/constants/admin.rs" "$old_rov" "$(pub vault-admin-rov)"
  (cd "$BV/programs/core-vault" && cargo build-sbf --offline --features localnet --sbf-out-dir "$BV/out" -- --locked) 2>&1 | tee "$BV/build.log" | tail -3
  if grep -qE 'overflows the maximum allowed frame|Stack offset' "$BV/build.log"; then die "ERROR: stack-frame overflow in the vault build"; fi
  rm -f "$BV/out/core_vault-keypair.json"
  echo "vault .so ($(pub program-vault)) sha256: $(shasum -a 256 "$VAULT_SO" | cut -d' ' -f1)  size $(wc -c <"$VAULT_SO" | tr -d ' ')"

  echo "== lev scratch build (the frozen candidate; throwaway id and admin keys)"
  rm -rf "$BL"; mkdir -p "$BL"
  rsync -a --exclude target --exclude tests programs crates Cargo.toml Cargo.lock Anchor.toml rust-toolchain.toml "$BL/"
  # 1. the scratch tree WITHOUT the swaps must give the frozen hash (proves the toolchain and the tree)
  (cd "$BL/programs/setl8-lev-trading" && cargo build-sbf --offline --features localnet --sbf-out-dir "$BL/out-plain" -- --locked) >"$BL/build-plain.log" 2>&1
  local plain; plain=$(shasum -a 256 "$BL/out-plain/setl8_lev_trading.so" | cut -d' ' -f1)
  if [ "$plain" = "$FROZEN_LOCALNET_SHA" ]; then echo "scratch build without swaps = the freeze note's localnet hash ($plain)"; else echo "WARNING: scratch build without swaps is $plain, the freeze note says $FROZEN_LOCALNET_SHA" >&2; fi
  # 2. the swaps: program id and the two admin keys, in the scratch tree only
  local lev_id lev_sl8 lev_rov
  lev_id=$(grep -o 'declare_id!("[A-Za-z0-9]*")' programs/setl8-lev-trading/src/lib.rs | sed 's/declare_id!("\(.*\)")/\1/')
  lev_sl8=$(localnet_const programs/setl8-lev-trading/src/constants/admin.rs SL8_ADMIN)
  lev_rov=$(localnet_const programs/setl8-lev-trading/src/constants/admin.rs ROV_ADMIN)
  [ -n "$lev_id" ] && [ -n "$lev_sl8" ] && [ -n "$lev_rov" ] || die "cannot find this program's id and localnet admin keys"
  swap_in "$BL/programs/setl8-lev-trading/src/lib.rs" "$lev_id" "$(pub program-lev)"
  swap_in "$BL/Anchor.toml" "$lev_id" "$(pub program-lev)"
  swap_in "$BL/programs/setl8-lev-trading/src/constants/admin.rs" "$lev_sl8" "$(pub lev-admin-sl8)"
  swap_in "$BL/programs/setl8-lev-trading/src/constants/admin.rs" "$lev_rov" "$(pub lev-admin-rov)"
  (cd "$BL/programs/setl8-lev-trading" && cargo build-sbf --offline --features localnet --sbf-out-dir "$BL/out" -- --locked) 2>&1 | tee "$BL/build.log" | tail -3
  if grep -qE 'overflows the maximum allowed frame|Stack offset' "$BL/build.log"; then die "ERROR: stack-frame overflow in the program build"; fi
  rm -f "$BL/out/setl8_lev_trading-keypair.json"
  echo "lev .so ($(pub program-lev)) sha256: $(shasum -a 256 "$LEV_SO" | cut -d' ' -f1)  size $(wc -c <"$LEV_SO" | tr -d ' ')  (frozen candidate re-keyed; plain size $(wc -c <"$BL/out-plain/setl8_lev_trading.so" | tr -d ' '))"

  echo "== the driver and the public verifier"
  cargo build --release --offline --manifest-path "$TOOL_DIR/Cargo.toml"
  cargo build --release --offline --locked -p setl8-lev-verifier
  echo "driver sha256: $(shasum -a 256 "$TOOL" | cut -d' ' -f1)"
}

deploy_one() { # name so
  local name=$1 so=$2 pid; pid=$(pub "$name")
  if solana program show "$pid" --url "$URL" --keypair "$D/upgrade-authority.json" >/dev/null 2>&1; then echo "$name $pid already deployed"; return; fi
  solana program deploy "$so" --program-id "$D/$name.json" --keypair "$D/upgrade-authority.json" \
    --upgrade-authority "$D/upgrade-authority.json" --max-len "$(wc -c <"$so" | tr -d ' ')" --url "$URL"
  echo "$name $pid deployed; .so sha256 $(shasum -a 256 "$so" | cut -d' ' -f1)"
}

cmd_deploy() {
  need_keys; genesis_ok >/dev/null
  [ -f "$VAULT_SO" ] && [ -f "$LEV_SO" ] || die "run 'build' first"
  deploy_one program-vault "$VAULT_SO"
  deploy_one program-lev "$LEV_SO"
}

if [ "$mode" = "devnet" ]; then
  case "$CMD" in
    keys) cmd_keys ;;
    build) cmd_build ;;
    needs) need_keys; tool needs ;;
    fund) need_keys; tool fund ;;
    deploy) cmd_deploy ;;
    run) need_keys; tool run "$@" ;;
    status) need_keys; tool status ;;
    all) cmd_keys; cmd_build; tool fund; cmd_deploy; tool run "$@" ;;
    *) die "unknown command '$CMD'" ;;
  esac
  exit 0
fi

# ------------------------------------------------------------------------------------------------ selftest (local validator)
LEDGER="$D/ledger-local"
PIDFILE="$D/validator.pid"
VLOG="$D/validator.log"

build_selftest_so() {
  [ -f "$LEV_SO" ] && return 0
  [ -d "$BL/programs" ] || die "run '--cluster devnet build' first"
  rm -rf "$BLS"; mkdir -p "$BLS"
  rsync -a --exclude target --exclude out --exclude out-plain "$BL/" "$BLS/"
  sed -i.bak 's/pub const MIN_PASS_DELAY_SECS: u32 = 86_400;/pub const MIN_PASS_DELAY_SECS: u32 = 60;/' "$BLS/programs/setl8-lev-trading/src/constants/mod.rs" && rm -f "$BLS/programs/setl8-lev-trading/src/constants/mod.rs.bak"
  grep -q 'MIN_PASS_DELAY_SECS: u32 = 60;' "$BLS/programs/setl8-lev-trading/src/constants/mod.rs" || die "cannot set the selftest minimum delay"
  # the compute measurement (P27) also needs the activity throttle, the cancel delay and the terminal retention short; they change no code path
  local cm="$BLS/programs/setl8-lev-trading/src/constants/mod.rs"
  sed -i.bak 's/pub const ACTIVITY_THROTTLE_SECS: i64 = 86_000;/pub const ACTIVITY_THROTTLE_SECS: i64 = 5;/; s/pub const CANCEL_PASS_AFTER_SECS: i64 = 3_600;/pub const CANCEL_PASS_AFTER_SECS: i64 = 5;/; s/pub const TERMINAL_RETENTION_SECS: i64 = 604_800;/pub const TERMINAL_RETENTION_SECS: i64 = 5;/' "$cm" && rm -f "$cm.bak"
  grep -q 'ACTIVITY_THROTTLE_SECS: i64 = 5;' "$cm" && grep -q 'CANCEL_PASS_AFTER_SECS: i64 = 5;' "$cm" && grep -q 'TERMINAL_RETENTION_SECS: i64 = 5;' "$cm" || die "cannot shorten the selftest constants"
  (cd "$BLS/programs/setl8-lev-trading" && cargo build-sbf --offline --features localnet --sbf-out-dir "$BLS/out" -- --locked) >"$BLS/build.log" 2>&1
  rm -f "$BLS/out/setl8_lev_trading-keypair.json"
  echo "selftest-only program build (minimum pass delay 60 s) sha256: $(shasum -a 256 "$LEV_SO" | cut -d' ' -f1)"
}

st_start() {
  need_keys
  [ -f "$VAULT_SO" ] || die "run '--cluster devnet build' first (the same scratch builds are used)"
  build_selftest_so
  [ -x "$TOOL" ] || die "the driver is not built"
  if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then echo "validator already running ($(cat "$PIDFILE"))"; else
    rm -rf "$LEDGER" "$WORK"
    nohup solana-test-validator --reset --quiet --limit-ledger-size 500000000 --ledger "$LEDGER" --rpc-port "${SELFTEST_PORT:-8899}" ${SELFTEST_TICKS:+--ticks-per-slot "$SELFTEST_TICKS"} >"$VLOG" 2>&1 &
    echo $! >"$PIDFILE"
    for _ in $(seq 1 90); do
      curl -s -m 2 -X POST "$URL" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q ok && break
      sleep 1
    done
  fi
  mode_dev="$mode"
  # fund and deploy exactly as on devnet (the validator's faucet gives any amount)
  tool fund
  deploy_one program-vault "$VAULT_SO"
  deploy_one program-lev "$LEV_SO"
}

st_stop() { if [ -f "$PIDFILE" ]; then kill "$(cat "$PIDFILE")" 2>/dev/null || true; rm -f "$PIDFILE"; fi; }

case "$SUB" in
  start) st_start ;;
  run) need_keys; tool run "$@" ;;
  stop) st_stop ;;
  all) trap st_stop EXIT; st_start; tool run "$@" ;;
  measure)
    # P27: the worst-case shape of every instruction on the local validator -> handover/cu-runtime.json and .md
    trap st_stop EXIT
    st_start
    { solana-test-validator --version | head -1; solana feature status --url "$URL" --keypair "$D/keeper.json" 2>&1; } >"$D/validator-info.txt"
    tool measure --out-dir "$REPO/handover" --validator-info "$D/validator-info.txt" "$@"
    ;;
  *) die "unknown selftest command '$SUB'" ;;
esac
