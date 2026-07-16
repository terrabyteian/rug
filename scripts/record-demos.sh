#!/usr/bin/env bash
#
# record-demos.sh — re-record every README demo GIF from the tapes in tapes/.
#
# Prerequisites: vhs (brew install vhs), a terraform/tofu binary on PATH,
# and docker NOT required (the tapes avoid the MinIO-backed fixture).
#
# Tapes are recorded read-only first, then the ones that touch fixture
# state (apply, cancel), with a fixture reset between each of those so
# every take starts from the committed baseline.

set -euo pipefail
cd "$(dirname "$0")/.."

command -v vhs >/dev/null 2>&1 || {
  echo "error: vhs not found — install it with: brew install vhs" >&2
  exit 1
}

READ_ONLY=(demo-headless demo-select demo-explorer)
MUTATING=(demo-hero demo-run demo-cached-plan demo-targeted)

echo "==> building release binary"
cargo build --release

echo "==> resetting fixtures"
./fixtures/reset.sh --keep-init

echo "==> initialising fixture modules"
# big-state and remote-state/demo fail here by design (no module sources /
# no MinIO); every other module must init cleanly.
./target/release/rug --dir fixtures/ init --all >/dev/null 2>&1 || true

for tape in "${READ_ONLY[@]}"; do
  echo "==> recording $tape"
  vhs "tapes/$tape.tape"
done

for tape in "${MUTATING[@]}"; do
  echo "==> recording $tape"
  vhs "tapes/$tape.tape"
  ./fixtures/reset.sh --keep-init
done

echo "==> GIF sizes"
status=0
for gif in docs/demo-*.gif; do
  size=$(stat -f%z "$gif" 2>/dev/null || stat -c%s "$gif")
  awk -v f="$gif" -v s="$size" 'BEGIN { printf "  %-28s %.2f MB\n", f, s / 1048576 }'
  if [ "$size" -gt 2097152 ]; then
    echo "  WARNING: $gif exceeds 2 MB" >&2
    status=1
  fi
done
exit "$status"
