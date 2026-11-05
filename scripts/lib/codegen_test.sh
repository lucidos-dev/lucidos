#!/usr/bin/env bash
# Tests for scripts/lib/codegen.sh: the codegen fingerprint, the cache, the
# copy that writes only changed bytes, and the dep-info check.
#
# Hermetic: render/cache fixtures use scratch repos; build inputs read this
# checkout. Stubs replace the writers and SDK build, so no Cargo or npm runs.
#
# Run: bash scripts/lib/codegen_test.sh
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

echo "engine build inputs"
ENGINE_ROOT="$SCRIPT_DIR/../../crates/lucidos-engine"
WATCH_PATHS=$(sed -n 's/.*println!("cargo:rerun-if-changed=\([^"{}]*\)").*/\1/p' "$ENGINE_ROOT/build.rs")
expect_ne "the build script declares its source inputs" "" "$WATCH_PATHS"
while IFS= read -r watched; do
    if [ -e "$ENGINE_ROOT/$watched" ] && [ -n "$(git -C "$ENGINE_ROOT" ls-files -- "$watched")" ]; then
        pass "declared build input is present and tracked: $watched"
    else
        fail "declared build input is missing or untracked: $watched"
    fi
done <<< "$WATCH_PATHS"

# The SDK step needs node. Record that it ran instead.
# CLI composition is tested separately; cache fixtures never invoke cargo.
codegen_cli_surface() { :; }
codegen_build_sdk() { echo sdk >> "$TMP/sdk-log"; }

WIRE=crates/lucidos-app/src/generated/thread-event-wire.ts

new_repo() { # <dir>
    git init -q -b main "$1"
    mkdir -p "$1/crates/lucidos-engine/src" "$1/crates/lucidos-app/src" "$1/scripts"
    echo 'pub const A: u32 = 1;' > "$1/crates/lucidos-engine/src/lib.rs"
    echo '# lock' > "$1/Cargo.lock"
    echo 'export {};' > "$1/crates/lucidos-app/src/main.ts"
    mkdir -p "$1/system-knowhow" "$1/scripts/lib"
    cp "$SCRIPT_DIR/codegen_blocks.py" "$1/scripts/lib/codegen_blocks.py"
    printf 'Handwritten\n<!-- generated:begin Sample -->\nstale\n<!-- generated:end -->\n' > "$1/system-knowhow/sample.md"
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
mkdir -p "$out/.codegen"
echo '{"schema_version":1,"documents":{"system-knowhow/sample.md":{"Sample":"\n`fresh`\n"}}}' > "$out/.codegen/knowhow-blocks.json"
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

echo "CLI dependency fingerprints"
for crate in lucidos-cli lucidos-installs lucidos-update-address; do
    mkdir -p "$R/crates/$crate/src"
    echo 'pub fn flag() {}' > "$R/crates/$crate/src/lib.rs"
    expect_ne "$crate changes the fingerprint" "$clean" "$(codegen_fingerprint "$R")"
    rm "$R/crates/$crate/src/lib.rs"
done
mkdir -p "$R/packages/lucidos-sdk/src"
echo 'export const lucidos = { added };' > "$R/packages/lucidos-sdk/src/index.ts"
expect_ne "SDK exports change the fingerprint" "$clean" "$(codegen_fingerprint "$R")"
rm "$R/packages/lucidos-sdk/src/index.ts"
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
sed -i.bak 's/Handwritten/Edited prose/' "$R/system-knowhow/sample.md"
rm "$R/system-knowhow/sample.md.bak"
prose=$(cat "$R/system-knowhow/sample.md")
codegen_main "$R" --cached-only 2>/dev/null
expect_eq "a cache hit preserves handwritten prose" "$prose" "$(cat "$R/system-knowhow/sample.md")"
expect_eq "fragments never sync as ordinary outputs" "no" "$([ -e "$R/$CODEGEN_BLOCKS" ] && echo yes || echo no)"
echo 'Missing markers' > "$R/system-knowhow/sample.md"
expect_eq "a cache hit fails on missing markers" "1" "$(codegen_main "$R" --cached-only 2>/dev/null; echo $?)"
expect_eq "a failed splice preserves the document" "Missing markers" "$(cat "$R/system-knowhow/sample.md")"
printf '%s\n' "$prose" > "$R/system-knowhow/sample.md"

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
mkdir -p "$R/crates/lucidos-engine/assets"
printf '%s: %s\n' "$TMP/test-bin" "$R/crates/lucidos-engine/assets" > "$TMP/directory.d"
touch "$TMP/test-bin"
expect_eq "a declared source directory is a valid dependency" "0" \
    "$(codegen_check_inputs "$R" "$TMP/directory.d" "$TMP/test-bin" 2>/dev/null; echo $?)"
touch -t 200001010000 "$TMP/test-bin"
expect_eq "a newer source directory fails freshness" "1" \
    "$(codegen_check_inputs "$R" "$TMP/directory.d" "$TMP/test-bin" 2>/dev/null; echo $?)"
rmdir "$R/crates/lucidos-engine/assets"
touch "$TMP/test-bin"
expect_eq "a missing source directory fails closed" "1" \
    "$(codegen_check_inputs "$R" "$TMP/directory.d" "$TMP/test-bin" 2>/dev/null; echo $?)"
echo "dependency artifacts"
mkdir -p "$R/target/debug/deps"
echo 'binary' > "$R/target/debug/deps/liblucidos_cli-abc.rlib"
printf '%s: %s\n' "$R/target/debug/deps/liblucidos_cli-abc.rlib" "$R/crates/lucidos-engine/src/lib.rs" > "$R/target/debug/deps/lucidos_cli-abc.d"
python3 - "$R" > "$TMP/artifacts.json" <<'PY'
import json, sys
root = sys.argv[1]
print(json.dumps({'reason': 'compiler-artifact', 'package_id': 'path+file://' + root + '/crates/lucidos-cli#0.1',
                  'filenames': [root + '/target/debug/deps/liblucidos_cli-abc.rlib']}))
PY
expect_eq "a dependency's dep-info is checked" "$R/target/debug/deps/lucidos_cli-abc.d" \
    "$(codegen_artifacts "$R" "$TMP/artifacts.json" | cut -f1)"
rm "$R/target/debug/deps/lucidos_cli-abc.d"
expect_eq "missing dependency dep-info fails" "1" "$(codegen_artifacts "$R" "$TMP/artifacts.json" 2>/dev/null; echo $?)"
: > "$TMP/empty.d"
expect_eq "empty dep-info fails" "1" "$(codegen_check_inputs "$R" "$TMP/empty.d" "$BIN" 2>/dev/null; echo $?)"
printf '# env-dep:EXAMPLE=value\n%s: %s\n' "$R/target/debug/x" "$R/Cargo.lock" > "$TMP/comments.d"
expect_eq "rustc metadata comments are not paths" "Cargo.lock" "$(codegen_repo_inputs "$R" "$TMP/comments.d")"

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
expect_eq "a reused bin is not refused for its age" "0" \
    "$(codegen_main "$R" --reused-bin "$BIN" 2>/dev/null; echo $?)"
git -C "$R" reset -q --hard

echo "surface composition"
mkdir -p "$TMP/surface/docs/generated"
echo '{"tools":{"offered":["current"],"retired":["old"]}}' > "$TMP/surface/docs/generated/agent-surface.json"
echo '{"name":"lucidos","commands":[{"name":"sample"}]}' > "$TMP/cli-fixture.json"
(
    source "$SCRIPT_DIR/codegen.sh"
    codegen_writer() { cp "$TMP/cli-fixture.json" "$2/cli-surface.json"; }
    codegen_cli_surface "$R" "$TMP/surface"
)
expect_eq "the CLI tree is combined" "lucidos" \
    "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["cli"]["name"])' "$TMP/surface/docs/generated/agent-surface.json")"
expect_eq "retired names carry their classification on the same line" '    "retired": ["old"]' \
    "$(sed -n '/"retired"/p' "$TMP/surface/docs/generated/agent-surface.json")"
if [ -f "$TMP/surface/cli-surface.json" ]; then
    fail "the intermediate CLI file was synced"
else
    pass "the intermediate CLI file is not synced"
fi

echo "prune"
CACHE=$(codegen_cache_root "$R")
for i in $(seq 1 40); do mkdir -p "$CACHE/fp$i"; done
touch -t 202001010000 "$CACHE/fp1"
codegen_cached "$R" fp1 > /dev/null
CODEGEN_CACHE_KEEP=5 codegen_prune "$CACHE"
expect_eq "the cache keeps the newest renderings" "5" "$(find "$CACHE" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')"
expect_eq "a cache hit counts as recent" "yes" "$([ -d "$CACHE/fp1" ] && echo yes)"

echo ""
echo "codegen_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
