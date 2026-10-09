#!/usr/bin/env bash
# Fails if a staged file looks like a keypair JSON, a .env, or contains a base58 secret-key array.
set -euo pipefail
bad=0
for f in $(git diff --cached --name-only --diff-filter=ACM); do
  case "$f" in
    *.env|.env*|*id.json|*keypair*.json) echo "BLOCKED: $f"; bad=1; continue;;
  esac
  if [ -f "$f" ] && grep -Eq '^\[([0-9]{1,3},){63}[0-9]{1,3}\]$' "$f"; then
    echo "BLOCKED: $f looks like a 64-byte keypair array"; bad=1
  fi
done
exit $bad
