#!/bin/sh
# Builds the WebAssembly bindings for Node and checks them against the
# fixtures. Needs node and wasm-pack (which fetches the wasm-bindgen
# matching Cargo.lock).
set -e
cd "$(dirname "$0")/../.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
wasm-pack --quiet build --release --target nodejs --out-dir "$tmp/pkg" wasm
node wasm/tests/check.mjs "$tmp/pkg"
