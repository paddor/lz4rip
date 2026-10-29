#!/bin/sh
set -e
cd "$(dirname "$0")"

# The npm package uses web bindings; JSR uses native WASM imports.
BINDGEN_TARGET=${1:-bundler}
PKG=${2:-src/pkg}
TMP=src/pkg-tmp

rm -rf "$PKG" "$TMP"
mkdir -p "$PKG"

echo "==> Building WASM..."
cd wasm
wasm-pack build --target "$BINDGEN_TARGET" --release --out-dir "../$TMP"
cd ..

cp "$TMP/"*.js "$PKG/"
cp "$TMP/lz4rip_wasm.d.ts" "$PKG/"
cp "$TMP/lz4rip_wasm_bg.wasm.d.ts" "$PKG/"
mv "$TMP/lz4rip_wasm_bg.wasm" "$PKG/"

rm -rf "$TMP"

WASM_SIZE=$(wc -c < "$PKG/lz4rip_wasm_bg.wasm")
echo "==> Done. ${WASM_SIZE} bytes"
