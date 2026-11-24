#!/usr/bin/env bash
# Hermetic evidence/wipe fixtures. No hosts, user caches or real builds touched.
set -Eeuo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/scripts" "$WORK/bin" "$WORK/home" "$WORK/cache"
cp "$ROOT/scripts/block-ort-upstream.sh" "$WORK/scripts/"
cat > "$WORK/bin/id" <<'STUB'
#!/bin/sh
echo 0
STUB
cat > "$WORK/bin/tee" <<'STUB'
#!/bin/sh
cat > "$HOME/hosts.fixture"
STUB
cat > "$WORK/bin/uname" <<'STUB'
#!/bin/sh
echo "${TEST_OS:-Linux}"
STUB
chmod +x "$WORK/bin/"*
export HOME="$WORK/home" XDG_CACHE_HOME="$WORK/cache" PATH="$WORK/bin:$PATH"
unset CARGO_TARGET_DIR
script="$WORK/scripts/block-ort-upstream.sh"
expect_fail() { if bash "$script" --assert > "$WORK/log" 2>&1; then echo 'unexpected evidence pass' >&2; return 1; fi; }
mirror='fetched ONNX Runtime from the Lucidos mirror: https://example.test/archive'
upstream='fetched ONNX Runtime from upstream: https://example.test/archive'
expect_fail
for layout in target/debug target/launch-tree/target/local-release target/aarch64-unknown-linux-gnu/release; do
    output="$WORK/$layout/build/ort-sys-abc/output"
    mkdir -p "$(dirname "$output")" "$WORK/$layout/.fingerprint/ort-sys-abc" "$XDG_CACHE_HOME/dfbin"
    echo "cached without a fetch" > "$output"
    expect_fail
    echo "$mirror" > "$output"
    bash "$script" --assert
    printf '%s\n%s\n' "$mirror" "$upstream" > "$output"
    expect_fail
    echo "$mirror" > "$output"
    mkdir -p "$WORK/target/debug/build/ort-sys-other"
    echo "$upstream" > "$WORK/target/debug/build/ort-sys-other/output"
    expect_fail
    bash "$script" --wipe
    [[ ! -e "$output" && ! -d "$WORK/$layout/.fingerprint/ort-sys-abc" && ! -d "$XDG_CACHE_HOME/dfbin" ]]
    expect_fail
done
for target in "$WORK/custom-target" relative-target; do
    export CARGO_TARGET_DIR="$target"
    [[ "$target" == /* ]] || target="$WORK/$target"
    mkdir -p "$target/debug/build/ort-sys-abc" "$target/debug/.fingerprint/ort-sys-abc"
    echo "$mirror" > "$target/debug/build/ort-sys-abc/output"
    (cd "$WORK" && bash "$script" --assert && bash "$script" --wipe)
    [[ ! -d "$target/debug/build/ort-sys-abc" && ! -d "$target/debug/.fingerprint/ort-sys-abc" ]]
done
unset CARGO_TARGET_DIR
mkdir -p "$HOME/Library/Caches/ort.pyke.io/dfbin"
TEST_OS=Darwin bash "$script" --wipe
[[ ! -d "$HOME/Library/Caches/ort.pyke.io/dfbin" && -s "$HOME/hosts.fixture" ]]
echo 'PASS: ORT evidence and wipe fixtures'
