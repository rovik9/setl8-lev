#!/usr/bin/env bash
# P30 gate: the files shown to the examiner describe lev trading only. Prints the grep result; exits 1 on any hit.
set -u
cd "$(dirname "$0")/.."
files="handover/devnet-demo.md handover/devnet-deployment.md handover/devnet-deployment.json"
if grep -inE 'option|predict|hft|simfunded|sector 2|roadmap' $files; then
  echo "SCOPE CHECK: FAIL (the lines above name something other than lev trading)"
  exit 1
fi
echo "SCOPE CHECK: PASS (no match for option, predict, hft, simfunded, sector 2, roadmap in: $files)"
