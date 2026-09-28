#!/bin/bash
# Tests for scripts/harden-suites.sh and its rules in
# scripts/lib/harden_suites.sh: the early suite run of `/harden` (ADR 0292).
#
# Hermetic: every run happens in a throwaway git repo, and HS_STUB_CMD replaces
# each suite with a stub, so no cargo, npm or Postgres runs. `stop` only ever
# signals the pid tree of a run this file started.
#
# Covered: suite selection row by row; the safe-path allowlist against
# commits, edits, staged edits, and untracked files added or deleted; an edit
# undone after the suite exits; the dep-info check in rustc's relative form;
# the read check; a fix that widens the selection; failing and zero-test
# suites; the Codex join; a run still setting up; start refusals; stop reaching
# a grandchild and sparing a recycled pid; the run staying in the caller's
# process group.
#
# Run: ./scripts/lib/harden_suites_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/harden_suites.sh
source "$SCRIPT_DIR/harden_suites.sh"
CLI="$SCRIPT_DIR/../harden-suites.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
expect_eq() { # <name> <expected> <actual>
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi
}
expect_has() { # <name> <needle> <haystack>
    case "$3" in
        *"$2"*) pass "$1" ;;
        *) fail "$1: [$2] not in [$3]" ;;
    esac
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---------------------------------------------------------------- pure rules

echo "safe paths"
for p in docs/plans/x.md docs/adr/0001-x.md docs/code-review-priors.md docs/temporary-measures.md; do
    if hs_is_safe_path "$p"; then pass "safe: $p"; else fail "safe: $p"; fi
done
for p in docs/glossary.md docs/adrx.md system-knowhow/a.md crates/a.rs README.md; do
    if hs_is_safe_path "$p"; then fail "unsafe: $p"; else pass "unsafe: $p"; fi
done

echo "suite selection"
# The compile inputs are a fixed list, so selection never depends on a build.
printf '%s\n' CHANGELOG.md crates/lucidos-engine/src/runtime/cc_menu_options.json \
    crates/lucidos-engine/src/api/sdk_iframe.css \
    crates/lucidos-app/src/components/settings/LocaleSection.tsx \
    crates/lucidos-app/src/store/actions/preferences.ts crates/lucidos-app/tauri.conf.json > "$TMP/inputs"
# The engine files the CLI's own source includes, as `start` records them.
printf '%s\n' crates/lucidos-engine/src/runtime/cc_menu_options.json > "$TMP/cli-inputs"
# The files a Vitest test reads, as `start` records them from hs_vitest_inputs.
printf '%s\n' crates/lucidos-engine/src/api \
    crates/lucidos-engine/src/engine/event_bus_system_event.rs .claude/rules/frontend.md > "$TMP/vitest-inputs"
select_for() { printf '%s\n' "$@" | hs_select_suites "$TMP/inputs" "$TMP/cli-inputs" "$TMP/vitest-inputs" | tr '\n' ' ' | sed 's/ $//'; }
expect_eq "rust" "rust" "$(select_for crates/lucidos-engine/src/a.rs)"
expect_eq "Cargo.lock" "rust" "$(select_for Cargo.lock)"
expect_eq "Cargo.toml" "rust" "$(select_for crates/lucidos-engine/Cargo.toml)"
expect_eq ".shellcheckrc" "shell-lint" "$(select_for .shellcheckrc)"
expect_eq "an app compile input runs the app tests" "rust app-lib" "$(select_for crates/lucidos-app/tauri.conf.json)"
expect_eq "migration" "rust" "$(select_for crates/lucidos-engine/migrations/1_x.sql)"
expect_eq "app crate" "rust app-lib" "$(select_for crates/lucidos-app/src/lib.rs)"
expect_eq "cli crate" "rust cli" "$(select_for crates/lucidos-cli/src/main.rs)"
expect_eq "a CLI include runs the CLI tests" "rust cli" \
    "$(select_for crates/lucidos-engine/src/runtime/cc_menu_options.json)"
expect_eq "no cli-inputs file" "rust" \
    "$(printf '%s\n' crates/lucidos-engine/src/runtime/cc_menu_options.json | hs_select_suites "$TMP/inputs" | tr '\n' ' ' | sed 's/ $//')"
expect_eq "a compiled-in markdown file" "rust" "$(select_for CHANGELOG.md)"
expect_eq "no compile-inputs file" "" "$(printf '%s\n' CHANGELOG.md | hs_select_suites | tr '\n' ' ')"
expect_eq "shell" "shell-lint" "$(select_for scripts/foo.sh)"
expect_eq "Makefile" "shell-lint" "$(select_for Makefile)"
expect_eq "installer" "shell-lint install" "$(select_for install.sh)"
for f in uninstall.sh scripts/lib/service.sh scripts/lib/stage_runtime.sh \
    scripts/lib/headless_tarball.sh scripts/lib/install_common.sh; do
    expect_eq "installer: $f" "shell-lint install" "$(select_for "$f")"
done
expect_eq "release script" "shell-lint release" "$(select_for scripts/release.sh)"
expect_eq "preferences" "rust app-lib ts" "$(select_for crates/lucidos-app/src/store/actions/preferences.ts)"
expect_eq "proc tree" "shell-lint harden-suites" "$(select_for scripts/lib/proc_tree.sh)"
expect_eq "release" "shell-lint release" "$(select_for scripts/lib/release_draft.sh)"
expect_eq "self" "shell-lint harden-suites" "$(select_for scripts/lib/harden_suites.sh)"
expect_eq "harden scope" "shell-lint harden-scope" "$(select_for scripts/harden-scope.sh)"
expect_eq "harden scope test" "shell-lint harden-scope" "$(select_for scripts/lib/harden_scope_test.sh)"
expect_eq "ts" "ts" "$(select_for crates/lucidos-app/src/a.ts)"
expect_eq "css" "vite" "$(select_for crates/lucidos-app/src/a.css)"
expect_eq "sdk iframe css" "rust vitest" "$(select_for crates/lucidos-engine/src/api/sdk_iframe.css)"
expect_eq "a Rust file a Vitest test reads" "rust vitest" \
    "$(select_for crates/lucidos-engine/src/engine/event_bus_system_event.rs)"
expect_eq "a Rust file no Vitest test reads" "rust" "$(select_for crates/lucidos-engine/src/engine/other.rs)"
expect_eq "a rule a Vitest test reads" "vitest" "$(select_for .claude/rules/frontend.md)"
expect_eq "a deleted file under a named directory" "rust vitest" \
    "$(select_for crates/lucidos-engine/src/api/gone.rs)"
expect_eq "ts already runs Vitest" "rust ts" \
    "$(select_for crates/lucidos-engine/src/engine/event_bus_system_event.rs crates/lucidos-app/src/a.ts)"

echo "vitest inputs, scanned from this checkout"
CHECKOUT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VI="$(hs_vitest_inputs "$CHECKOUT")"
expect_has "the event enum the coverage test reads" "crates/lucidos-engine/src/engine/event_bus_system_event.rs" "$VI"
expect_has "the stylesheet the CSS parse test reads" "crates/lucidos-engine/src/api/sdk_iframe.css" "$VI"
expect_lacks "no path escapes the checkout" "../" "$VI"
VI_CONFIGURED="$(GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=grep.lineNumber GIT_CONFIG_VALUE_0=true \
    GIT_CONFIG_KEY_1=grep.column GIT_CONFIG_VALUE_1=true hs_vitest_inputs "$CHECKOUT")"
expect_eq "a git config cannot change the scan" "$VI" "$VI_CONFIGURED"
expect_eq "ts subsumes css" "ts" "$(select_for crates/lucidos-app/src/a.ts crates/lucidos-app/src/a.css)"
expect_eq "locale" "rust app-lib ts" \
    "$(select_for crates/lucidos-app/src/components/settings/LocaleSection.tsx)"
expect_eq "knowhow" "engine-filtered" "$(select_for system-knowhow/a.md)"
expect_eq "rust subsumes filters" "rust" "$(select_for system-knowhow/a.md crates/lucidos-engine/src/a.rs)"
expect_eq "docs only" "" "$(select_for docs/a.md README.md)"

echo "engine filters"
expect_eq "knowhow filters" \
    "always_loaded_context_stays_under_budget system_knowhow_descriptions_stay_routing_sized" \
    "$(printf '%s\n' crates/a.ts system-knowhow/a.md | hs_engine_filters)"
expect_eq "no knowhow, no filters" "" "$(printf '%s\n' crates/a.ts | hs_engine_filters)"

echo "source includes"
SRC="$TMP/src"
mkdir -p "$SRC/crates/c/src/api" "$SRC/crates/c/assets"
: > "$SRC/crates/c/Cargo.toml"
cat > "$SRC/crates/c/src/api/a.rs" << 'EOF'
const A: &str = include_str!("../../assets/a.css");
const B: &[u8] = include_bytes!(
    "../../../../CHANGELOG.md"
);
const C: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/c.json"));
// prose that says include_str! without a literal is not an include
EOF
git -C "$SRC" init -q && git -C "$SRC" add -A
expect_eq "every include form resolves" "CHANGELOG.md crates/c/assets/a.css crates/c/assets/c.json" \
    "$(hs_source_includes "$SRC" | tr '\n' ' ' | sed 's/ $//')"
mkdir -p "$SRC/crates/d/src"
: > "$SRC/crates/d/Cargo.toml"
echo 'const D: &str = include_str!("../../c/src/api/a.rs");' > "$SRC/crates/d/src/d.rs"
git -C "$SRC" add -A
expect_eq "a pathspec narrows to one crate" "crates/c/src/api/a.rs" \
    "$(hs_source_includes "$SRC" 'crates/d/*.rs' | tr '\n' ' ' | sed 's/ $//')"

echo "suite commands"
early_rust="$(HS_STUB_CMD='' hs_suite_command rust early /dev/null)"
expect_has "early rust skips codex driver" "--skip runtime::codex::driver_tests" "$early_rust"
expect_has "early rust skips app-server driver" "--skip runtime::codex_app_server::driver_tests" "$early_rust"
expect_eq "normal rust skips nothing" "make lint && make test" "$(HS_STUB_CMD='' hs_suite_command rust normal /dev/null)"
expect_has "driver names both modules" \
    "runtime::codex::driver_tests runtime::codex_app_server::driver_tests" \
    "$(HS_STUB_CMD='' hs_suite_command driver early /dev/null)"
printf '%s\n' system-knowhow/a.md > "$TMP/paths"
expect_has "engine-filtered carries filters" "-- -- always_loaded" \
    "$(HS_STUB_CMD='' hs_suite_command engine-filtered normal "$TMP/paths")"

# ---------------------------------------------------------------- fixture repo

REPO="$TMP/repo"
STUB_DIR="$TMP/stub"
mkdir -p "$REPO" "$STUB_DIR"
git -C "$REPO" init -q -b main
git -C "$REPO" config user.email "t@t"
git -C "$REPO" config user.name "t"
printf '.lucidos/\ntarget/\n' > "$REPO/.gitignore"
mkdir -p "$REPO/crates/lucidos-engine/src" "$REPO/crates/lucidos-app/src" "$REPO/docs/plans"
echo "fn a() {}" > "$REPO/crates/lucidos-engine/src/a.rs"
echo "export {}" > "$REPO/crates/lucidos-app/src/a.ts"
git -C "$REPO" add -A && git -C "$REPO" commit -qm base
git -C "$REPO" checkout -q -b feature
echo "fn a() { 1; }" > "$REPO/crates/lucidos-engine/src/a.rs"
echo "export const x = 1" > "$REPO/crates/lucidos-app/src/a.ts"
git -C "$REPO" add -A && git -C "$REPO" commit -qm change
ROOT="$(git -C "$REPO" rev-parse --show-toplevel)"

# The stub reads its behaviour per suite from $STUB_DIR: <suite>.rc (exit
# code), <suite>.sleep (seconds), <suite>.child (spawn a grandchild).
cat > "$STUB_DIR/stub.sh" << 'EOF'
#!/bin/bash
s=$1
d=$(dirname "$0")
ps -o pgid= -p $$ | tr -d ' ' > "$d/$s.pgid"
echo ran >> "$d/$s.ran"
if [ -f "$d/$s.child" ]; then
    sleep 300 &
    echo $! > "$d/$s.childpid"
fi
[ -f "$d/$s.sleep" ] && sleep "$(cat "$d/$s.sleep")"
[ "$s" = driver ] && [ ! -f "$d/driver.notests" ] && echo "running 3 tests"
exit "$(cat "$d/$s.rc" 2>/dev/null || echo 0)"
EOF
chmod +x "$STUB_DIR/stub.sh"
export HS_STUB_CMD="$STUB_DIR/stub.sh"

hs() { (cd "$REPO" && bash "$CLI" "$@"); }
reset_stub() { rm -f "$STUB_DIR"/*.rc "$STUB_DIR"/*.sleep "$STUB_DIR"/*.child "$STUB_DIR"/*.ran \
    "$STUB_DIR"/*.pgid "$STUB_DIR"/*.childpid "$STUB_DIR"/driver.notests; }
commit_file() { # <relpath> <content>
    mkdir -p "$REPO/$(dirname "$1")"
    echo "$2" > "$REPO/$1"
    git -C "$REPO" add -- "$1" && git -C "$REPO" commit -qm "$1"
}

echo "early run, clean"
hs start --early > "$TMP/out" 2>&1
expect_eq "start exits 0 once its suites ran" 0 "$?"
expect_has "selects rust and ts" "Suites (early): rust ts" "$(cat "$TMP/out")"
out="$(hs wait --budget 20)"
rc=$?
expect_eq "wait exit" 0 "$rc"
expect_has "driver ran alone" "driver PASS" "$out"
expect_has "rust passes" "rust PASS" "$out"
expect_has "ts passes" "ts PASS" "$out"
expect_eq "run shares the caller's process group" "$(ps -o pgid= -p $$ | tr -d ' ')" "$(cat "$STUB_DIR/rust.pgid")"

echo "allowlisted changes keep the result"
commit_file docs/plans/p.md plan
commit_file docs/code-review-priors.md prior
hs verdict > /dev/null
expect_eq "allowlisted commits: PASS" 0 "$?"
echo "draft" > "$REPO/docs/temporary-measures.md"
hs verdict > /dev/null
expect_eq "allowlisted untracked file: PASS" 0 "$?"
rm "$REPO/docs/temporary-measures.md"

echo "other changes void it"
echo "fn a() { 2; }" > "$REPO/crates/lucidos-engine/src/a.rs"
out="$(hs verdict)"
rc=$?
expect_eq "unstaged edit: RERUN" 2 "$rc"
expect_has "names the path" "VOID crates/lucidos-engine/src/a.rs" "$out"
git -C "$REPO" add -A
hs verdict > /dev/null
expect_eq "staged edit: RERUN" 2 "$?"
git -C "$REPO" reset -q --hard
mkdir -p "$REPO/system-knowhow" && echo new > "$REPO/system-knowhow/n.md"
hs verdict > /dev/null
expect_eq "untracked file: RERUN" 2 "$?"
rm -rf "$REPO/system-knowhow"

echo "an untracked file deleted since the start voids it"
reset_stub
echo "draft" > "$REPO/crates/lucidos-engine/src/stray.rs"
hs start --early > /dev/null 2>&1
rm "$REPO/crates/lucidos-engine/src/stray.rs"
out="$(hs wait --budget 20)"
expect_eq "deleted untracked file: RERUN" 2 "$?"
expect_has "names it" "VOID crates/lucidos-engine/src/stray.rs" "$out"
commit_file crates/lucidos-engine/src/b.rs "fn b() {}"
hs verdict > /dev/null
expect_eq "committed fix: RERUN" 2 "$?"

echo "an edit made mid-run and undone still voids"
reset_stub
echo 3 > "$STUB_DIR/rust.sleep"
hs start --early > /dev/null 2>&1 &
BG=$!
sleep 1
cp "$REPO/crates/lucidos-engine/src/a.rs" "$TMP/a.rs.bak"
echo "fn a() { 3; }" > "$REPO/crates/lucidos-engine/src/a.rs"
wait $BG
cp "$TMP/a.rs.bak" "$REPO/crates/lucidos-engine/src/a.rs"
out="$(hs wait --budget 20)"
rc=$?
expect_eq "mid-run edit: RERUN" 2 "$rc"
expect_has "rust voided by its exit stamp" "rust VOID crates/lucidos-engine/src/a.rs" "$out"
if [ -f "$STUB_DIR/driver.ran" ]; then fail "driver skipped after a void"; else pass "driver skipped after a void"; fi

echo "a failing suite"
reset_stub
echo 1 > "$STUB_DIR/ts.rc"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "failing ts: FAIL" 1 "$?"
expect_has "names ts" "ts FAIL" "$out"

echo "a driver run with no tests"
reset_stub
touch "$STUB_DIR/driver.notests"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "no driver tests: FAIL" 1 "$?"
expect_has "says so" "driver FAIL ran no tests" "$out"

echo "the Codex join"
reset_stub
mkdir -p "$REPO/.lucidos" && echo "{}" > "$REPO/.lucidos/codex-review.out"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "Codex still running: wait says running" 3 "$?"
if [ -f "$STUB_DIR/driver.ran" ]; then fail "driver held back"; else pass "driver held back"; fi
out="$(hs verdict)"
expect_eq "no driver run yet: RERUN, never PASS" 2 "$?"
expect_has "says so" "driver MISSING" "$out"
hs wait --budget 20 --codex-abandoned > /dev/null
expect_eq "abandoned Codex review: driver runs" 0 "$?"
touch "$REPO/.lucidos/codex-review.done"

echo "a normal run has no driver step"
reset_stub
hs start > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "normal run: PASS" 0 "$?"
case "$out" in
    *driver*) fail "no driver line in a normal run" ;;
    *) pass "no driver line in a normal run" ;;
esac

echo "start refusals and stop"
reset_stub
echo "dirty" >> "$REPO/crates/lucidos-engine/src/a.rs"
hs start --early > /dev/null 2>&1
expect_eq "dirty tree refused" 1 "$?"
git -C "$REPO" checkout -q -- crates/lucidos-engine/src/a.rs
echo 30 > "$STUB_DIR/rust.sleep"
touch "$STUB_DIR/rust.child"
hs start --early > /dev/null 2>&1 &
BG=$!
for _ in $(seq 1 50); do [ -f "$STUB_DIR/rust.childpid" ] && break; sleep 0.1; done
hs start --early > /dev/null 2>&1
expect_eq "second start refused" 1 "$?"
child="$(cat "$STUB_DIR/rust.childpid")"
hs stop > /dev/null
wait $BG 2> /dev/null
if kill -0 "$child" 2> /dev/null; then fail "stop reached the grandchild"; kill "$child"; else pass "stop reached the grandchild"; fi
if kill -0 "$BG" 2> /dev/null; then fail "stop ended the runner"; else pass "stop ended the runner"; fi
out="$(hs verdict)"
expect_eq "stopped run: RERUN" 2 "$?"
expect_has "says stopped" "rust VOID stopped" "$out"
hs start --early > /dev/null 2>&1 &
BG=$!
sleep 0.5
hs stop > /dev/null
wait $BG 2> /dev/null

echo "the dep-info check"
reset_stub
mkdir -p "$REPO/target/debug/deps"
# rustc's own form for an include_str! input: workspace-relative, with `..`.
echo "$ROOT/target/debug/x: crates/lucidos-engine/src/../../../docs/adr/0001-x.md" \
    > "$REPO/target/debug/deps/x.d"
hs start --early > /dev/null 2>&1
commit_file docs/adr/0001-x.md adr
out="$(hs wait --budget 20)"
expect_eq "allowlisted compile input: RERUN" 2 "$?"
expect_has "names it" "VOID docs/adr/0001-x.md" "$out"
printf '# env-dep:X=y\n%s\n' "$ROOT/target/debug/x: $ROOT/docs/adr/0002-y.md" > "$REPO/target/debug/deps/x.d"
expect_eq "a missing compile-inputs file voids a safe path" "docs/adr/x.md" \
    "$(echo docs/adr/x.md | hs_unsafe_paths "$TMP/no-such-file" 0)"
expect_eq "an absolute dep-info input normalises, comment lines drop" "docs/adr/0002-y.md target/debug/x" \
    "$(hs_compile_inputs "$ROOT" | tr '\n' ' ' | sed 's/ $//')"
rm -rf "$REPO/target"

echo "the read check"
reset_stub
READ_SRC='let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/plans/x.md");'
commit_file crates/lucidos-engine/src/reads.rs "$READ_SRC"
out="$(hs start --early 2>&1)"
expect_has "start says the allowlist is off" "allowlist disabled" "$out"
commit_file docs/plans/q.md plan
hs wait --budget 20 > /dev/null
expect_eq "allowlist off: RERUN on a plan edit" 2 "$?"
git -C "$REPO" rm -q crates/lucidos-engine/src/reads.rs && git -C "$REPO" commit -qm rm

read_hits() { hs_read_check "$ROOT" | tr '\n' ' ' | sed 's/ $//'; }
commit_file crates/x/src/a.rs '// docs/plans/x.md read via CARGO_MANIFEST_DIR'
expect_eq "comment lines never count" "" "$(read_hits)"
commit_file crates/x/src/b.rs 'let msg = "see docs/plans/x.md";'
expect_eq "a plain mention without an anchor is not a read" "" "$(read_hits)"
commit_file crates/x/src/c.rs 'fs::read_to_string("../../docs/adr/1.md")'
expect_eq "a relative path is a read" "crates/x/src/c.rs 1" "$(read_hits)"
git -C "$REPO" rm -q crates/x/src/c.rs && git -C "$REPO" commit -qm rm
PINNED=crates/lucidos-engine/src/engine/frontend_refresh.rs
commit_file "$PINNED" "$(printf 'include_str!("x");\n"see docs/plans/y.md"')"
expect_eq "a pinned file at its count passes" "" "$(read_hits)"
commit_file "$PINNED" "$(printf 'include_str!("x");\n"see docs/plans/y.md"\n"and docs/adr/z.md"')"
expect_eq "a pinned file over its count trips" "$PINNED 2" "$(read_hits)"

echo "a docs-only diff selects nothing"
git -C "$REPO" checkout -q -b docs-only main
commit_file docs/readme-extra.md words
out="$(hs start --early 2>&1)"
expect_has "nothing selected" "No suites selected" "$out"
hs verdict > /dev/null
expect_eq "empty selection: PASS" 0 "$?"
commit_file crates/lucidos-engine/src/fix.rs "fn fix() {}"
out="$(hs verdict)"
expect_eq "a fix that widens the selection: RERUN" 2 "$?"
expect_has "names the new selection" "selection VOID the diff now selects: rust" "$out"

echo "an edit to a compiled-in file selects Rust with no build"
git -C "$REPO" checkout -q main
mkdir -p "$REPO/crates/lucidos-engine/src/engine"
commit_file crates/lucidos-engine/Cargo.toml ""
commit_file crates/lucidos-engine/src/engine/changelog.rs 'const C: &str = include_str!("../../../../CHANGELOG.md");'
commit_file CHANGELOG.md "v1"
git -C "$REPO" checkout -q -b changelog-only
commit_file CHANGELOG.md "v2"
out="$(hs start --early 2>&1)"
expect_has "the changelog edit selects rust" "Suites (early): rust" "$out"
hs stop > /dev/null

echo "a filtered engine run that matched no test"
reset_stub
git -C "$REPO" checkout -q -b knowhow main
commit_file system-knowhow/a.md words
cat > "$STUB_DIR/stub.sh" << 'EOF'
#!/bin/bash
echo "running 0 tests"
EOF
hs start > /dev/null 2>&1
out="$(hs verdict)"
expect_eq "zero filtered tests: FAIL" 1 "$?"
expect_has "says so" "engine-filtered FAIL ran no tests" "$out"

echo "a run still setting up is never a verdict"
sleep 5 &
FAKE=$!
sleep 0.2
rm -f "$REPO/.lucidos/harden-suites/suites"
echo "$FAKE" > "$REPO/.lucidos/harden-suites/pid"
ps -o lstart= -p "$FAKE" | sed 's/^ *//; s/ *$//' > "$REPO/.lucidos/harden-suites/pid-started"
hs verdict > /dev/null
expect_eq "no suites file yet: running" 3 "$?"
hs wait --budget 1 > /dev/null
expect_eq "wait keeps waiting" 3 "$?"
kill "$FAKE" 2> /dev/null
wait "$FAKE" 2> /dev/null
hs verdict > /dev/null
expect_eq "no suites file and no runner: RERUN" 2 "$?"
sleep 5 &
OTHER=$!
echo "$OTHER" > "$REPO/.lucidos/harden-suites/pid"
echo "Thu Jan  1 00:00:00 1970" > "$REPO/.lucidos/harden-suites/pid-started"
out="$(hs stop)"
expect_has "a recycled pid is not the runner" "No suite run is going" "$out"
if kill -0 "$OTHER" 2> /dev/null; then pass "stop left the recycled pid alone"; else fail "stop left the recycled pid alone"; fi
kill "$OTHER" 2> /dev/null
wait "$OTHER" 2> /dev/null

echo "the tree walk"
# shellcheck source=scripts/lib/proc_tree.sh
source "$SCRIPT_DIR/proc_tree.sh"
expect_eq "root, children, grandchildren" "10 11 12 13" \
    "$(printf '11 10\n12 10\n13 11\n99 1\n' | proc_tree_walk 10 | tr '\n' ' ' | sed 's/ $//')"
expect_eq "a cycle ends" "10 11" "$(printf '11 10\n10 11\n' | proc_tree_walk 10 | tr '\n' ' ' | sed 's/ $//')"

echo
echo "harden_suites_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
