#!/usr/bin/env bash
# Build every signer in this directory to `wasm32-unknown-unknown` release
# mode, then copy the resulting `.wasm` next to its `Cargo.toml` so the
# integration tests + manual workspace deploys can find it under a
# predictable name.
#
# Usage: ./signers/build-all.sh [signer-name ...]
set -euo pipefail

cd "$(dirname "$0")"

build_one() {
  local name=$1
  if [[ ! -f "$name/Cargo.toml" ]]; then
    echo "skip: $name (no Cargo.toml)" >&2
    return
  fi
  echo "Building $name..." >&2
  cargo build --target wasm32-unknown-unknown --release --manifest-path "$name/Cargo.toml"
  # cargo flattens hyphens to underscores for the artifact basename.
  local artifact_name="${name//-/_}.wasm"
  local out="$name/target/wasm32-unknown-unknown/release/$artifact_name"
  if [[ ! -f "$out" ]]; then
    echo "error: expected output not found: $out" >&2
    return 1
  fi
  cp "$out" "$name/$name.wasm"
  echo "  → $name/$name.wasm ($(wc -c < "$name/$name.wasm") bytes)" >&2
}

if [[ $# -gt 0 ]]; then
  for n in "$@"; do build_one "$n"; done
else
  for d in */; do build_one "${d%/}"; done
fi
