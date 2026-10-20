#!/usr/bin/env bash
# scripts/fetch-test-roms.sh
#
# Download the public NES accuracy test ROM collection used by the opt-in
# `nessie-core` blargg suite (`crates/nessie-core/tests/blargg.rs`).
#
# The ROMs (blargg's instr_test-v5, ppu_vbl_nmi, apu_test, mmc3_test_2, …)
# come from https://github.com/christopherpow/nes-test-roms, pinned to a
# fixed commit so results are reproducible. They are fetched rather than
# committed because the collection carries no explicit licence.
#
# Usage:
#   scripts/fetch-test-roms.sh            # → target/nes-test-roms
#   scripts/fetch-test-roms.sh /some/dir  # custom destination
#
# Then run the suite:
#   cargo test -p nessie-core --test blargg -- --ignored
# (set NES_TEST_ROMS=/some/dir when using a custom destination).

set -euo pipefail

REPO_URL="https://github.com/christopherpow/nes-test-roms.git"
COMMIT="95d8f621ae55cee0d09b91519a8989ae0e64753b"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${1:-$ROOT/target/nes-test-roms}"

if [ -d "$DEST/.git" ] && [ "$(git -C "$DEST" rev-parse HEAD 2>/dev/null)" = "$COMMIT" ]; then
  echo "nes-test-roms already at $COMMIT in $DEST"
  exit 0
fi

rm -rf "$DEST"
mkdir -p "$DEST"
git -C "$DEST" init --quiet
git -C "$DEST" remote add origin "$REPO_URL"
git -C "$DEST" fetch --quiet --depth 1 origin "$COMMIT"
git -C "$DEST" checkout --quiet FETCH_HEAD
echo "Fetched nes-test-roms@$COMMIT into $DEST"
