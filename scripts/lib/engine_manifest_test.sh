#!/bin/bash
# Tests for the engine input manifest and the local engine cache
# (scripts/lib/engine_manifest.sh).
#
# Hermetic. A throwaway git repo stands in for the checkout, hand-written
# dep-info files stand in for cargo's, and the cache lives in the sandbox.
#
# Run: ./scripts/lib/engine_manifest_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

unset RUSTFLAGS RUSTUP_TOOLCHAIN
export LUCIDOS_E2E_ENGINE_CACHE="$SANDBOX/cache"
# shellcheck source=scripts/lib/engine_manifest.sh
source "$SCRIPT_DIR/engine_manifest.sh"

# A repo with an engine crate, a frontend file, a spec and an ignored output,
# plus a profile dir whose dep-info names the engine's inputs the way cargo
# does: absolute paths, a `..` hop, a rerun-if-changed directory, and paths
# outside the tree.
make_checkout() {
    local repo="$1" name
    rm -rf "$repo"
    mkdir -p "$repo/crates/engine/src" "$repo/crates/engine/migrations" \
        "$repo/crates/app/src" "$repo/crates/app/e2e" "$repo/target/e2e"
    git init -q "$repo"
    echo '[workspace]' >"$repo/Cargo.toml"
    echo 'lock v1' >"$repo/Cargo.lock"
    echo 'channel = "1.0.0"' >"$repo/rust-toolchain.toml"
    echo '[package]' >"$repo/crates/engine/Cargo.toml"
    echo 'fn main() {}' >"$repo/crates/engine/build.rs"
    echo 'fn main() {}' >"$repo/crates/engine/src/main.rs"
    echo 'pub fn a() {}' >"$repo/crates/engine/src/a.rs"
    echo 'CREATE TABLE t ();' >"$repo/crates/engine/migrations/0001.sql"
    echo 'render()' >"$repo/crates/app/src/main.tsx"
    echo 'test()' >"$repo/crates/app/e2e/a.spec.ts"
    printf 'target/\ncrates/engine/VERSION\n' >"$repo/.gitignore"
    echo '2026.1.1' >"$repo/crates/engine/VERSION"
    git -C "$repo" add -A
    git -C "$repo" -c user.email=dev@example.com -c user.name=Dev commit -qm init
    for name in "${ENGINE_MANIFEST_BINARIES[@]}"; do
        echo "$name binary" >"$repo/target/e2e/$name"
        printf '%s: %s %s %s %s %s %s\n' "$repo/target/e2e/$name" \
            "$repo/crates/engine/src/main.rs" "$repo/crates/engine/src/../src/a.rs" \
            "$repo/crates/engine/migrations" "$repo/crates/engine/VERSION" \
            "/nowhere/.git/HEAD" "/home/user/.cargo/registry/src/x/lib.rs" \
            >"$repo/target/e2e/$name.d"
    done
}

REPO="$SANDBOX/repo"
MANIFEST="$SANDBOX/manifest"

fresh() {
    make_checkout "$REPO"
    engine_manifest_write "$REPO" "$REPO/target/e2e" "$MANIFEST" e2e e2e-test-hooks
}

check() { engine_manifest_check "$REPO" "$MANIFEST" e2e e2e-test-hooks 2>/dev/null; }

test_the_manifest_lists_inputs_and_pins_but_no_outputs() {
    echo "test: the manifest lists dep-info inputs and the pins, never an output or an outside path"
    fresh
    local entries
    entries="$(cut -f1,3 "$MANIFEST" | grep -v '^key')"
    local want
    for want in "file	crates/engine/src/main.rs" "file	crates/engine/src/a.rs" \
        "dir	crates/engine/migrations" "file	crates/engine/Cargo.toml" "file	crates/engine/build.rs" \
        "file	Cargo.lock" "file	Cargo.toml" "file	rust-toolchain.toml"; do
        if printf '%s\n' "$entries" | grep -qxF "$want"; then pass "lists $want"; else fail "missing $want"; fi
    done
    if printf '%s\n' "$entries" | grep -q 'VERSION\|registry\|HEAD\|main.tsx'; then
        fail "lists an output, an outside path or a frontend file: $entries"
    else
        pass "lists no output, outside path or frontend file"
    fi
}

test_an_unchanged_tree_matches() {
    echo "test: the tree the manifest was written from matches it"
    fresh
    if check; then pass "unchanged tree matches"; else fail "unchanged tree refused"; fi
}

test_a_frontend_or_spec_edit_still_matches() {
    echo "test: a frontend-only or spec-only edit keeps the engine"
    fresh
    echo 'render(2)' >"$REPO/crates/app/src/main.tsx"
    echo 'test(2)' >"$REPO/crates/app/e2e/a.spec.ts"
    echo 'test()' >"$REPO/crates/app/e2e/new.spec.ts"
    if check; then pass "frontend and spec edits match"; else fail "a frontend or spec edit refused the engine"; fi
}

expect_refused() { # <what> <expected reason substring> [check args...]
    local what="$1" want="$2" reason
    shift 2
    if reason="$(engine_manifest_check "$REPO" "$MANIFEST" e2e e2e-test-hooks "$@" 2>&1)"; then
        fail "$what was accepted"
    elif [[ "$reason" == *"$want"* ]]; then
        pass "$what refuses: $reason"
    else
        fail "$what refused for the wrong reason: $reason"
    fi
}

test_a_rust_edit_refuses() {
    echo "test: an edit to a listed file refuses"
    fresh
    echo 'pub fn a() { 1; }' >"$REPO/crates/engine/src/a.rs"
    expect_refused "an edited source file" "crates/engine/src/a.rs changed"
    fresh
    rm "$REPO/crates/engine/src/a.rs"
    expect_refused "a deleted source file" "crates/engine/src/a.rs is gone"
}

test_a_new_file_in_a_listed_directory_refuses() {
    echo "test: a new file in a listed directory refuses, even untracked"
    fresh
    echo 'CREATE TABLE u ();' >"$REPO/crates/engine/migrations/0002.sql"
    expect_refused "a new migration" "crates/engine/migrations/"
}

test_another_configuration_refuses() {
    echo "test: another toolchain, profile, feature list or platform refuses"
    fresh
    echo 'channel = "1.0.1"' >"$REPO/rust-toolchain.toml"
    expect_refused "another toolchain pin" "rust-toolchain.toml changed"
    fresh
    if engine_manifest_check "$REPO" "$MANIFEST" release e2e-test-hooks 2>/dev/null; then
        fail "another profile was accepted"
    else
        pass "another profile refuses"
    fi
    if engine_manifest_check "$REPO" "$MANIFEST" e2e "" 2>/dev/null; then
        fail "another feature list was accepted"
    else
        pass "another feature list refuses"
    fi
    expect_refused "another platform" "platform" Plan9-mips
    if RUSTFLAGS="-C target-cpu=native" check; then
        fail "other RUSTFLAGS were accepted"
    else
        pass "other RUSTFLAGS refuse"
    fi
}

test_an_input_edited_during_the_build_refuses_to_write() {
    echo "test: an input newer than the dep-info is never written into a manifest"
    make_checkout "$REPO"
    touch -t 200001010000 "$REPO"/target/e2e/*.d
    if engine_manifest_write "$REPO" "$REPO/target/e2e" "$MANIFEST.late" e2e e2e-test-hooks 2>/dev/null; then
        fail "a manifest was written over inputs that changed after the build"
    else
        pass "no manifest over inputs that changed after the build"
    fi
}

test_the_prekey_follows_configuration_and_pins_only() {
    echo "test: the pre-key changes with the configuration and the pins, not with source"
    fresh
    local base
    base="$(engine_manifest_prekey "$REPO" e2e e2e-test-hooks Linux-x86_64)"
    echo 'pub fn a() { 2; }' >"$REPO/crates/engine/src/a.rs"
    if [ "$(engine_manifest_prekey "$REPO" e2e e2e-test-hooks Linux-x86_64)" = "$base" ]; then
        pass "a source edit keeps the pre-key"
    else
        fail "a source edit moved the pre-key"
    fi
    if [ "$(engine_manifest_prekey "$REPO" e2e e2e-test-hooks Darwin-arm64)" != "$base" ]; then pass "another platform moves it"; else fail "another platform kept it"; fi
    echo 'lock v2' >"$REPO/Cargo.lock"
    if [ "$(engine_manifest_prekey "$REPO" e2e e2e-test-hooks Linux-x86_64)" != "$base" ]; then pass "a lockfile change moves it"; else fail "a lockfile change kept it"; fi
}

mtime_year() { # <file>
    python3 -c 'import os, sys, time; print(time.gmtime(os.path.getmtime(sys.argv[1])).tm_year)' "$1"
}

test_the_mtime_step_ages_only_unchanged_inputs() {
    echo "test: the mtime step ages unchanged inputs and stamps every other file now"
    fresh
    echo 'pub fn a() { 4; }' >"$REPO/crates/engine/src/a.rs"
    echo 'pub fn b() {}' >"$REPO/crates/engine/src/b.rs"
    engine_manifest_stamp_mtimes "$REPO" "$MANIFEST" >/dev/null
    local now f
    now="$(date -u +%Y)"
    for f in crates/engine/src/main.rs crates/engine/migrations/0001.sql Cargo.lock; do
        if [ "$(mtime_year "$REPO/$f")" = 2000 ]; then pass "$f is aged"; else fail "$f was not aged"; fi
    done
    for f in crates/engine/src/a.rs crates/engine/src/b.rs crates/app/src/main.tsx; do
        if [ "$(mtime_year "$REPO/$f")" = "$now" ]; then pass "$f is stamped now"; else fail "$f was aged"; fi
    done
    echo 'CREATE TABLE u ();' >"$REPO/crates/engine/migrations/0002.sql"
    engine_manifest_stamp_mtimes "$REPO" "$MANIFEST" >/dev/null
    if [ "$(mtime_year "$REPO/crates/engine/migrations/0001.sql")" = "$now" ]; then pass "a changed directory stamps all its files now"; else fail "a changed directory kept an aged file"; fi
}

test_the_cache_files_a_build_and_finds_it_only_for_a_matching_tree() {
    echo "test: the local cache files a build and hands it back only to a matching tree"
    rm -rf "$LUCIDOS_E2E_ENGINE_CACHE"
    make_checkout "$REPO"
    engine_cache_store "$REPO" "$REPO/target/e2e" e2e e2e-test-hooks >/dev/null || fail "store failed"
    local found
    found="$(engine_cache_find "$REPO" e2e e2e-test-hooks)"
    if [ -n "$found" ] && [ "$(cat "$found/lucidos-engine")" = "lucidos-engine binary" ] \
        && [ -f "$found/lucidos-codegen" ] && [ ! -e "$found/lucidos-engine.d" ]; then
        pass "a matching tree finds the filed binaries"
    else
        fail "a matching tree found '$found'"
    fi
    echo 'pub fn a() { 3; }' >"$REPO/crates/engine/src/a.rs"
    if [ -z "$(engine_cache_find "$REPO" e2e e2e-test-hooks)" ]; then
        pass "an edited tree finds nothing"
    else
        fail "an edited tree was handed a cached engine"
    fi
    if [ -z "$(engine_cache_find "$REPO" release e2e-test-hooks)" ]; then
        pass "another profile finds nothing"
    else
        fail "another profile was handed a cached engine"
    fi
}

test_the_cache_keeps_only_the_newest_builds() {
    echo "test: the cache keeps the newest $ENGINE_CACHE_KEEP builds"
    rm -rf "$LUCIDOS_E2E_ENGINE_CACHE"
    local i
    for i in $(seq 1 $((ENGINE_CACHE_KEEP + 2))); do
        make_checkout "$REPO"
        echo "pub fn a() { $i; }" >"$REPO/crates/engine/src/a.rs"
        touch "$REPO"/target/e2e/*.d
        engine_cache_store "$REPO" "$REPO/target/e2e" e2e e2e-test-hooks >/dev/null
        sleep 1
    done
    local kept
    kept="$(find "$LUCIDOS_E2E_ENGINE_CACHE" -name manifest | grep -c .)"
    if [ "$kept" -eq "$ENGINE_CACHE_KEEP" ]; then pass "kept $kept"; else fail "kept $kept"; fi
    if [ -n "$(engine_cache_find "$REPO" e2e e2e-test-hooks)" ]; then
        pass "the newest build survives"
    else
        fail "the newest build was pruned"
    fi
}

test_the_manifest_lists_inputs_and_pins_but_no_outputs
test_an_unchanged_tree_matches
test_a_frontend_or_spec_edit_still_matches
test_a_rust_edit_refuses
test_a_new_file_in_a_listed_directory_refuses
test_another_configuration_refuses
test_an_input_edited_during_the_build_refuses_to_write
test_the_prekey_follows_configuration_and_pins_only
test_the_mtime_step_ages_only_unchanged_inputs
test_the_cache_files_a_build_and_finds_it_only_for_a_matching_tree
test_the_cache_keeps_only_the_newest_builds

echo ""
echo "engine_manifest_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
