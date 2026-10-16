#!/usr/bin/env bash
# Tests for scripts/lib/codegen.sh: the codegen fingerprint, the cache, the
# copy that writes only changed bytes, and the dep-info check.
#
# Hermetic: every case runs in a scratch git repo under $TMP. A stub stands in
# for the lucidos-codegen bin, and codegen_build_sdk is replaced, so no cargo,
# node or npm runs.
#
# Run: ./scripts/lib/codegen_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/codegen.sh
source "$SCRIPT_DIR/codegen.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
expect_eq() { # <name> <expected> <actual>
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi
}
expect_ne() { # <name> <unexpected> <actual>
    if [ "$2" != "$3" ]; then pass "$1"; else fail "$1: both are [$3]"; fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# The SDK step needs node. Record that it ran instead.
codegen_build_sdk() { echo sdk >> "$TMP/sdk-log"; }

WIRE=crates/lucidos-app/src/generated/thread-event-wire.ts

new_repo() { # <dir>
    git init -q -b main "$1"
    mkdir -p "$1/crates/lucidos-engine/src" "$1/crates/lucidos-app/src" "$1/scripts"
    echo 'pub const A: u32 = 1;' > "$1/crates/lucidos-engine/src/lib.rs"
    echo '# lock' > "$1/Cargo.lock"
    echo 'export {};' > "$1/crates/lucidos-app/src/main.ts"
    git -C "$1" add -A
    git -C "$1" -c user.email=t@example.com -c user.name=t commit -q -m base
}

# A stand-in for lucidos-codegen: writes one generated file whose content
# follows the engine source, and a dep-info file listing <deps>.
new_stub_bin() { # <repo> <dep>...
    local repo=$1 bin=$TMP/stub-codegen
    shift
    cat > "$bin" <<'STUB'
#!/bin/bash
while [ $# -gt 0 ]; do
    case $1 in --out) out=$2; shift 2 ;; --repo-root) root=$2; shift 2 ;; *) exit 2 ;; esac
done
mkdir -p "$out/crates/lucidos-app/src/generated"
cp "$root/crates/lucidos-engine/src/lib.rs" "$out/crates/lucidos-app/src/generated/thread-event-wire.ts"
STUB
    chmod +x "$bin"
    {
        printf '%s:' "$repo/target/debug/lucidos-codegen"
        printf ' %s' "$@"
        echo
    } > "$bin.d"
    echo "$bin"
}

echo "fingerprint"
R="$TMP/repo"
new_repo "$R"
expect_eq "a clean tree has its commit's fingerprint" \
    "$(codegen_fingerprint "$R" HEAD)" "$(codegen_fingerprint "$R")"
clean=$(codegen_fingerprint "$R")
echo 'changed' > "$R/crates/lucidos-app/src/main.ts"
expect_eq "a change outside the inputs keeps it" "$clean" "$(codegen_fingerprint "$R")"
echo 'pub const A: u32 = 2;' > "$R/crates/lucidos-engine/src/lib.rs"
edited=$(codegen_fingerprint "$R")
expect_ne "an edited input changes it" "$clean" "$edited"
git -C "$R" add -A
expect_eq "staging an edit keeps it" "$edited" "$(codegen_fingerprint "$R")"
echo 'pub const A: u32 = 1;' > "$R/crates/lucidos-engine/src/lib.rs"
expect_eq "undoing the edit in the working tree restores it" "$clean" "$(codegen_fingerprint "$R")"
git -C "$R" reset -q --hard
echo 'pub fn b() {}' > "$R/crates/lucidos-engine/src/new.rs"
expect_ne "an untracked input changes it" "$clean" "$(codegen_fingerprint "$R")"
rm "$R/crates/lucidos-engine/src/new.rs"
rm "$R/Cargo.lock"
expect_ne "a deleted input changes it" "$clean" "$(codegen_fingerprint "$R")"
git -C "$R" checkout -q -- Cargo.lock
expect_eq "the tree is back to its commit" "$clean" "$(codegen_fingerprint "$R")"

echo "render, cache and copy"
BIN=$(new_stub_bin "$R" "$R/crates/lucidos-engine/src/lib.rs" "$R/Cargo.lock")
: > "$TMP/sdk-log"
expect_eq "a miss renders and succeeds" "0" "$(codegen_main "$R" --bin "$BIN" 2>/dev/null; echo $?)"
expect_eq "the rendering reached the tree" "pub const A: u32 = 1;" "$(cat "$R/$WIRE")"
expect_eq "the SDK built after it" "sdk" "$(cat "$TMP/sdk-log")"
cached=$(codegen_cached "$R" "$clean")
expect_eq "the rendering is cached under the fingerprint" "pub const A: u32 = 1;" "$(cat "$cached/$WIRE" 2>/dev/null)"
touch -t 202001010000 "$R/$WIRE"
before=$(ls -l -T "$R/$WIRE" 2>/dev/null || stat -c %y "$R/$WIRE")
codegen_main "$R" --cached-only 2>/dev/null
after=$(ls -l -T "$R/$WIRE" 2>/dev/null || stat -c %y "$R/$WIRE")
expect_eq "a hit leaves an unchanged file's mtime alone" "$before" "$after"
echo 'stale' > "$R/$WIRE"
codegen_main "$R" --cached-only 2>/dev/null
expect_eq "a hit rewrites a changed file" "pub const A: u32 = 1;" "$(cat "$R/$WIRE")"

echo "cached-only on a miss"
echo 'pub const A: u32 = 3;' > "$R/crates/lucidos-engine/src/lib.rs"
expect_eq "a miss with outputs present keeps them and succeeds" "0" \
    "$(codegen_main "$R" --cached-only 2>/dev/null; echo $?)"
expect_eq "the kept output is untouched" "pub const A: u32 = 1;" "$(cat "$R/$WIRE")"
rm "$R/$WIRE"
expect_eq "a miss with no outputs fails" "1" "$(codegen_main "$R" --cached-only 2>/dev/null; echo $?)"
git -C "$R" reset -q --hard

echo "dep-info check"
expect_eq "declared inputs pass" "" \
    "$(codegen_undeclared_inputs "$R" "$BIN.d")"
printf '%s: %s %s %s %s\n' "$R/target/debug/x" \
    "$R/crates/lucidos-engine/src/../../lucidos-app/src/styles/global/surface.css" \
    "$R/target/debug/build/out/gen.rs" \
    "$HOME/.cargo/registry/src/serde/lib.rs" \
    "$R/crates/lucidos-engine/src/api/../../../lucidos-app/src/main.ts" > "$TMP/mixed.d"
expect_eq "only an undeclared repo file is reported, its path normalised" \
    "crates/lucidos-app/src/main.ts" "$(codegen_undeclared_inputs "$R" "$TMP/mixed.d")"
BAD=$(new_stub_bin "$R" "$R/crates/lucidos-app/src/main.ts")
echo 'pub const A: u32 = 4;' > "$R/crates/lucidos-engine/src/lib.rs"
expect_eq "a render that compiled an undeclared input fails" "1" \
    "$(codegen_main "$R" --bin "$BAD" 2>/dev/null; echo $?)"
expect_eq "and caches nothing" "" "$(codegen_cached "$R" "$(codegen_fingerprint "$R")")"
git -C "$R" reset -q --hard

printf '%s: %s %s %s\n' "$R/target/debug/x" "$R/.git/HEAD" "$R/crates/lucidos-engine/../../.git/packed-refs" \
    "crates/lucidos-app/src/main.ts" > "$TMP/rustc.d"
expect_eq "git-dir paths are skipped and a relative path is read from the root" \
    "crates/lucidos-app/src/main.ts" "$(codegen_undeclared_inputs "$R" "$TMP/rustc.d")"
touch "$TMP/test-bin"
expect_eq "freshness-only ignores coverage" "0" \
    "$(codegen_check_inputs "$R" "$TMP/rustc.d" "$TMP/test-bin" --freshness-only 2>/dev/null; echo $?)"
expect_eq "the test binary is read from cargo's output, anchored at the root" \
    "/repo/target/debug/deps/lucidos_engine-abc123" \
    "$(printf '%s\n' '   Compiling x' '     Running unittests src/lib.rs (target/debug/deps/lucidos_engine-abc123)' | codegen_test_binary /repo)"
expect_eq "an absolute test binary stays as cargo printed it" "/t/debug/deps/lucidos_engine-abc123" \
    "$(printf '%s\n' '     Running unittests src/lib.rs (/t/debug/deps/lucidos_engine-abc123)' | codegen_test_binary /repo)"

echo "a rendering already cached"
BIN=$(new_stub_bin "$R" "$R/crates/lucidos-engine/src/lib.rs")
echo 'pub const A: u32 = 6;' > "$R/crates/lucidos-engine/src/lib.rs"
fp=$(codegen_fingerprint "$R")
mkdir -p "$(codegen_cache_root "$R")/$fp/crates/lucidos-app/src/generated"
echo 'theirs' > "$(codegen_cache_root "$R")/$fp/$WIRE"
codegen_render "$R" "$fp" "$BIN" 2>/dev/null
expect_eq "a lost race leaves no rendering nested inside the winner" "" \
    "$(find "$(codegen_cache_root "$R")/$fp" -name '.rendering.*')"
expect_eq "the rendering already there stays" "theirs" "$(cat "$(codegen_cache_root "$R")/$fp/$WIRE")"
git -C "$R" reset -q --hard

echo "stale bin"
BIN=$(new_stub_bin "$R" "$R/crates/lucidos-engine/src/lib.rs")
touch -t 202001010000 "$BIN"
echo 'pub const A: u32 = 5;' > "$R/crates/lucidos-engine/src/lib.rs"
expect_eq "a bin older than an input it compiled is refused" "1" \
    "$(codegen_main "$R" --bin "$BIN" 2>/dev/null; echo $?)"
git -C "$R" reset -q --hard

echo "prune"
CACHE=$(codegen_cache_root "$R")
for i in $(seq 1 40); do mkdir -p "$CACHE/fp$i"; done
CODEGEN_CACHE_KEEP=5 codegen_prune "$CACHE"
expect_eq "the cache keeps the newest renderings" "5" "$(find "$CACHE" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')"

echo ""
echo "codegen_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
