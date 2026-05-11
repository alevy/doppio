#!/usr/bin/env bash
# Run the Node.js smoke test for the doppio-wasm crate.
#
# Builds the wasm binary (if not already built), generates the nodejs binding,
# then executes the smoke test.
#
# Usage (from repo root or crate directory):
#   bash crates/doppio-wasm/test-smoke.sh
#
# Requirements: same as build-wasm.sh (see that file for details).

set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

# Build if pkg-node/ doesn't exist or is stale relative to the source.
if [ ! -f "$CRATE_DIR/pkg-node/doppio_wasm.js" ]; then
    echo "==> pkg-node/ not found; running build-wasm.sh first..."
    bash "$CRATE_DIR/build-wasm.sh"
fi

echo "==> Running smoke test..."
node "$CRATE_DIR/tests/smoke.mjs"
