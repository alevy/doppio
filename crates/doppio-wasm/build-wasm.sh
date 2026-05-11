#!/usr/bin/env bash
# Build script for the doppio-wasm crate.
#
# Produces two output directories:
#   pkg/      -- wasm-bindgen output targeting the web (ESM, for PR2 / browser use)
#   pkg-node/ -- wasm-bindgen output targeting Node.js (CJS, for the smoke test)
#
# Usage (from repo root or crate directory):
#   bash crates/doppio-wasm/build-wasm.sh
#
# Requirements:
#   - Rust toolchain with wasm32-unknown-unknown target installed
#   - wasm-bindgen-cli matching the crate's wasm-bindgen version (0.2.117)
#     Install via: nix shell nixpkgs#wasm-bindgen-cli
#     or: cargo install wasm-bindgen-cli --version 0.2.117

set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

echo "==> Building doppio-wasm (release, wasm32-unknown-unknown)..."
cargo build --target wasm32-unknown-unknown -p doppio-wasm --release \
    --manifest-path "$REPO_ROOT/Cargo.toml"

WASM_FILE="$REPO_ROOT/target/wasm32-unknown-unknown/release/doppio_wasm.wasm"

echo "==> Running wasm-bindgen (target: web -> pkg/)..."
wasm-bindgen --target web --out-dir "$CRATE_DIR/pkg" "$WASM_FILE"

echo "==> Running wasm-bindgen (target: nodejs -> pkg-node/)..."
wasm-bindgen --target nodejs --out-dir "$CRATE_DIR/pkg-node" "$WASM_FILE"

echo ""
echo "==> Output sizes:"
WASM_RAW=$(wc -c < "$CRATE_DIR/pkg/doppio_wasm_bg.wasm")
WASM_GZ=$(gzip -c "$CRATE_DIR/pkg/doppio_wasm_bg.wasm" | wc -c)
echo "  Raw:   $(numfmt --to=iec $WASM_RAW)B  ($WASM_RAW bytes)"
echo "  gzip:  $(numfmt --to=iec $WASM_GZ)B  ($WASM_GZ bytes)"

if command -v brotli >/dev/null 2>&1; then
    WASM_BR=$(brotli -c "$CRATE_DIR/pkg/doppio_wasm_bg.wasm" | wc -c)
    echo "  brotli: $(numfmt --to=iec $WASM_BR)B  ($WASM_BR bytes)"
fi

echo ""
echo "pkg/ contents (web target, for browser / PR2):"
ls -lh "$CRATE_DIR/pkg/"
echo ""
echo "pkg-node/ contents (nodejs target, for smoke test):"
ls -lh "$CRATE_DIR/pkg-node/"
