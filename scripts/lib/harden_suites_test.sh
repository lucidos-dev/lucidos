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
# the read check, on fixtures and on this checkout, and the VOID it causes
# naming its files; a fix that widens the selection; failing and zero-test
# suites; the Codex join; the driver run in the background and its join; the
# default wait budget; a run still setting up; start refusals; stop reaching
# a grandchild and sparing a recycled pid; the run staying in the caller's
# process group; a slow, a failing and a stopped touched-spec selection; the
# WebKit specs sent to GitHub; the tests a failed log names; a retry round of
# what failed, and the full run it still owes; the timings JSON; test-only
# includes selecting their test rather than the crate; a dead engine filter;
# the crate graph, the owners of each path and the crates a diff reaches, with
# the suites and commands that follow them; the TEST_CRATES pin.
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

expect_lacks() { # <name> <needle> <haystack>
    case "$3" in
        *"$2"*) fail "$1: [$2] unexpectedly in [$3]" ;;
        *) pass "$1" ;;
    esac
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---------------------------------------------------------------- pure rules

echo "safe paths"
for p in docs/plans/x.md docs/plans/x.png docs/adr/0001-x.md docs/code-review-priors.md docs/temporary-measures.md \
    docs/adrx.md docs/e2e-test-decisions.md; do
    if hs_is_safe_path "$p"; then pass "safe: $p"; else fail "safe: $p"; fi
done
# A test reads the glossary, so it is no safe path.
for p in docs/glossary.md docs/x.txt system-knowhow/a.md crates/a.rs README.md; do
    if hs_is_safe_path "$p"; then fail "unsafe: $p"; else pass "unsafe: $p"; fi
done

echo "the read check on this checkout"
# A file the read check flags turns the allowlist off for every early run, and
# nothing else says so. Pinned here, so the drift fails where it lands.
CHECKOUT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
expect_eq "no file in this checkout turns the allowlist off" "" \
    "$(hs_read_check "$CHECKOUT" | tr '\n' ' ' | sed 's/ $//')"

echo "generated knowhow freshness"
for s in ts vitest knowhow-generated; do
    expect_eq "$s leaves the cargo lane" "1" "$(hs_is_cargo_suite "$s"; echo $?)"
    expect_eq "$s waits for the codegen step" "0" "$(hs_waits_for_codegen "$s"; echo $?)"
    cmd="$(hs_suite_command "$s" normal "$TMP")"
    expect_lacks "$s never runs codegen" "codegen.sh" "$cmd"
    expect_lacks "$s never runs the typecheck script" "npm run typecheck" "$cmd"
    if printf '%s' "$cmd" | grep -q -E 'npm test($| |&)' && ! printf '%s' "$cmd" | grep -q -F 'npm test --ignore-scripts'; then
        fail "$s runs a bare npm test, whose pretest hook runs codegen"
    else
        pass "$s runs no bare npm test"
    fi
done
expect_eq "engine waits for no codegen step" "1" "$(hs_waits_for_codegen engine; echo $?)"
expect_eq "a selection with ts needs the codegen step" "0" "$(hs_needs_codegen "engine
ts"; echo $?)"
expect_eq "a selection without a reader needs no codegen step" "1" "$(hs_needs_codegen "engine
first-paint"; echo $?)"
expect_eq "the codegen step runs codegen" "./scripts/codegen.sh" "$(hs_suite_command codegen-step normal "$TMP")"
expect_has "fast docs still run freshness" "knowhow-generated" "$(printf '%s\n' docs/a.md | hs_select_suites '' '' '' '' fast)"
expect_has "freshness runs even for docs" "knowhow-generated" "$(printf '%s\n' docs/a.md | hs_select_suites)"
expect_eq "freshness compares with the cached rendering alone" "./scripts/check-knowhow-generated.sh --cached-only" "$(hs_suite_command knowhow-generated normal "$TMP")"
expect_has "codegen changes run their fixtures" "codegen" "$(printf '%s\n' scripts/lib/codegen.sh | hs_select_suites)"
expect_eq "the codegen suite runs its hermetic fixtures" "bash scripts/lib/codegen_test.sh && bash scripts/lib/codegen_blocks_test.sh" "$(hs_suite_command codegen normal "$TMP")"
expect_has "block parser edits run codegen fixtures" "codegen" "$(printf '%s\n' scripts/lib/codegen_blocks.py | hs_select_suites)"
expect_has "engine build input edits run codegen fixtures" "codegen" "$(printf '%s\n' crates/lucidos-engine/build.rs | hs_select_suites)"
expect_has "fast engine build edits keep codegen fixtures" "codegen" "$(printf '%s\n' crates/lucidos-engine/build.rs | hs_select_suites '' '' '' '' fast)"

echo "suite selection"
# Path-selection assertions omit the unconditional freshness suite tested above.
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
# A workspace graph shaped like the real one, as `start` records it: the engine
# has two dependents, and the gateway, app and CLI have none.
printf 'lucidos-app\tcrates/lucidos-app\tlucidos-home\nlucidos-capabilities\tcrates/lucidos-capabilities\t\nlucidos-cli\tcrates/lucidos-cli\tlucidos-capabilities lucidos-home\nlucidos-e2e\tcrates/lucidos-e2e\tlucidos-engine lucidos-local-token\nlucidos-engine\tcrates/lucidos-engine\tlucidos-home lucidos-local-token\nlucidos-eval\tcrates/lucidos-eval\tlucidos-engine lucidos-local-token\nlucidos-gateway\tcrates/lucidos-gateway\tlucidos-home\nlucidos-home\tcrates/lucidos-home\t\nlucidos-local-token\tcrates/lucidos-local-token\tlucidos-home\n' > "$TMP/sel-graph"
printf 'CHANGELOG.md\tcrates/lucidos-engine\n' > "$TMP/sel-owners"
ALL_SUITES="lint crates gateway cli app-lib eval engine"
select_cfg() { # [drift-file] [tier]
    hs_select_suites "$TMP/inputs" "$TMP/cli-inputs" "$TMP/vitest-inputs" "${1:-}" "${2:-}" "" "" "$TMP/sel-graph" "$TMP/sel-owners" | sed '/^knowhow-generated$/d' | tr '\n' ' ' | sed 's/ $//'
}
select_for() { printf '%s\n' "$@" | select_cfg; }
expect_eq "an engine edit" "lint eval engine" "$(select_for crates/lucidos-engine/src/a.rs)"
expect_eq "Cargo.lock" "$ALL_SUITES" "$(select_for Cargo.lock)"
expect_eq "Cargo.toml" "lint eval engine" "$(select_for crates/lucidos-engine/Cargo.toml)"
# The files that configure every build, and the fail-safe cases, select every
# crate and lint the whole workspace.
for p in Cargo.lock Cargo.toml .cargo/config.toml rust-toolchain.toml clippy.toml Makefile crates/vendor/x/src/lib.rs; do
    expect_eq "$p selects every crate's suite" "$ALL_SUITES" "$(select_for "$p" crates/lucidos-gateway/src/a.rs)"
done
expect_eq "a .rs outside every member selects every crate" "$ALL_SUITES" "$(select_for tools/x.rs)"
expect_eq "an empty graph selects every crate" "$ALL_SUITES" \
    "$(printf '%s\n' crates/lucidos-gateway/src/a.rs | hs_select_suites "$TMP/inputs" "$TMP/cli-inputs" "$TMP/vitest-inputs" "" "" "" "" /dev/null /dev/null | sed '/^knowhow-generated$/d' | tr '\n' ' ' | sed 's/ $//')"
expect_eq "a missing graph selects every crate" "$ALL_SUITES" \
    "$(printf '%s\n' crates/lucidos-gateway/src/a.rs | hs_select_suites "$TMP/inputs" "" "" "" "" "" "" /nonexistent /nonexistent | sed '/^knowhow-generated$/d' | tr '\n' ' ' | sed 's/ $//')"
expect_eq "a gateway edit skips the engine and every other crate" "lint gateway" "$(select_for crates/lucidos-gateway/src/a.rs)"
expect_eq "a small crate edit tests the small crates and the CLI above it" "lint crates cli" "$(select_for crates/lucidos-capabilities/Cargo.toml)"
expect_eq "a shared crate reaches every crate that depends on it" "lint crates gateway cli app-lib eval engine" \
    "$(select_for crates/lucidos-home/src/a.rs)"
expect_eq "an eval edit tests the eval crate alone" "lint eval" "$(select_for crates/lucidos-eval/src/a.rs)"
expect_eq "an e2e edit lints and runs no suite" "lint" "$(select_for crates/lucidos-e2e/tests/a.rs)"
expect_eq "ORT download policy" "$ALL_SUITES ort-download" "$(select_for crates/vendor/ort-sys/build.rs)"
expect_eq "ORT mirror tests" "shell-lint ort-download" "$(select_for scripts/lib/onnxruntime_mirror_test.sh)"

expect_eq ".shellcheckrc" "shell-lint" "$(select_for .shellcheckrc)"
expect_eq "an app compile input runs the app tests, and the gateway pins it" "lint gateway app-lib first-paint" "$(select_for crates/lucidos-app/tauri.conf.json)"
expect_eq "migration" "lint eval engine" "$(select_for crates/lucidos-engine/migrations/1_x.sql)"
expect_eq "app crate" "lint app-lib" "$(select_for crates/lucidos-app/src/lib.rs)"
expect_eq "cli crate" "lint cli" "$(select_for crates/lucidos-cli/src/main.rs)"
expect_eq "a CLI include runs the CLI tests" "lint cli eval engine" \
    "$(select_for crates/lucidos-engine/src/runtime/cc_menu_options.json)"
expect_eq "no cli-inputs file" "lint eval engine" \
    "$(printf '%s\n' crates/lucidos-engine/src/runtime/cc_menu_options.json | hs_select_suites "$TMP/inputs" "" "" "" "" "" "" "$TMP/sel-graph" "$TMP/sel-owners" | sed '/^knowhow-generated$/d' | tr '\n' ' ' | sed 's/ $//')"
expect_eq "a compiled-in markdown file" "lint eval engine" "$(select_for CHANGELOG.md)"
expect_eq "no compile-inputs file" "" "$(printf '%s\n' CHANGELOG.md | hs_select_suites | sed '/^knowhow-generated$/d' | tr '\n' ' ')"
expect_eq "a capability manifest change runs the CLI tests" "lint crates cli" \
    "$(select_for crates/lucidos-capabilities/src/lib.rs)"
printf 'drift\n' > "$TMP/drift"
expect_eq "generated drift runs the TS suite" "lint eval engine ts" \
    "$(printf '%s\n' crates/lucidos-engine/src/x.rs | select_cfg "$TMP/drift")"
: > "$TMP/no-drift"
expect_eq "no generated drift leaves the TS suite out" "lint eval engine" \
    "$(printf '%s\n' crates/lucidos-engine/src/x.rs | select_cfg "$TMP/no-drift")"

echo "generated drift"
# A stub stands in for scripts/codegen.sh: it answers each fingerprint and
# cached rendering from the environment, and logs every render.
DRIFT_REPO="$TMP/drift-repo"
git init -q -b main "$DRIFT_REPO"
git -C "$DRIFT_REPO" -c user.email=t@example.com -c user.name=t commit -q --allow-empty -m base
cat > "$TMP/codegen-stub" <<'STUB'
#!/bin/bash
case "${1:-}" in
    --fingerprint) if [ -n "${2:-}" ]; then echo "$STUB_BASE_FP"; else echo "$STUB_HEAD_FP"; fi ;;
    --cache-dir)
        if [ -n "${2:-}" ]; then dir=$STUB_BASE_DIR; else dir=$STUB_HEAD_DIR; fi
        [ -d "$dir" ] && echo "$dir" ;;
    "") echo rendered >> "$STUB_LOG" ;;
esac
STUB
chmod +x "$TMP/codegen-stub"
mkdir -p "$TMP/render-base" "$TMP/render-same" "$TMP/render-other"
echo "export const A = 1;" > "$TMP/render-base/a.ts"
echo "export const A = 1;" > "$TMP/render-same/a.ts"
echo "export const A = 2;" > "$TMP/render-other/a.ts"
drift_for() { # <base-fp> <head-fp> <base-dir> <head-dir>
    HS_CODEGEN="$TMP/codegen-stub" STUB_BASE_FP=$1 STUB_HEAD_FP=$2 STUB_BASE_DIR=$3 STUB_HEAD_DIR=$4 \
        STUB_LOG="$TMP/render-log" hs_generated_drift "$DRIFT_REPO" 2>/dev/null
}
: > "$TMP/render-log"
expect_eq "an unchanged fingerprint is no drift" "" "$(drift_for f1 f1 "$TMP/render-base" "$TMP/render-base")"
expect_eq "an unchanged fingerprint renders nothing" "" "$(cat "$TMP/render-log")"
expect_eq "a new fingerprint with the same output is no drift" "" \
    "$(drift_for f1 f2 "$TMP/render-base" "$TMP/render-same")"
expect_eq "a new fingerprint renders the branch" "rendered" "$(cat "$TMP/render-log")"
expect_eq "a changed output is drift" "drift" "$(drift_for f1 f2 "$TMP/render-base" "$TMP/render-other")"
expect_eq "no rendering for the base is drift" "drift" "$(drift_for f1 f2 "$TMP/absent" "$TMP/render-same")"
expect_eq "shell" "shell-lint" "$(select_for scripts/foo.sh)"
expect_eq "Makefile" "$ALL_SUITES" "$(select_for Makefile)"
expect_eq "installer" "shell-lint gateway install" "$(select_for install.sh)"
expect_eq "installer: service.sh" "shell-lint gateway install" "$(select_for scripts/lib/service.sh)"
for f in uninstall.sh scripts/lib/stage_runtime.sh \
    scripts/lib/stage_runtime_test.sh scripts/lib/stage_runtime_linux_smoke.sh \
    scripts/build-headless.sh scripts/lib/install_test.sh \
    scripts/lib/headless_tarball.sh scripts/lib/install_common.sh; do
    expect_eq "installer: $f" "shell-lint install" "$(select_for "$f")"
done
for f in scripts/lib/stage_runtime_linux.py scripts/lib/stage_runtime_linux_test.py; do
    expect_eq "Linux closure: $f" "install" "$(select_for "$f")"
done
expect_eq "a script both crates pin" "shell-lint gateway engine-filtered" \
    "$(select_for scripts/lib/workspace_constants.sh)"
expect_eq "a script the engine pins" "shell-lint engine-filtered" "$(select_for scripts/status.sh)"
expect_eq "a gateway edit subsumes the gateway pins" "lint gateway install" \
    "$(select_for install.sh crates/lucidos-gateway/src/a.rs)"
expect_eq "release script" "shell-lint release" "$(select_for scripts/release.sh)"
expect_eq "preferences" "lint app-lib ts first-paint" "$(select_for crates/lucidos-app/src/store/actions/preferences.ts)"
expect_eq "proc tree" "shell-lint harden-suites" "$(select_for scripts/lib/proc_tree.sh)"
expect_eq "release" "shell-lint release" "$(select_for scripts/lib/release_draft.sh)"
expect_eq "self" "shell-lint harden-suites" "$(select_for scripts/lib/harden_suites.sh)"
expect_eq "harden scope" "shell-lint harden-scope" "$(select_for scripts/harden-scope.sh)"
expect_eq "harden scope test" "shell-lint harden-scope" "$(select_for scripts/lib/harden_scope_test.sh)"
expect_eq "harden other-model review" "shell-lint harden-other-model-review" \
    "$(select_for scripts/harden-other-model-review.sh)"
expect_eq "harden other-model review lib" "shell-lint harden-other-model-review" \
    "$(select_for scripts/lib/other_model_review.sh)"
expect_eq "harden other-model review test" "shell-lint harden-other-model-review" \
    "$(select_for scripts/lib/harden_other_model_review_test.sh)"
expect_eq "ts" "ts first-paint" "$(select_for crates/lucidos-app/src/a.ts)"
expect_eq "css" "vite first-paint" "$(select_for crates/lucidos-app/src/a.css)"
expect_eq "sdk iframe css" "lint eval engine vitest" "$(select_for crates/lucidos-engine/src/api/sdk_iframe.css)"
expect_eq "a Rust file a Vitest test reads" "lint eval engine vitest" \
    "$(select_for crates/lucidos-engine/src/engine/event_bus_system_event.rs)"
expect_eq "a Rust file no Vitest test reads" "lint eval engine" "$(select_for crates/lucidos-engine/src/engine/other.rs)"
expect_eq "a rule a Vitest test reads" "vitest" "$(select_for .claude/rules/frontend.md)"
expect_eq "a deleted file under a named directory" "lint eval engine vitest" \
    "$(select_for crates/lucidos-engine/src/api/gone.rs)"
expect_eq "ts already runs Vitest" "lint eval engine ts first-paint" \
    "$(select_for crates/lucidos-engine/src/engine/event_bus_system_event.rs crates/lucidos-app/src/a.ts)"

echo "fast tier selection (an integrated sub-thread)"
select_fast() { printf '%s\n' "$@" | select_cfg "" fast; }
expect_eq "fast: an engine edit runs the lint, not the engine suite" "lint" "$(select_fast crates/lucidos-engine/src/a.rs)"
expect_eq "fast: the app crate drops its tests" "lint" "$(select_fast crates/lucidos-app/src/lib.rs)"
expect_eq "fast: the cli crate drops its tests" "lint" "$(select_fast crates/lucidos-cli/src/main.rs)"
expect_eq "fast: ts keeps tsc, Vitest and vite build, drops first-paint" "ts" "$(select_fast crates/lucidos-app/src/a.ts)"
expect_eq "fast: css keeps vite build" "vite" "$(select_fast crates/lucidos-app/src/a.css)"
expect_eq "fast: a rule a Vitest test reads keeps Vitest" "vitest" "$(select_fast .claude/rules/frontend.md)"
expect_eq "fast: shell keeps the shell lint" "shell-lint" "$(select_fast scripts/foo.sh)"
expect_eq "fast: the installer drops its suites" "shell-lint" "$(select_fast install.sh)"
expect_eq "fast: the release path drops its suites" "shell-lint" "$(select_fast scripts/release.sh)"
expect_eq "fast: an engine pin drops the filtered engine tests" "shell-lint" "$(select_fast scripts/status.sh)"
expect_eq "fast: knowhow drops the filtered engine tests" "" "$(select_fast system-knowhow/a.md)"
expect_eq "fast: the script tests stay" "shell-lint harden-scope" "$(select_fast scripts/harden-scope.sh)"
expect_eq "fast: mixed engine and ts" "lint ts" \
    "$(select_fast crates/lucidos-engine/src/a.rs crates/lucidos-app/src/a.ts)"
expect_eq "full is the default tier" "lint eval engine" \
    "$(printf '%s\n' crates/lucidos-engine/src/a.rs | select_cfg "")"
expect_eq "fast: generated drift keeps the TS suite" "lint ts" \
    "$(printf '%s\n' crates/lucidos-engine/src/a.rs | select_cfg "$TMP/drift" fast)"
expect_eq "an unknown tier is the full tier" "full" "$(hs_normalize_tier banana)"
expect_eq "fast normalizes to fast" "fast" "$(hs_normalize_tier fast)"

echo "vitest inputs, scanned from this checkout"
CHECKOUT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VI="$(hs_vitest_inputs "$CHECKOUT")"
expect_has "the event enum the coverage test reads" "crates/lucidos-engine/src/engine/event_bus_system_event.rs" "$VI"
expect_has "the stylesheet the CSS parse test reads" "crates/lucidos-engine/src/api/sdk_iframe.css" "$VI"
expect_lacks "no path escapes the checkout" "../" "$VI"
VI_CONFIGURED="$(GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=grep.lineNumber GIT_CONFIG_VALUE_0=true \
    GIT_CONFIG_KEY_1=grep.column GIT_CONFIG_VALUE_1=true hs_vitest_inputs "$CHECKOUT")"
expect_eq "a git config cannot change the scan" "$VI" "$VI_CONFIGURED"
expect_eq "ts subsumes css" "ts first-paint" "$(select_for crates/lucidos-app/src/a.ts crates/lucidos-app/src/a.css)"
expect_eq "locale" "lint app-lib ts first-paint" \
    "$(select_for crates/lucidos-app/src/components/settings/LocaleSection.tsx)"
expect_eq "knowhow" "engine-filtered" "$(select_for system-knowhow/a.md)"
expect_eq "an engine edit subsumes filters" "lint eval engine" "$(select_for system-knowhow/a.md crates/lucidos-engine/src/a.rs)"
expect_eq "docs only" "" "$(select_for docs/a.md docs/b.md)"
expect_eq "an SDK source" "ts first-paint" "$(select_for packages/lucidos-sdk/src/a.ts)"
expect_eq "the lockfile" "first-paint" "$(select_for package-lock.json)"
expect_eq "a system widget" "first-paint" "$(select_for system-widgets/a/index.html)"
expect_eq "an app Rust file is no first-paint input" "lint app-lib" "$(select_for crates/lucidos-app/src/a.rs)"
# The row must cover every root the growth check reads, so read them from it.
FP_ROOTS="$(sed -n "s/^export const ARCHIVE_PATHS = \[\(.*\)\];$/\1/p" "$CHECKOUT/crates/lucidos-app/first-paint-growth.mjs" | tr -d "' " | tr ',' ' ')"
expect_has "the growth check's roots were read" "crates/lucidos-app" "$FP_ROOTS"
for root in $FP_ROOTS; do
    expect_has "first-paint root $root selects the suite" "first-paint" "$(select_for "$root" "$root/a.js")"
done

echo "engine filters"
expect_eq "knowhow filters" \
    "always_loaded_context_stays_under_budget system_knowhow_descriptions_stay_routing_sized value_pins_tests" \
    "$(printf '%s\n' crates/a.ts system-knowhow/a.md | hs_engine_filters)"
expect_eq "an engine-pinned script runs the pins alone" "value_pins_tests" \
    "$(printf '%s\n' scripts/status.sh | hs_engine_filters)"
expect_eq "no knowhow, no filters" "" "$(printf '%s\n' crates/a.ts | hs_engine_filters)"

echo "pinned paths, scanned from this checkout"
# Each list must name exactly the non-knowhow files its pin test reads, so a
# new pin row cannot go unselected.
pin_test_paths() { # <rust file>
    grep -o -E '"(scripts/[^"]+|install\.sh|README\.md|crates/[^"]+)"' "$CHECKOUT/$1" | tr -d '"' | sort -u | tr '\n' ' '
}
sorted_words() { # <space-separated list>
    printf '%s\n' "$1" | tr ' ' '\n' | sed '/^$/d' | sort -u | tr '\n' ' '
}
expect_eq "HS_GATEWAY_PINNED names what the gateway pins read" "$(sorted_words "$HS_GATEWAY_PINNED")" \
    "$(pin_test_paths crates/lucidos-gateway/src/value_pins_tests.rs)"
expect_eq "HS_ENGINE_PINNED names what the engine pins read" "$(sorted_words "$HS_ENGINE_PINNED")" \
    "$(pin_test_paths crates/lucidos-engine/src/engine/value_pins_tests.rs)"

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

echo "test-only includes"
# A path every include site of which is a test fn selects that test. Any
# production site keeps it a production input, which selects the Rust suite.
INC="$TMP/inc"
mkdir -p "$INC/crates/lucidos-engine/src" "$INC/crates/lucidos-cli/src" "$INC/crates/other/src" "$INC/docs"
: > "$INC/crates/lucidos-engine/Cargo.toml"
: > "$INC/crates/lucidos-cli/Cargo.toml"
: > "$INC/crates/other/Cargo.toml"
cat > "$INC/crates/lucidos-engine/src/a.rs" << 'EOF'
const MODULE: &str = include_str!("../../../docs/module-const.md");

fn helper() -> &'static str {
    include_str!("../../../docs/helper.md")
}

pub fn production() -> usize {
    let text = include_str!("../../../docs/mixed.md");
    text.len()
}

#[cfg(test)]
mod tests {
    const IN_TESTS_MOD: &str = include_str!("../../../docs/tests-mod-const.md");

    /// A doc comment between the attribute and the fn.
    #[test]
    fn reads_the_test_doc() {
        assert!(include_str!("../../../docs/test-only.md").contains("x"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reads_it_too() {
        let checks = [
            include_str!("../../../docs/test-only.md"),
            include_str!("../../../docs/mixed.md"),
        ];
        assert!(!checks.is_empty());
    }

    #[test]
    fn a_const_in_a_test_fn() {
        const LOCAL: &str = include_str!("../../../docs/fn-const.md");
        assert!(!LOCAL.is_empty());
    }

    #[test]
    fn names_a_build_input() {
        assert!(!include_str!("../../../docs/built.md").is_empty());
    }
}
EOF
echo 'fn main() { println!("cargo:rerun-if-changed=../../docs/built.md"); }' > "$INC/crates/lucidos-engine/build.rs"
cat > "$INC/crates/lucidos-cli/src/c.rs" << 'EOF'
#[test]
fn the_cli_reads_it() {
    assert!(!include_str!("../../../docs/cli-test.md").is_empty());
}
EOF
cat > "$INC/crates/other/src/o.rs" << 'EOF'
#[test]
fn a_crate_without_a_suite() {
    assert!(!include_str!("../../../docs/other.md").is_empty());
}
EOF
cat > "$INC/crates/lucidos-engine/src/main.rs" << 'EOF'
#[test]
fn a_bin_target_test() {
    assert!(!include_str!("../../../docs/bin-test.md").is_empty());
}
EOF
git -C "$INC" init -q && git -C "$INC" add -A
site_of() { hs_include_sites "$INC" | awk -F '\t' -v p="docs/$1" '$1 == p { print $2 " " $3 }' | tr '\n' ',' | sed 's/,$//'; }
expect_eq "a module-level const is production" "prod " "$(site_of module-const.md)"
expect_eq "a helper fn is production" "prod " "$(site_of helper.md)"
expect_eq "a const in a test module is production" "prod " "$(site_of tests-mod-const.md)"
expect_eq "a test fn names itself" "test reads_it_too,test reads_the_test_doc" "$(site_of test-only.md)"
expect_eq "a const inside a test fn is the test's" "test a_const_in_a_test_fn" "$(site_of fn-const.md)"
expect_eq "a mixed path keeps both sites" "prod ,test reads_it_too" "$(site_of mixed.md)"
expect_eq "a test outside the lib target counts as production" "prod a_bin_target_test" "$(site_of bin-test.md)"
hs_test_inputs "$INC" > "$TMP/test-inputs"
expect_eq "only test-only paths in suites of their own are test inputs" \
    "docs/cli-test.md cli | docs/fn-const.md engine-filtered a_const_in_a_test_fn | docs/test-only.md engine-filtered reads_it_too | docs/test-only.md engine-filtered reads_the_test_doc" \
    "$(tr '\t' ' ' < "$TMP/test-inputs" | sed 's/ $//' | awk '{ printf "%s%s", (NR > 1 ? " | " : ""), $0 }')"
hs_source_includes "$INC" > "$TMP/inc-inputs"
select_inc() { printf '%s\n' "$@" | hs_select_suites "$TMP/inc-inputs" '' '' '' '' '' "$TMP/test-inputs" | sed '/^knowhow-generated$/d' | tr '\n' ' ' | sed 's/ $//'; }
for doc in module-const.md helper.md tests-mod-const.md mixed.md built.md other.md bin-test.md; do
    expect_eq "docs/$doc selects every crate, no owner lists it" "$ALL_SUITES" "$(select_inc "docs/$doc")"
done
expect_eq "a .tsx plus a test-only doc runs that test, not the crate" "engine-filtered ts first-paint" \
    "$(select_inc crates/lucidos-app/src/a.tsx docs/test-only.md)"
expect_eq "the filter is each including test fn" "reads_it_too reads_the_test_doc" \
    "$(printf '%s\n' docs/test-only.md | hs_engine_filters "$TMP/test-inputs")"
expect_eq "knowhow filters come first" \
    "always_loaded_context_stays_under_budget system_knowhow_descriptions_stay_routing_sized value_pins_tests a_const_in_a_test_fn" \
    "$(printf '%s\n' docs/fn-const.md system-knowhow/a.md | hs_engine_filters "$TMP/test-inputs")"
expect_eq "a test-only CLI include runs the CLI suite" "cli" "$(select_inc docs/cli-test.md)"
expect_eq "a test-only include still voids an early result" "docs/test-only.md" \
    "$(printf '%s\n' docs/test-only.md | hs_unsafe_paths "$TMP/inc-inputs" 0)"

echo "test-only includes on this checkout"
# The diff that prompted this: a frontend fix plus a reworded plugins.md.
hs_source_includes "$CHECKOUT" > "$TMP/checkout-inputs"
hs_test_inputs "$CHECKOUT" > "$TMP/checkout-test-inputs"
plugins_selection="$(printf '%s\n' crates/lucidos-app/src/components/plugins/PluginDetailPage.tsx system-knowhow/plugins.md \
    | hs_select_suites "$TMP/checkout-inputs" '' '' '' '' '' "$TMP/checkout-test-inputs")"
expect_eq "plugins.md no longer selects the lint or the engine suite" "" \
    "$(printf '%s\n' "$plugins_selection" | grep -x -e lint -e engine)"
expect_has "it selects the filtered engine run" "engine-filtered" "$plugins_selection"
expect_has "and the TS suite" "ts" "$plugins_selection"
expect_has "the plugins.md test fns are filters" "the_plugins_knowhow_states_the_limits" \
    "$(printf '%s\n' system-knowhow/plugins.md | hs_engine_filters "$TMP/checkout-test-inputs")"

echo "the crate graph"
# A fixture workspace: home <- token <- engine <- eval, e2e (dev); home <- gateway, app, cli;
# caps <- cli (build). `cargo metadata` is a stub, so no cargo runs.
GRAPH_ROOT="$TMP/graph-root"
mkdir -p "$GRAPH_ROOT"
package_json() { # <name> [<dependency>[:<kind>]]...
    local name=$1 deps="" d dep kind
    shift
    for d in "$@"; do
        dep=${d%%:*}
        kind=null
        [ "$dep" = "$d" ] || kind="\"${d#*:}\""
        deps="$deps${deps:+,}{\"name\":\"$dep\",\"path\":\"$GRAPH_ROOT/crates/$dep\",\"kind\":$kind}"
    done
    deps="$deps${deps:+,}{\"name\":\"serde\",\"kind\":null}"
    printf '{"name":"%s","manifest_path":"%s/crates/%s/Cargo.toml","dependencies":[%s]}' "$name" "$GRAPH_ROOT" "$name" "$deps"
}
{
    printf '{"workspace_root":"%s","packages":[' "$GRAPH_ROOT"
    {
        package_json home
        package_json token home
        package_json caps
        package_json engine token home
        package_json eval engine token
        package_json e2e engine:dev token:dev
        package_json gateway home
        package_json app home
        package_json cli home caps:build
    } | sed 's/}{/},{/g'
    printf ']}'
} > "$TMP/metadata.json"
export HS_CARGO_METADATA_CMD="cat $TMP/metadata.json"
hs_crate_graph "$GRAPH_ROOT" > "$TMP/graph"
expect_eq "the graph lists each member with its directory and path dependencies" \
    "$(printf 'app\tcrates/app\thome|caps\tcrates/caps\t|cli\tcrates/cli\tcaps home|e2e\tcrates/e2e\tengine token|engine\tcrates/engine\thome token|eval\tcrates/eval\tengine token|gateway\tcrates/gateway\thome|home\tcrates/home\t|token\tcrates/token\thome')" \
    "$(tr '\n' '|' < "$TMP/graph" | sed 's/|$//')"
expect_eq "a failed cargo gives an empty graph" "" "$(HS_CARGO_METADATA_CMD="exit 1" hs_crate_graph "$GRAPH_ROOT")"
expect_eq "an unparsable answer gives an empty graph" "" "$(HS_CARGO_METADATA_CMD="echo not json" hs_crate_graph "$GRAPH_ROOT")"
unset HS_CARGO_METADATA_CMD

echo "compile owners"
# A dep-info file belongs to the member whose own sources it lists, whatever its name.
OWNERS_DEPS="$TMP/owners-target/debug/deps"
mkdir -p "$OWNERS_DEPS"
cat > "$OWNERS_DEPS/engine-abc.d" << DEPINFO
$GRAPH_ROOT/target/debug/deps/libengine-abc.rmeta: crates/engine/src/lib.rs crates/engine/src/../../../CHANGELOG.md /outside/registry/serde/src/lib.rs $GRAPH_ROOT/crates/engine/migrations/1_x.sql

crates/engine/src/lib.rs:
CHANGELOG.md:
DEPINFO
# An integration test and a build script carry names that are no crate's.
cat > "$OWNERS_DEPS/api_flow-def.d" << DEPINFO
$GRAPH_ROOT/target/debug/deps/api_flow-def: crates/e2e/tests/api_flow.rs crates/e2e/tests/../../../docs/shared.txt
DEPINFO
cat > "$OWNERS_DEPS/build_script_build-123.d" << DEPINFO
$GRAPH_ROOT/target/debug/build/cli-1/build_script_build-123: crates/cli/build.rs crates/cli/../caps/manifest.toml
DEPINFO
git init -q "$GRAPH_ROOT"
mkdir -p "$GRAPH_ROOT/crates/gateway/src"
printf 'const A: &str = include_str!("../../../README.md");\n' > "$GRAPH_ROOT/crates/gateway/src/lib.rs"
printf '[package]\n' > "$GRAPH_ROOT/crates/gateway/Cargo.toml"
git -C "$GRAPH_ROOT" add -A
owners="$(CARGO_TARGET_DIR="$TMP/owners-target" hs_compile_owners "$GRAPH_ROOT" "$TMP/graph")"
expect_has "a compile input belongs to the member whose sources list it" "$(printf 'CHANGELOG.md\tcrates/engine')" "$owners"
expect_has "an integration test's dep-info belongs to its crate" "$(printf 'docs/shared.txt\tcrates/e2e')" "$owners"
expect_has "a build script's dep-info belongs to its crate" "$(printf 'crates/caps/manifest.toml\tcrates/cli')" "$owners"
expect_lacks "a path outside the checkout is no owner row" "/outside/registry" "$owners"
expect_lacks "a crate's own sources are no owner rows" "crates/engine/src/lib.rs" "$owners"
expect_lacks "a crate's own migration is no owner row" "migrations/1_x.sql" "$owners"
expect_has "an include site names its crate with no build" "$(printf 'README.md\tcrates/gateway')" "$owners"
expect_eq "no build and no graph still lists the include sites" "$(printf 'README.md\tcrates/gateway')" \
    "$(CARGO_TARGET_DIR="$TMP/no-target" hs_compile_owners "$GRAPH_ROOT" /nonexistent)"
printf '%s\n' "$owners" > "$TMP/owners"

echo "reached crates"
: > "$TMP/no-inputs"
reach() { printf '%s\n' "$@" | hs_reached_crates "${REACH_INPUTS:-$TMP/no-inputs}" "${REACH_TESTS:-$TMP/no-inputs}" "${REACH_GRAPH:-$TMP/graph}" "$TMP/owners" | tr '\n' ' ' | sed 's/ $//'; }
expect_eq "a gateway edit reaches only the gateway" "gateway" "$(reach crates/gateway/src/a.rs)"
expect_eq "an engine edit reaches its dependents" "e2e engine eval" "$(reach crates/engine/src/a.rs)"
expect_eq "a leaf crate manifest reaches its closure" "app" "$(reach crates/app/Cargo.toml)"
expect_eq "a migration belongs to its member" "e2e engine eval" "$(reach crates/engine/migrations/2_y.sql)"
expect_eq "the closure runs through each level" "app cli e2e engine eval gateway home token" "$(reach crates/home/src/a.rs)"
expect_eq "a build dependency counts as an edge" "caps cli" "$(reach crates/caps/src/a.rs)"
expect_eq "a dev dependency counts as an edge" "e2e" "$(reach crates/e2e/tests/x.rs)"
expect_eq "two edits reach the union" "app gateway" "$(reach crates/gateway/src/a.rs crates/app/src/b.rs)"
expect_eq "a non-Rust path reaches nothing" "" "$(reach README.md docs/plans/x.md crates/app/src/a.ts)"
for p in Cargo.lock Cargo.toml .cargo/config.toml rust-toolchain.toml clippy.toml Makefile crates/vendor/x/src/lib.rs; do
    expect_eq "$p reaches every crate" "*" "$(reach "$p" crates/gateway/src/a.rs)"
done
expect_eq "a .rs outside every member reaches every crate" "*" "$(reach tools/x.rs)"
expect_eq "a Cargo.toml outside every member reaches every crate" "*" "$(reach signers/x/Cargo.toml)"
expect_eq "a migration outside every member reaches every crate" "*" "$(reach docs/plans/x.sql)"
expect_eq "an empty graph reaches every crate" "*" "$(REACH_GRAPH=/dev/null reach crates/gateway/src/a.rs)"
expect_eq "a missing graph reaches every crate" "*" "$(REACH_GRAPH=/nonexistent reach crates/gateway/src/a.rs)"
expect_eq "an empty graph with no Rust path reaches nothing" "" "$(REACH_GRAPH=/dev/null reach README.md)"
printf '%s\n' CHANGELOG.md docs/shared.txt docs/unowned.txt > "$TMP/reach-inputs"
REACH_INPUTS="$TMP/reach-inputs"
expect_eq "a compile input reaches the closure of its owner" "e2e engine eval" "$(reach CHANGELOG.md)"
expect_eq "a compile input no owner lists reaches every crate" "*" "$(reach docs/unowned.txt)"
printf 'CHANGELOG.md\tengine-filtered\tsome_test\n' > "$TMP/reach-tests"
REACH_TESTS="$TMP/reach-tests"
expect_eq "a test-only include is no Rust path" "" "$(reach CHANGELOG.md)"
unset REACH_INPUTS REACH_TESTS

echo "dead filters"
printf '%s\n' 'running 3 tests' 'test core::a::alpha_works ... ok' 'test core::b::beta_skipped ... ignored' \
    'test core::c::gamma ... FAILED' > "$TMP/filtered.log"
expect_eq "every filter matched" "" "$(hs_dead_filters "$TMP/filtered.log" alpha_works core::a)"
expect_eq "an ignored or failed test is no match" "beta_skipped gamma" \
    "$(hs_dead_filters "$TMP/filtered.log" alpha beta_skipped gamma)"

echo "suite commands"
# A run's state directory, as `start` leaves it.
CMD_STATE="$TMP/cmd-state"
mkdir -p "$CMD_STATE"
: > "$CMD_STATE/paths"
: > "$CMD_STATE/touched-specs"
echo abc123 > "$CMD_STATE/start-sha"
command_for() { HS_STUB_CMD='' hs_suite_command "$1" "$2" "$CMD_STATE"; }
early_engine="$(command_for engine early)"
expect_has "early engine skips codex driver" "--skip runtime::codex::driver_tests" "$early_engine"
expect_has "early engine skips app-server driver" "--skip runtime::codex_app_server::driver_tests" "$early_engine"
expect_eq "early engine passes the skips after the filter separators" "./scripts/test-engine.sh -- -- --skip runtime::codex::driver_tests --skip runtime::codex_app_server::driver_tests" "$early_engine"
expect_eq "normal engine skips nothing" "./scripts/test-engine.sh" "$(command_for engine normal)"
cp "$TMP/sel-graph" "$CMD_STATE/crate-graph"
cp "$TMP/sel-owners" "$CMD_STATE/compile-owners"
: > "$CMD_STATE/compile-inputs"
: > "$CMD_STATE/test-inputs"
paths_are() { printf '%s\n' "$@" > "$CMD_STATE/paths"; }
paths_are crates/lucidos-gateway/src/a.rs
expect_eq "a gateway edit lints the gateway alone" "make lint CLIPPY_SCOPE='-p lucidos-gateway'" "$(command_for lint normal)"
paths_are crates/lucidos-engine/src/a.rs
expect_eq "an engine edit lints its dependents too" "make lint CLIPPY_SCOPE='-p lucidos-e2e -p lucidos-engine -p lucidos-eval'" "$(command_for lint normal)"
paths_are crates/lucidos-home/src/a.rs
expect_eq "a leaf-dependency edit lints each crate that reaches it" \
    "make lint CLIPPY_SCOPE='-p lucidos-app -p lucidos-cli -p lucidos-e2e -p lucidos-engine -p lucidos-eval -p lucidos-gateway -p lucidos-home -p lucidos-local-token'" "$(command_for lint normal)"
paths_are crates/lucidos-home/src/a.rs crates/lucidos-capabilities/src/a.rs
expect_eq "reaching every member lints the workspace" "make lint" "$(command_for lint normal)"
for p in Cargo.lock Cargo.toml .cargo/config.toml rust-toolchain.toml clippy.toml Makefile crates/vendor/x/src/lib.rs; do
    paths_are "$p"
    expect_eq "$p lints the workspace" "make lint" "$(command_for lint normal)"
    expect_eq "$p tests every small crate" "make test-crates" "$(command_for crates normal)"
done
paths_are
expect_eq "a state with no reach lints the workspace" "make lint" "$(command_for lint normal)"
paths_are crates/lucidos-capabilities/src/a.rs
expect_eq "a small crate edit tests that crate" "cargo test --locked -p lucidos-capabilities" "$(command_for crates normal)"
paths_are crates/lucidos-home/src/a.rs
expect_eq "the small crates a diff reaches are tested" "cargo test --locked -p lucidos-home -p lucidos-local-token" "$(command_for crates normal)"
expect_eq "the eval suite runs the eval crate" "cargo test --locked -p lucidos-eval" "$(command_for eval normal)"
expect_eq "a shell-only diff runs the cheap linters" "make lint-shell lint-eval lint-knowhow lint-fmt" "$(command_for shell-lint normal)"
if hs_is_cargo_suite lint; then pass "lint shares the cargo lane"; else fail "lint shares the cargo lane"; fi
expect_has "driver names both modules" \
    "runtime::codex::driver_tests runtime::codex_app_server::driver_tests" \
    "$(command_for driver early)"
printf '%s\n' system-knowhow/a.md > "$CMD_STATE/paths"
expect_has "engine-filtered carries filters" "-- -- always_loaded" "$(command_for engine-filtered normal)"
expect_eq "gateway runs the gateway crate" "cargo test --locked -p lucidos-gateway" \
    "$(command_for gateway normal)"
if hs_is_cargo_suite gateway; then pass "gateway shares the cargo lane"; else fail "gateway shares the cargo lane"; fi
expect_eq "first-paint runs the growth check" "cd crates/lucidos-app && node first-paint-growth.mjs check" \
    "$(command_for first-paint normal)"
if hs_is_cargo_suite first-paint; then fail "first-paint gets its own lane"; else pass "first-paint gets its own lane"; fi

echo "the touched browser specs"
printf '%s\n' chat.spec.ts rail.spec.ts > "$TMP/touched"
: > "$TMP/touched-none"
select_touched() { # <touched-file> <tier> <paths...>
    local touched=$1 tier=$2
    shift 2
    printf '%s\n' "$@" | hs_select_suites "$TMP/inputs" "$TMP/cli-inputs" "$TMP/vitest-inputs" /dev/null "$tier" "$touched" "" "$TMP/sel-graph" "$TMP/sel-owners" | sed '/^knowhow-generated$/d' | tr '\n' ' ' | sed 's/ $//'
}
expect_eq "a touched spec selects e2e-touched" "ts first-paint e2e-touched" \
    "$(select_touched "$TMP/touched" full crates/lucidos-app/src/a.ts)"
expect_eq "a spec change alone selects it" "ts first-paint e2e-touched" \
    "$(select_touched "$TMP/touched" full crates/lucidos-app/e2e/chat.spec.ts)"
expect_eq "no touched spec selects none" "ts first-paint" \
    "$(select_touched "$TMP/touched-none" full crates/lucidos-app/src/a.ts)"
expect_eq "a backend-only diff selects no e2e" "lint eval engine" \
    "$(select_touched "$TMP/touched-none" full crates/lucidos-engine/src/a.rs)"
expect_eq "the fast tier leaves the specs to the parent" "ts" \
    "$(select_touched "$TMP/touched" fast crates/lucidos-app/src/a.ts)"
cp "$TMP/touched" "$CMD_STATE/touched-specs"
expect_eq "each touched spec is one -f" "./scripts/e2e-browser.sh -f chat.spec.ts -f rail.spec.ts" \
    "$(command_for e2e-touched normal)"
if hs_is_cargo_suite e2e-touched; then fail "e2e-touched gets its own lane"; else pass "e2e-touched gets its own lane"; fi

echo "the touched specs mobile-webkit runs go to GitHub"
printf '%s\n' chat.spec.ts nav-mobile.spec.ts rail-all.spec.ts > "$TMP/touched-phone"
expect_eq "a phone spec selects the WebKit suite too" "ts first-paint e2e-touched e2e-touched-webkit" \
    "$(select_touched "$TMP/touched-phone" full crates/lucidos-app/src/a.ts)"
expect_eq "the fast tier drops the WebKit suite" "ts" \
    "$(select_touched "$TMP/touched-phone" fast crates/lucidos-app/src/a.ts)"
expect_eq "only the specs mobile-webkit runs" "nav-mobile.spec.ts rail-all.spec.ts" \
    "$(hs_webkit_specs < "$TMP/touched-phone" | tr '\n' ' ' | sed 's/ $//')"
cp "$TMP/touched-phone" "$CMD_STATE/touched-specs"
expect_eq "the WebKit suite runs on GitHub, pinned to the start commit" \
    "./scripts/e2e-browser.sh --github --webkit --commit abc123 -f nav-mobile.spec.ts -f rail-all.spec.ts" \
    "$(command_for e2e-touched-webkit normal)"
expect_eq "the local suite still runs every touched spec" \
    "./scripts/e2e-browser.sh -f chat.spec.ts -f nav-mobile.spec.ts -f rail-all.spec.ts" \
    "$(command_for e2e-touched normal)"
if hs_is_cargo_suite e2e-touched-webkit; then fail "e2e-touched-webkit gets its own lane"; else pass "e2e-touched-webkit gets its own lane"; fi
expect_eq "the project rule selects the harden-suites tests" "shell-lint harden-suites" \
    "$(select_for scripts/lib/e2e_projects.sh)"

echo "a retried suite runs only what failed"
printf '%s\n' engine::a::tests::x engine::b::tests::y > "$CMD_STATE/engine.retry"
expect_eq "engine tests by exact name" "./scripts/test-engine.sh -- -- --exact engine::a::tests::x engine::b::tests::y" \
    "$(command_for engine early)"
cp "$CMD_STATE/engine.retry" "$CMD_STATE/engine-filtered.retry"
expect_eq "a filtered engine suite reruns by name too" \
    "./scripts/test-engine.sh -- -- --exact engine::a::tests::x engine::b::tests::y" "$(command_for engine-filtered normal)"
echo rail-all.spec.ts > "$CMD_STATE/e2e-touched.retry"
expect_eq "the local specs that failed" "./scripts/e2e-browser.sh -f rail-all.spec.ts" "$(command_for e2e-touched normal)"
echo rail-all.spec.ts > "$CMD_STATE/e2e-touched-webkit.retry"
expect_eq "the WebKit specs that failed" "./scripts/e2e-browser.sh --github --webkit --commit abc123 -f rail-all.spec.ts" \
    "$(command_for e2e-touched-webkit normal)"
echo whatever > "$CMD_STATE/ts.retry"
expect_eq "a suite with no named tests reruns whole" \
    "cd crates/lucidos-app && npx tsc --noEmit && npm test --ignore-scripts && npx vite build" "$(command_for ts normal)"
rm -f "$CMD_STATE"/*.retry

echo "the tests a failed suite names"
cat > "$TMP/cargo.log" << 'EOF'
     Running unittests src/lib.rs (target/debug/deps/lucidos_engine-0a1b2c3d4e5f6789)
running 3 tests
test engine::a::tests::x ... FAILED
test engine::a::tests::ok ... ok
test engine::b::tests::y ... FAILED

failures:

---- engine::a::tests::x stdout ----
panicked

failures:
    engine::a::tests::x
    engine::b::tests::y

test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
EOF
expect_eq "engine lib failures by name" "engine::a::tests::x engine::b::tests::y" \
    "$(hs_failed_tests engine "$TMP/cargo.log" | tr '\n' ' ' | sed 's/ $//')"
cat >> "$TMP/cargo.log" << 'EOF'
     Running unittests src/lib.rs (target/debug/deps/lucidos_gateway-0a1b2c3d4e5f6789)
test proxy::tests::z ... FAILED
EOF
expect_has "a failure outside the engine lib cannot be named" "?" "$(hs_failed_tests engine "$TMP/cargo.log")"
cat > "$TMP/pw.log" << 'EOF'
  1) [chromium] › e2e/chat.spec.ts:10:5 › sends ──────
  2 failed
    [chromium] › e2e/chat.spec.ts:10:5 › sends a message
    [mobile] › e2e/rail-all.spec.ts:4:3 › opens
  1 flaky
    [chromium] › e2e/board.spec.ts:2:3 › flakes
  71 passed (3.5m)
EOF
expect_eq "failed specs by file, flaky ones left out" "chat.spec.ts rail-all.spec.ts" \
    "$(hs_failed_tests e2e-touched "$TMP/pw.log" | tr '\n' ' ' | sed 's/ $//')"
expect_eq "a suite with no named tests names none" "" "$(hs_failed_tests ts "$TMP/pw.log")"

echo "the retry plan"
RETRY_ROOT="$TMP/retry-root"
mkdir -p "$RETRY_ROOT/crates/lucidos-app/e2e" "$TMP/retry-state"
touch "$RETRY_ROOT/crates/lucidos-app/e2e/chat.spec.ts"
plan="$(printf '%s\n' "engine engine::a::tests::x" "ts" "e2e-touched chat.spec.ts" "e2e-touched gone.spec.ts" \
    "e2e-touched-webkit gone-all.spec.ts" "engine engine::b::tests::y" | hs_retry_plan "$RETRY_ROOT" "$TMP/retry-state")"
expect_eq "each suite once, a suite whose specs are gone dropped" "engine ts e2e-touched" "$(printf '%s' "$plan" | tr '\n' ' ' | sed 's/ $//')"
expect_eq "the engine tests to rerun" "engine::a::tests::x engine::b::tests::y" \
    "$(tr '\n' ' ' < "$TMP/retry-state/engine.retry" | sed 's/ $//')"
expect_eq "a gone spec is dropped" "chat.spec.ts" "$(cat "$TMP/retry-state/e2e-touched.retry")"
if [ -f "$TMP/retry-state/ts.retry" ]; then fail "a whole suite has no retry list"; else pass "a whole suite has no retry list"; fi
# A record an earlier script left names `rust`, which no suite runs now.
mkdir -p "$TMP/stale-state"
plan="$(printf '%s\n' "rust engine::a::tests::x" "rust" "ts" | hs_retry_plan "$RETRY_ROOT" "$TMP/stale-state")"
expect_eq "a record naming a retired suite is dropped" "ts" "$(printf '%s' "$plan" | tr '\n' ' ' | sed 's/ $//')"
if [ -f "$TMP/stale-state/rust.retry" ]; then fail "a retired suite keeps no retry list"; else pass "a retired suite keeps no retry list"; fi
plan="$(printf '%s\n' "rust engine::a::tests::x" | hs_retry_plan "$RETRY_ROOT" "$TMP/stale-state")"
expect_eq "a record of retired suites alone leaves nothing to retry" "" "$plan"
expect_eq "the selector's own files select the harden-suites tests" "shell-lint harden-suites" \
    "$(select_for scripts/lib/touched_specs.sh)"

echo "every workspace crate's tests run in a suite"
# `make lint` compiles every crate's tests and runs none, so each crate needs a
# suite that runs them. Reads the real Makefile through `make -n`.
REAL_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
suite_cmds="$(make -s -n -C "$REAL_ROOT" test 2> /dev/null)
$(command_for app-lib normal)
$(command_for cli normal)"
while read -r crate; do
    case "$crate" in
        lucidos-engine) needle="test-engine.sh" ;;
        lucidos-e2e) continue ;; # the e2e scripts run it against a live engine
        *) needle="-p $crate" ;;
    esac
    expect_has "$crate is tested" "$needle" "$suite_cmds"
done < <(sed -n '/^members = \[/,/^\]/p' "$REAL_ROOT/Cargo.toml" | grep -o 'crates/[a-z0-9-]*' | sed 's|crates/||')
# `make test-crates` and the `crates` suite cover the same set: every member
# without a suite of its own. A new member must land in TEST_CRATES.
members="$(sed -n '/^members = \[/,/^\]/p' "$REAL_ROOT/Cargo.toml" | grep -o 'crates/[a-z0-9-]*' | sed 's|crates/||' | sort)"
small_members="$(for m in $members; do hs_is_listed "$m" "$HS_SUITE_CRATES" || echo "$m"; done)"
makefile_crates="$(make -s -n -C "$REAL_ROOT" test-crates 2> /dev/null | tr ' ' '\n' | grep '^lucidos-' | sort)"
expect_eq "the Makefile's TEST_CRATES are the members the script calls small" "$small_members" "$makefile_crates"

# ---------------------------------------------------------------- fixture repo

REPO="$TMP/repo"
STUB_DIR="$TMP/stub"
mkdir -p "$REPO" "$STUB_DIR"
git -C "$REPO" init -q -b main
git -C "$REPO" config user.email "t@t"
git -C "$REPO" config user.name "t"
printf '.lucidos/\ntarget/\n' > "$REPO/.gitignore"
mkdir -p "$REPO/crates/lucidos-engine/src" "$REPO/crates/lucidos-app/src" "$REPO/docs/plans" "$REPO/scripts"
# A stable fingerprint keeps suite fixtures independent of real Cargo writers.
printf '#!/bin/sh\necho fixture-fingerprint\n' > "$REPO/scripts/codegen.sh"
chmod +x "$REPO/scripts/codegen.sh"
echo "fn a() {}" > "$REPO/crates/lucidos-engine/src/a.rs"
echo "export {}" > "$REPO/crates/lucidos-app/src/a.ts"
git -C "$REPO" add -A && git -C "$REPO" commit -qm base
git -C "$REPO" checkout -q -b feature
echo "fn a() { 1; }" > "$REPO/crates/lucidos-engine/src/a.rs"
echo "export const x = 1" > "$REPO/crates/lucidos-app/src/a.ts"
git -C "$REPO" add -A && git -C "$REPO" commit -qm change
ROOT="$(git -C "$REPO" rev-parse --show-toplevel)"
# The fixture repo's two crates, as `cargo metadata` would list them: no edges,
# so an engine edit reaches the engine alone. A stub, so no cargo runs.
printf '{"workspace_root":"%s","packages":[{"name":"lucidos-engine","manifest_path":"%s/crates/lucidos-engine/Cargo.toml","dependencies":[]},{"name":"lucidos-app","manifest_path":"%s/crates/lucidos-app/Cargo.toml","dependencies":[]}]}' "$REPO" "$REPO" "$REPO" > "$TMP/repo-metadata.json"
export HS_CARGO_METADATA_CMD="cat $TMP/repo-metadata.json"

# The stub reads its behaviour per suite from $STUB_DIR: <suite>.rc (exit
# code), <suite>.sleep (seconds), <suite>.child (spawn a grandchild),
# <suite>.killlane (SIGKILL the cargo lane, from the run's state directory).
cat > "$STUB_DIR/stub.sh" << 'EOF'
#!/bin/bash
s=$1
d=$(dirname "$0")
echo "start $s" >> "$d/events"
ps -o pgid= -p $$ | tr -d ' ' > "$d/$s.pgid"
echo ran >> "$d/$s.ran"
if [ -f "$d/$s.child" ]; then
    sleep 300 &
    echo $! > "$d/$s.childpid"
fi
[ -f "$d/$s.say" ] && cat "$d/$s.say"
[ -f "$d/$s.killlane" ] && kill -9 "$(cat .lucidos/harden-suites/codegen-step.pid)"
[ -f "$d/$s.sleep" ] && sleep "$(cat "$d/$s.sleep")"
[ "$s" = driver ] && [ ! -f "$d/driver.notests" ] && echo "running 3 tests"
echo "end $s" >> "$d/events"
exit "$(cat "$d/$s.rc" 2>/dev/null || echo 0)"
EOF
chmod +x "$STUB_DIR/stub.sh"
export HS_STUB_CMD="$STUB_DIR/stub.sh"
# No engine answers here, through a stub CLI that prints nothing, so `start`
# never asks a real engine. HARDEN_TIER then names the tier.
cat > "$STUB_DIR/lucidos.sh" << 'EOF3'
#!/bin/bash
cat "$(dirname "$0")/lucidos.answer" 2>/dev/null
exit 0
EOF3
chmod +x "$STUB_DIR/lucidos.sh"
export HS_LUCIDOS_CMD="$STUB_DIR/lucidos.sh"
export HARDEN_TIER=full

# The build memory gate, stubbed so no test reads the real host. It records
# each label it was asked about and exits with the code in gate.rc.
cat > "$STUB_DIR/gate.sh" << 'EOF2'
#!/bin/bash
d=$(dirname "$0")
printf '%s\n' "$2" >> "$d/gate.calls"
exit "$(cat "$d/gate.rc" 2>/dev/null || echo 0)"
EOF2
chmod +x "$STUB_DIR/gate.sh"
export HS_MEMORY_GATE_CMD="$STUB_DIR/gate.sh"

hs() { (cd "$REPO" && bash "$CLI" "$@"); }
reset_stub() { rm -f "$STUB_DIR"/*.rc "$STUB_DIR"/*.sleep "$STUB_DIR"/*.child "$STUB_DIR"/*.ran \
    "$STUB_DIR"/*.pgid "$STUB_DIR"/*.childpid "$STUB_DIR"/*.killlane "$STUB_DIR"/driver.notests "$STUB_DIR"/gate.calls "$STUB_DIR"/*.say \
    "$STUB_DIR"/select.fail "$STUB_DIR"/select.out "$STUB_DIR"/events "$TMP"/repo*/.lucidos/harden-retry; }
commit_file() { # <relpath> <content>
    mkdir -p "$REPO/$(dirname "$1")"
    echo "$2" > "$REPO/$1"
    git -C "$REPO" add -- "$1" && git -C "$REPO" commit -qm "$1"
}

echo "early run, clean"
hs start --early > "$TMP/out" 2>&1
expect_eq "start exits 0 once its suites ran" 0 "$?"
expect_has "selects lint, engine and ts" "Suites (early): lint engine ts" "$(cat "$TMP/out")"
out="$(hs wait --budget 20)"
rc=$?
expect_eq "wait exit" 0 "$rc"
expect_has "driver ran alone" "driver PASS" "$out"
expect_has "engine passes" "engine PASS" "$out"
expect_has "ts passes" "ts PASS" "$out"
expect_eq "run shares the caller's process group" "$(ps -o pgid= -p $$ | tr -d ' ')" "$(cat "$STUB_DIR/engine.pgid")"

expect_eq "the gate was asked before the codegen step and each cargo suite only" \
    "the /harden codegen step
the /harden lint suite
the /harden engine suite
the /harden driver suite" "$(cat "$STUB_DIR/gate.calls")"

# Overlap check on the stub's event log: a cargo-lane entry starts only after
# the previous one ended, and a codegen reader starts only after the step ended.
lane_overlaps() { # <events-file>
    awk '
        { kind = $1; s = $2 }
        s == "codegen-step" || s == "lint" || s == "engine" || s == "driver" {
            if (kind == "start") { if (open != "") print open " overlaps " s; open = s }
            else if (open == s) open = ""
        }
        kind == "end" && s == "codegen-step" { codegen_done = 1 }
        kind == "start" && (s == "ts" || s == "vitest" || s == "knowhow-generated") && !codegen_done {
            print s " started before the codegen step ended"
        }
    ' "$1"
}
expect_eq "the codegen step ran once" "1" "$(grep -c -x 'start codegen-step' "$STUB_DIR/events")"
expect_eq "no lane entry overlaps another, and readers start after codegen" "" "$(lane_overlaps "$STUB_DIR/events")"
expect_eq "the codegen step leads the lane" "start codegen-step" "$(grep -E '^start (codegen-step|engine)$' "$STUB_DIR/events" | head -1)"

echo "a slow codegen step holds its readers and the lane"
reset_stub
echo 2 > "$STUB_DIR/codegen-step.sleep"
echo 1 > "$STUB_DIR/engine.sleep"
hs start --early > /dev/null 2>&1
hs wait --budget 20 > /dev/null
expect_eq "a slow step still passes" 0 "$?"
expect_eq "no lane entry overlaps another, and readers start after codegen" "" "$(lane_overlaps "$STUB_DIR/events")"

echo "a failed codegen step fails its readers"
reset_stub
echo 1 > "$STUB_DIR/codegen-step.rc"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "a failed step fails the run" 1 "$?"
expect_has "ts fails" "ts FAIL" "$out"
expect_has "freshness fails" "knowhow-generated FAIL" "$out"
expect_has "engine still runs" "engine PASS" "$out"
expect_has "the reader's log points at the step's log" "codegen-step.log" "$(cat "$REPO/.lucidos/harden-suites/ts.log")"
if [ -f "$STUB_DIR/ts.ran" ]; then fail "ts ran on a failed codegen step"; else pass "ts never ran"; fi
rm -f "$REPO/.lucidos/harden-retry"

echo "a lane killed mid-codegen releases its readers"
reset_stub
touch "$STUB_DIR/codegen-step.killlane"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "a killed lane asks for a rerun" 2 "$?"
expect_has "the reader is missing, not running" "ts MISSING" "$out"
if [ -f "$STUB_DIR/ts.ran" ]; then fail "ts ran without a codegen step"; else pass "ts never ran"; fi

echo "a refused memory gate starts no cargo suite"
reset_stub
echo 72 > "$STUB_DIR/gate.rc"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
rc=$?
expect_eq "a refused suite asks for a rerun" 2 "$rc"
expect_has "names the gate" "engine VOID refused by the build memory gate" "$out"
expect_has "a refused codegen step voids its readers" "ts VOID refused by the build memory gate" "$out"
if [ -f "$STUB_DIR/engine.ran" ]; then fail "the engine suite ran on a refused gate"; else pass "the engine suite never ran"; fi
if [ -f "$STUB_DIR/codegen-step.ran" ]; then fail "codegen ran on a refused gate"; else pass "codegen never ran"; fi
if [ -f "$STUB_DIR/first-paint.ran" ]; then pass "a non-cargo suite still ran"; else fail "the first-paint suite did not run"; fi
rm -f "$STUB_DIR/gate.rc"
hs start --early > /dev/null 2>&1
hs wait --budget 20 > /dev/null
expect_eq "a fresh run after recovery passes" 0 "$?"

echo "a gate refused inside make reads VOID, not FAIL"
reset_stub
echo 2 > "$STUB_DIR/engine.rc"
echo "ERROR: build refused on host memory: engine tests did not start" > "$STUB_DIR/engine.say"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "a nested refusal asks for a rerun" 2 "$?"
expect_has "names the gate" "engine VOID refused by the build memory gate" "$out"
reset_stub
hs start --early > /dev/null 2>&1
hs wait --budget 20 > /dev/null
expect_eq "the clean run the next cases build on passes" 0 "$?"

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
echo 3 > "$STUB_DIR/engine.sleep"
hs start --early > /dev/null 2>&1 &
BG=$!
# The edit waits for the engine suite to start: an earlier edit fails the start.
for _ in $(seq 1 100); do [ -f "$STUB_DIR/engine.ran" ] && break; sleep 0.2; done
cp "$REPO/crates/lucidos-engine/src/a.rs" "$TMP/a.rs.bak"
echo "fn a() { 3; }" > "$REPO/crates/lucidos-engine/src/a.rs"
wait $BG
cp "$TMP/a.rs.bak" "$REPO/crates/lucidos-engine/src/a.rs"
out="$(hs wait --budget 20)"
rc=$?
expect_eq "mid-run edit: RERUN" 2 "$rc"
expect_has "engine voided by its exit stamp" "engine VOID crates/lucidos-engine/src/a.rs" "$out"
if [ -f "$STUB_DIR/driver.ran" ]; then fail "driver skipped after a void"; else pass "driver skipped after a void"; fi

echo "a failing suite"
reset_stub
echo 1 > "$STUB_DIR/ts.rc"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "failing ts: FAIL" 1 "$?"
expect_has "names ts" "ts FAIL" "$out"

echo "a failed round reruns what failed first, then the full set"
reset_stub
printf '%s\n' "     Running unittests src/lib.rs (target/debug/deps/lucidos_engine-0a1b2c3d4e5f6789)" \
    "test engine::a::tests::x ... FAILED" "" "failures:" "    engine::a::tests::x" "" \
    "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out" > "$STUB_DIR/engine.say"
echo 101 > "$STUB_DIR/engine.rc"
echo 1 > "$STUB_DIR/ts.rc"
hs start > /dev/null 2>&1
hs wait --budget 20 > /dev/null
expect_eq "the first round fails" 1 "$?"
expect_eq "the record names the engine test, and ts whole" "engine engine::a::tests::x
ts" "$(cat "$REPO/.lucidos/harden-retry")"
timings="$(hs timings)"
expect_has "timings say the round" '"round": "full", "mode": "normal"' "$timings"
expect_has "timings carry each suite's exit" '"exit": 101}' "$timings"
if printf '%s' "$timings" | python3 -c 'import json, sys; json.load(sys.stdin)' 2>/dev/null; then
    pass "timings are JSON"
else
    fail "timings are JSON: $timings"
fi
timing_check="$(printf '%s' "$timings" | python3 -c '
import json, sys
t = json.load(sys.stdin)
ts, step = t["suites"]["ts"], t["codegen_step"]
print("ts queued" if ts["queued"] <= ts["start"] else "ts queued after start")
print("step ran" if step["queued"] <= step["start"] <= step["end"] and step["exit"] == 0 else "step %s" % step)
print("ts after step" if ts["start"] >= step["end"] else "ts before step")
' 2>&1)"
expect_eq "timings carry queued, and the codegen step the readers waited for" "ts queued
step ran
ts after step" "$timing_check"
rm -f "$STUB_DIR/engine.rc" "$STUB_DIR/ts.ran" "$STUB_DIR/engine.ran"
echo "running 1 test" > "$STUB_DIR/engine.say"
hs start > "$TMP/out" 2>&1
expect_has "the next start is a retry" "Suites (normal, a retry of what failed): engine ts" "$(cat "$TMP/out")"
expect_eq "the retry keeps the engine test's name" "engine::a::tests::x" "$(cat "$REPO/.lucidos/harden-suites/engine.retry")"
out="$(hs wait --budget 20)"
expect_eq "ts still fails, so the retry fails" 1 "$?"
expect_eq "the record keeps only what still fails" "ts" "$(cat "$REPO/.lucidos/harden-retry")"
expect_has "timings say it was a retry" '"round": "retry"' "$(hs timings)"
rm -f "$STUB_DIR/ts.rc"
hs start > "$TMP/out" 2>&1
expect_has "the retry runs only ts" "Suites (normal, a retry of what failed): ts" "$(cat "$TMP/out")"
out="$(hs wait --budget 20)"
expect_eq "a retry that passes never passes the harden" 2 "$?"
expect_has "says the full set runs next" "What failed now passes" "$out"
if [ -f "$REPO/.lucidos/harden-retry" ]; then fail "a passing retry clears the record"; else pass "a passing retry clears the record"; fi
hs start > "$TMP/out" 2>&1
expect_has "then the full set runs" "Suites (normal): lint engine ts" "$(cat "$TMP/out")"
hs wait --budget 20 > /dev/null
expect_eq "and passes" 0 "$?"
rm -f "$STUB_DIR/engine.say"
echo engine > "$REPO/.lucidos/harden-retry"
hs start > "$TMP/out" 2>&1
expect_has "a retry of engine alone" "a retry of what failed): engine" "$(cat "$TMP/out")"
expect_has "no reader, so no codegen step in the timings" '"codegen_step": null' "$(hs timings)"
hs wait --budget 20 > /dev/null

echo "an early run of a retry round has no driver step"
reset_stub
echo "engine engine::a::tests::x" > "$REPO/.lucidos/harden-retry"
echo "running 1 test" > "$STUB_DIR/engine.say"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "the early retry passes to a rerun" 2 "$?"
if [ -f "$STUB_DIR/driver.ran" ]; then fail "a retry round ran the driver"; else pass "a retry round runs no driver"; fi

echo "a retried driver run waits for the other-model review, then runs once"
reset_stub
printf '%s\n' "engine engine::a::tests::x" "driver runtime::codex::driver_tests::y" > "$REPO/.lucidos/harden-retry"
echo "running 1 test" > "$STUB_DIR/engine.say"
mkdir -p "$REPO/.lucidos" && echo "{}" > "$REPO/.lucidos/other-model-review.out"
hs start > "$TMP/out" 2>&1
expect_has "the retry names the driver" "a retry of what failed): engine driver" "$(cat "$TMP/out")"
if [ -f "$STUB_DIR/driver.ran" ]; then fail "start held the driver back"; else pass "start held the driver back"; fi
hs wait --budget 20 > /dev/null
expect_eq "review still running: wait says running" 3 "$?"
if [ -f "$STUB_DIR/driver.ran" ]; then fail "wait held the driver back"; else pass "wait held the driver back"; fi
out="$(hs wait --budget 20 --review-abandoned)"
expect_eq "the retry passes to a rerun" 2 "$?"
expect_eq "the retried driver ran once" "ran" "$(cat "$STUB_DIR/driver.ran")"
rm -f "$REPO/.lucidos/other-model-review.out" "$STUB_DIR/engine.say"

echo "a retried driver run alone still starts"
reset_stub
echo "driver runtime::codex::driver_tests::y" > "$REPO/.lucidos/harden-retry"
hs start > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "a driver-only retry passes to a rerun" 2 "$?"
expect_eq "the driver ran once" "ran" "$(cat "$STUB_DIR/driver.ran")"

echo "a retried engine run that matched no test"
reset_stub
echo "engine engine::gone::tests::x" > "$REPO/.lucidos/harden-retry"
hs start > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "zero retried tests: FAIL" 1 "$?"
expect_has "says so" "engine FAIL ran no tests" "$out"

echo "a driver run with no tests"
reset_stub
touch "$STUB_DIR/driver.notests"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "no driver tests: FAIL" 1 "$?"
expect_has "says so" "driver FAIL ran no tests" "$out"

echo "the other-model review join"
reset_stub
mkdir -p "$REPO/.lucidos" && echo "{}" > "$REPO/.lucidos/other-model-review.out"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "review still running: wait says running" 3 "$?"
if [ -f "$STUB_DIR/driver.ran" ]; then fail "driver held back"; else pass "driver held back"; fi
out="$(hs verdict)"
expect_eq "no driver run yet: RERUN, never PASS" 2 "$?"
expect_has "says so" "driver MISSING" "$out"
hs wait --budget 20 --review-abandoned > /dev/null
expect_eq "abandoned review: driver runs" 0 "$?"
touch "$REPO/.lucidos/other-model-review.done"

echo "the driver runs in the background and wait joins it"
reset_stub
# Long enough that a loaded host's verdict call still sees it running.
echo 10 > "$STUB_DIR/driver.sleep"
hs start --early > /dev/null 2>&1
out="$(hs wait --budget 1)"
expect_eq "a driver run past the budget: still running" 3 "$?"
expect_has "says the driver is what it waits on" "Codex driver tests are still running" "$out"
# It runs in the background, so give it a moment to write its first line.
for _ in $(seq 1 50); do [ -f "$STUB_DIR/driver.ran" ] && break; sleep 0.1; done
if [ -f "$STUB_DIR/driver.ran" ]; then pass "the driver started"; else fail "the driver started"; fi
out="$(hs verdict)"
expect_eq "a running driver is never a verdict" 3 "$?"
expect_has "says so" "driver RUNNING" "$out"
hs start --early > /dev/null 2>&1
expect_eq "a running driver refuses a new start" 1 "$?"
out="$(hs wait --budget 20)"
expect_eq "the next wait joins the driver" 0 "$?"
expect_has "driver passes" "driver PASS" "$out"
expect_eq "the driver ran once" "ran" "$(cat "$STUB_DIR/driver.ran")"
reset_stub
echo 30 > "$STUB_DIR/driver.sleep"
hs start --early > /dev/null 2>&1
hs wait --budget 1 > /dev/null
hs stop > /dev/null
out="$(hs verdict)"
expect_eq "a stopped driver: RERUN" 2 "$?"
expect_has "says stopped" "driver VOID stopped" "$out"

echo "the default wait budget"
# A pin, since a shell value has no import: the budget must return inside the
# Claude Code cache lifetime of 5 minutes, with margin for the next request.
if [ "$HS_WAIT_BUDGET" -le 230 ]; then pass "the default budget fits a cache window"; else fail "budget $HS_WAIT_BUDGET over 230 s"; fi
expect_has "the usage names the default" "default $HS_WAIT_BUDGET)" "$(bash "$CLI" --help)"

echo "a normal run has no driver step"
reset_stub
hs start > /dev/null 2>&1
out="$(hs wait --budget 20)"
expect_eq "normal run: PASS" 0 "$?"
case "$out" in
    *driver*) fail "no driver line in a normal run" ;;
    *) pass "no driver line in a normal run" ;;
esac

echo "a fast-tier run (an integrated sub-thread)"
reset_stub
HARDEN_TIER=fast hs start --early > "$TMP/out" 2>&1
expect_has "says the tier and runs the lint in place of the engine suite" \
    "Suites (early, fast tier): lint ts" "$(cat "$TMP/out")"
out="$(hs wait --budget 20)"
expect_eq "fast run: PASS" 0 "$?"
expect_has "the lint passes" "lint PASS" "$out"
case "$out" in
    *driver* | *"engine "*) fail "a fast run runs no engine suite" ;;
    *) pass "a fast run runs no engine suite" ;;
esac
expect_eq "the verdict reads the recorded tier, not the caller's" 0 \
    "$(cd "$REPO" && HARDEN_TIER=full bash "$CLI" verdict > /dev/null 2>&1; echo $?)"

echo "the engine's tier wins over HARDEN_TIER"
reset_stub
echo full > "$STUB_DIR/lucidos.answer"
HARDEN_TIER=fast hs start --early > "$TMP/out" 2>&1
expect_has "a full branch keeps its suites whatever HARDEN_TIER says" \
    "Suites (early): lint engine ts" "$(cat "$TMP/out")"
hs wait --budget 20 > /dev/null
rm -f "$STUB_DIR/lucidos.answer"
expect_eq "engine fast" fast "$(hs_resolve_tier fast full)"
expect_eq "engine full beats HARDEN_TIER=fast" full "$(hs_resolve_tier full fast)"
expect_eq "an unknown answer is full" full "$(hs_resolve_tier maybe fast)"
expect_eq "no answer takes HARDEN_TIER" fast "$(hs_resolve_tier "" fast)"
expect_eq "no answer and no HARDEN_TIER is full" full "$(hs_resolve_tier "" "")"

echo "start refusals and stop"
reset_stub
echo "dirty" >> "$REPO/crates/lucidos-engine/src/a.rs"
hs start --early > /dev/null 2>&1
expect_eq "dirty tree refused" 1 "$?"
git -C "$REPO" checkout -q -- crates/lucidos-engine/src/a.rs
echo 30 > "$STUB_DIR/engine.sleep"
touch "$STUB_DIR/engine.child"
hs start --early > /dev/null 2>&1 &
BG=$!
for _ in $(seq 1 50); do [ -f "$STUB_DIR/engine.childpid" ] && break; sleep 0.1; done
hs start --early > /dev/null 2>&1
expect_eq "second start refused" 1 "$?"
child="$(cat "$STUB_DIR/engine.childpid")"
hs stop > /dev/null
wait $BG 2> /dev/null
if kill -0 "$child" 2> /dev/null; then fail "stop reached the grandchild"; kill "$child"; else pass "stop reached the grandchild"; fi
if kill -0 "$BG" 2> /dev/null; then fail "stop ended the runner"; else pass "stop ended the runner"; fi
out="$(hs verdict)"
expect_eq "stopped run: RERUN" 2 "$?"
expect_has "says stopped" "engine VOID stopped" "$out"
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
out="$(hs wait --budget 20)"
expect_eq "allowlist off: RERUN on a plan edit" 2 "$?"
expect_has "the VOID names what turned the allowlist off" \
    "VOID docs/plans/q.md changed since the start commit (the safe-path allowlist is off: crates/lucidos-engine/src/reads.rs may read a safe path)" "$out"
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

echo "a docs-only diff selects freshness"
git -C "$REPO" checkout -q -b docs-only main
commit_file docs/readme-extra.md words
out="$(hs start --early 2>&1)"
expect_has "docs still select freshness" "knowhow-generated" "$out"
hs verdict > /dev/null
expect_eq "docs freshness: PASS" 0 "$?"
commit_file crates/lucidos-engine/src/fix.rs "fn fix() {}"
out="$(hs verdict)"
expect_eq "a fix that widens the selection: RERUN" 2 "$?"
expect_has "names the new selection" "selection VOID the diff now selects: lint engine" "$out"

echo "an edit to a compiled-in file selects Rust with no build"
git -C "$REPO" checkout -q main
mkdir -p "$REPO/crates/lucidos-engine/src/engine"
commit_file crates/lucidos-engine/Cargo.toml ""
commit_file crates/lucidos-engine/src/engine/changelog.rs 'const C: &str = include_str!("../../../../CHANGELOG.md");'
commit_file CHANGELOG.md "v1"
git -C "$REPO" checkout -q -b changelog-only
commit_file CHANGELOG.md "v2"
out="$(hs start --early 2>&1)"
expect_has "the changelog edit selects engine" "Suites (early): lint engine" "$out"
hs stop > /dev/null

echo "touched browser specs, through the CLI"
# A second repo, so the runs above keep their state. It carries an app with one
# spec, and a branch that renames a label the spec names.
REPO2="$TMP/repo2"
git init -q -b main "$REPO2"
git -C "$REPO2" config user.email "t@t"
git -C "$REPO2" config user.name "t"
printf '.lucidos/\n' > "$REPO2/.gitignore"
mkdir -p "$REPO2/crates/lucidos-app/src" "$REPO2/crates/lucidos-app/e2e" "$REPO2/crates/lucidos-engine/src"
echo 'export const label = "Open board";' > "$REPO2/crates/lucidos-app/src/a.ts"
echo 'test("t", async ({ page }) => { await page.getByText("Open board").click(); });' > "$REPO2/crates/lucidos-app/e2e/board.spec.ts"
echo "fn a() {}" > "$REPO2/crates/lucidos-engine/src/a.rs"
git -C "$REPO2" add -A && git -C "$REPO2" commit -qm base
git -C "$REPO2" checkout -q -b feature
hs2() { (cd "$REPO2" && HS_CODEGEN=/usr/bin/true bash "$CLI" "$@"); }
echo "fn a() { 1; }" > "$REPO2/crates/lucidos-engine/src/a.rs"
git -C "$REPO2" commit -qam backend
reset_stub
hs2 start > "$TMP/out" 2>&1
expect_has "a backend-only branch selects no e2e" "Suites (normal): lint engine" "$(cat "$TMP/out")"
if grep -q e2e-touched "$TMP/out"; then fail "a backend-only branch selected e2e"; else pass "a backend-only branch selected no e2e"; fi
echo 'export const label = "Open canvas";' > "$REPO2/crates/lucidos-app/src/a.ts"
git -C "$REPO2" commit -qam rename
reset_stub
echo 1 > "$STUB_DIR/e2e-touched.rc"
hs2 start > "$TMP/out" 2>&1
expect_has "the rename selects the spec's suite" "e2e-touched" "$(cat "$TMP/out")"
expect_eq "the spec is recorded" "board.spec.ts" "$(cat "$REPO2/.lucidos/harden-suites/touched-specs")"
out="$(hs2 wait --budget 20)"
expect_eq "a failing touched spec fails the verdict" 1 "$?"
expect_has "the row names the failure" "e2e-touched FAIL" "$out"
expect_has "the suite log says there is no coverage map yet" "coverage map: none yet" \
    "$(cat "$REPO2/.lucidos/harden-suites/e2e-touched.log")"
reset_stub
hs2 start > /dev/null 2>&1
out="$(hs2 wait --budget 20)"
expect_eq "a passing touched spec passes" 0 "$?"
expect_has "the row passes" "e2e-touched PASS" "$out"
reset_stub
echo "$E2E_LOCK_BUSY_EXIT" > "$STUB_DIR/e2e-touched.rc"
hs2 start > /dev/null 2>&1
out="$(hs2 wait --budget 20)"
expect_eq "a busy e2e lock passes the verdict (ADR 0541)" 0 "$?"
expect_has "the row says it skipped, and why" "e2e-touched SKIP every e2e slot was busy" "$out"
reset_stub
echo "$E2E_LOCK_BUSY_EXIT" > "$STUB_DIR/engine.rc"
hs2 start > /dev/null 2>&1
out="$(hs2 wait --budget 20)"
expect_eq "the busy exit from a non-e2e suite still fails" 1 "$?"
expect_has "and reads FAIL" "engine FAIL" "$out"
reset_stub
echo 'export const label = "Open canvas now";' > "$REPO2/crates/lucidos-app/src/a.ts"
git -C "$REPO2" commit -qam again
hs2 verdict > /dev/null
expect_eq "a later frontend edit voids it" 2 "$?"

echo "a slow touched-spec selection"
# The selector stub stands in for ts_touched_specs: it sleeps for select.sleep
# seconds, fails when select.fail exists, else prints select.out.
cat > "$STUB_DIR/select.sh" << 'EOF4'
#!/bin/bash
d=$(dirname "$0")
[ -f "$d/select.sleep" ] && sleep "$(cat "$d/select.sleep")"
[ -f "$d/select.fail" ] && exit 1
cat "$d/select.out" 2> /dev/null
exit 0
EOF4
chmod +x "$STUB_DIR/select.sh"
export HS_TOUCHED_SPECS_CMD="$STUB_DIR/select.sh"
STATE2="$REPO2/.lucidos/harden-suites"
reset_stub
echo board.spec.ts > "$STUB_DIR/select.out"
echo 3 > "$STUB_DIR/select.sleep"
hs2 start > "$TMP/out" 2>&1 &
BG=$!
for _ in $(seq 1 50); do [ -f "$STUB_DIR/engine.ran" ] && break; sleep 0.1; done
if [ -f "$STUB_DIR/engine.ran" ]; then pass "a cargo suite starts while the selection runs"; else fail "a cargo suite starts while the selection runs"; fi
if [ -f "$STUB_DIR/e2e-touched.ran" ]; then fail "e2e-touched waits for the selection"; else pass "e2e-touched waits for the selection"; fi
if [ -f "$STATE2/suites" ]; then fail "the suites file waits for the selection"; else pass "the suites file waits for the selection"; fi
out="$(hs2 verdict)"
expect_eq "a run still selecting is never a verdict" 3 "$?"
expect_has "says it is setting up" "the run is still setting up" "$out"
wait $BG
expect_has "the full selection is announced" "Suites (normal): lint engine ts first-paint e2e-touched" "$(cat "$TMP/out")"
if [ -f "$STUB_DIR/e2e-touched.ran" ]; then pass "e2e-touched starts once the specs are selected"; else fail "e2e-touched starts once the specs are selected"; fi
expect_eq "the specs are recorded" "board.spec.ts" "$(cat "$STATE2/touched-specs")"
expect_eq "each suite ran once" "ran" "$(cat "$STUB_DIR/engine.ran")"
out="$(hs2 wait --budget 20)"
expect_eq "the verdict passes" 0 "$?"
expect_has "e2e-touched passes" "e2e-touched PASS" "$out"

echo "a failing touched-spec selection"
reset_stub
echo 2 > "$STUB_DIR/select.sleep"
touch "$STUB_DIR/select.fail" "$STUB_DIR/engine.child"
echo 30 > "$STUB_DIR/engine.sleep"
hs2 start > "$TMP/out" 2>&1 &
BG=$!
for _ in $(seq 1 50); do [ -f "$STUB_DIR/engine.childpid" ] && break; sleep 0.1; done
wait $BG
expect_eq "start exits 1" 1 "$?"
expect_has "says what failed" "Could not select the touched browser specs" "$(cat "$TMP/out")"
child="$(cat "$STUB_DIR/engine.childpid")"
if kill -0 "$child" 2> /dev/null; then fail "a launched suite outlived the failed start"; kill "$child"; else pass "no launched suite outlived the failed start"; fi
if [ -f "$STATE2/suites" ]; then fail "no suites file is written"; else pass "no suites file is written"; fi
hs2 verdict > /dev/null
expect_eq "a failed start leaves no verdict to trust" 2 "$?"

echo "a stop during the touched-spec selection"
reset_stub
echo 5 > "$STUB_DIR/select.sleep"
touch "$STUB_DIR/engine.child"
echo 30 > "$STUB_DIR/engine.sleep"
hs2 start > /dev/null 2>&1 &
BG=$!
for _ in $(seq 1 50); do [ -f "$STUB_DIR/engine.childpid" ] && break; sleep 0.1; done
hs2 stop > /dev/null
wait $BG 2> /dev/null
child="$(cat "$STUB_DIR/engine.childpid")"
if kill -0 "$child" 2> /dev/null; then fail "stop reached the suite started before the selection ended"; kill "$child"; else pass "stop reached the suite started before the selection ended"; fi
if kill -0 "$BG" 2> /dev/null; then fail "stop ended the runner"; else pass "stop ended the runner"; fi
hs2 verdict > /dev/null
expect_eq "a stopped selection leaves no verdict to trust" 2 "$?"
unset HS_TOUCHED_SPECS_CMD

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
