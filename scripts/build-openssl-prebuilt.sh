#!/usr/bin/env bash
# build-openssl-prebuilt.sh: build the shared prebuilt OpenSSL that Lucidos
# agent builds link instead of vendoring it per worktree (ADR 0392).
#
# Usage:
#   scripts/build-openssl-prebuilt.sh <openssl-src version> <features> <destination dir>
#
# The engine runs this once per version, and names every argument
# (`runtime::compile_mode`). `<features>` is the comma-separated openssl-src
# feature list openssl-sys asks for. It builds a throwaway crate whose build
# script calls `openssl_src::Build` exactly as openssl-sys does when vendoring,
# at the same pinned version and features. The result is the same static
# OpenSSL a vendored build compiles, made once instead of once per worktree.
#
# The destination appears whole or not at all: the build goes to a sibling
# temp dir, which is renamed into place with a `.complete` marker inside.
# A lock dir stops two seeds of one version running at once.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

USAGE="usage: build-openssl-prebuilt.sh <openssl-src version> <features> <destination dir>"
VERSION="${1:?$USAGE}"
FEATURES="${2?$USAGE}"
DEST="${3:?$USAGE}"
FEATURES_TOML="$(printf '%s' "$FEATURES" | awk -F, '{ for (i = 1; i <= NF; i++) printf "%s\"%s\"", (i > 1 ? ", " : ""), $i }')"

if [ -f "$DEST/.complete" ]; then
    exit 0
fi

mkdir -p "$(dirname "$DEST")"
LOCK="$DEST.lock"
if ! mkdir "$LOCK" 2>/dev/null; then
    # A seed killed mid-run leaves its lock. Past an hour it is stale.
    if [ -n "$(find "$LOCK" -maxdepth 0 -mmin +60 2>/dev/null)" ]; then
        rm -rf "$LOCK"
        mkdir "$LOCK"
    else
        echo "build-openssl-prebuilt.sh: another seed of $VERSION is running" >&2
        exit 0
    fi
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/openssl-prebuilt.XXXXXX")"
STAGE="$DEST.tmp.$$"
cleanup() { rm -rf "$WORK" "$STAGE" "$LOCK"; }
trap cleanup EXIT

mkdir -p "$WORK/crate/src"
cat > "$WORK/crate/Cargo.toml" <<EOF
[package]
name = "lucidos-openssl-prebuilt"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[build-dependencies]
openssl-src = { version = "=$VERSION", features = [$FEATURES_TOML] }
EOF
: > "$WORK/crate/src/lib.rs"
cat > "$WORK/crate/build.rs" <<'EOF'
fn main() {
    let artifacts = openssl_src::Build::new().build();
    let root = artifacts.lib_dir().parent().expect("lib dir has a parent");
    let out = std::env::var("LUCIDOS_OPENSSL_ROOT_FILE").expect("root file is named");
    std::fs::write(out, root.to_string_lossy().as_bytes()).expect("root file is writable");
}
EOF

ROOT_FILE="$WORK/root"
(
    cd "$WORK/crate"
    LUCIDOS_OPENSSL_ROOT_FILE="$ROOT_FILE" CARGO_TARGET_DIR="$WORK/target" \
        "$SCRIPT_DIR/with-build-slot.sh" --label "prebuilt OpenSSL $VERSION" -- \
        cargo build --quiet
)

ROOT="$(cat "$ROOT_FILE")"
mkdir -p "$STAGE"
cp -R "$ROOT/include" "$ROOT/lib" "$STAGE/"
printf '%s\n' "$VERSION" > "$STAGE/.complete"
rm -rf "$DEST"
mv "$STAGE" "$DEST"
echo "build-openssl-prebuilt.sh: OpenSSL $VERSION ready in $DEST"
