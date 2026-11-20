#!/usr/bin/env bash
# Builds the npm package `remarkable-annotations` (wasm-pack, ES module for
# browsers, workers and privileged Firefox/Zotero code) into OUT_DIR
# (default: wasm/pkg). Used by .github/workflows/release-npm.yml.
#
#   wasm/build-npm.sh [OUT_DIR]
#   (cd wasm/pkg && npm publish --dry-run)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(mkdir -p "${1:-$here/pkg}" && cd "${1:-$here/pkg}" && pwd)

wasm-pack build --release --target web --out-dir "$out" "$here"

cp "$here/../NOTICE" "$here/../LICENSE-MIT" "$here/../LICENSE-APACHE" "$out/"
cp "$here/README.md" "$out/"
cd "$out"
# wasm-pack names the package after the crate (remarkable-annotations-wasm)
npm pkg set name=remarkable-annotations
npm pkg set 'files[]=NOTICE' 'files[]=LICENSE-MIT' 'files[]=LICENSE-APACHE'
npm pkg set description='Read and write reMarkable annotations (.rm v6 scenes) in WebAssembly'
npm pkg set 'keywords[]=remarkable' 'keywords[]=annotations' 'keywords[]=pdf' 'keywords[]=webassembly'
npm pkg get name version
