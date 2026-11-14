#!/bin/bash
# Tests for the baked-build-script-path gate (ADR 0079): the shared library
# scripts/lib/build_script_path_scan.sh and the CLI
# scripts/check-build-script-paths.sh.
#
# Hermetic: every case runs against a throwaway git repo under mktemp. Nothing
# reads the real tree, so the outcome cannot drift as this repo gains crates,
# and nothing here can touch a running workspace.
#
# Covered: the banned form flagged; the run-time form clean; `option_env!` and
# CARGO_MANIFEST_PATH flagged too; a build script outside `crates/` covered; a
# `build.rs` under `src/` NEVER scanned, since cargo does not run it; the same
# `env!` in ordinary source never flagged; an untracked build script ignored
# until committed; and the CLI's exit status in all three states.
#
# Run: ./scripts/lib/build_script_path_scan_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/build_script_path_scan.sh
source "$SCRIPT_DIR/build_script_path_scan.sh"
CLI="$SCRIPT_DIR/../check-build-script-paths.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# Assertions are named for what they mean, so no case needs `A && B || C`
# (which runs C when B fails) or a bare `$?` read two commands later.
expect_hit() { # <scan-output> <what-was-being-checked>
    if [ -n "$1" ]; then pass "$2"; else fail "$2 (expected a hit, got none)"; fi
}
expect_clean() { # <scan-output> <what-was-being-checked>
    if [ -z "$1" ]; then pass "$2"; else fail "$2 (unexpectedly flagged: $1)"; fi
}
expect_rc() { # <actual-rc> <expected-rc> <what-was-being-checked>
    if [ "$1" -eq "$2" ]; then pass "$3"; else fail "$3 (rc $1, wanted $2)"; fi
}
expect_cannot_run() { # <repo-path> <what-was-being-checked>
    if build_script_path_scan "$1" > /dev/null 2>&1; then
        fail "$2 (reported clean)"
    else
        pass "$2"
    fi
}

REPO="$(mktemp -d)"
trap 'rm -rf "$REPO"' EXIT
git -C "$REPO" init -q -b main
git -C "$REPO" config user.email "t@t"
git -C "$REPO" config user.name "t"

write() { # <relpath> <content>
    mkdir -p "$REPO/$(dirname "$1")"
    printf '%s\n' "$2" > "$REPO/$1"
}

# A build script only counts when a Cargo.toml sits beside it, so every fixture
# crate has to carry one. That sibling IS what the discovery test turns on.
crate_with_build() { # <crate-dir> <build.rs content>
    write "$1/Cargo.toml" '[package]'
    write "$1/build.rs" "$2"
}

commit() { git -C "$REPO" add -A && git -C "$REPO" commit -qm "fixture"; }
reset_repo() { rm -rf "${REPO:?}/crates" "${REPO:?}/tools"; }

# The banned form, spelled once. Every fixture that should fail uses this.
BAKED='    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));'
RUNTIME='    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());'

# Run the CLI inside the fixture repo. Assertions read CLI_RC rather than a
# bare `$?`, which survives exactly one command.
run_cli() {
    CLI_OUT=$(cd "$REPO" && bash "$CLI" 2>&1)
    CLI_RC=$?
}

echo "build_script_path_scan"

# ── The banned form ──────────────────────────────────────────────────────────
reset_repo
crate_with_build "crates/thing" "$BAKED"
commit
OUT="$(build_script_path_scan "$REPO")"
case "$OUT" in
    crates/thing/build.rs:1:*CARGO_MANIFEST_DIR*) pass "a baked manifest dir is flagged, with path and line" ;;
    *) fail "expected a hit for the baked form, got: $OUT" ;;
esac
run_cli
expect_rc "$CLI_RC" 1 "the CLI fails on a baked path"
case "$CLI_OUT" in
    *std::env::var*) pass "the CLI names the run-time replacement" ;;
    *) fail "the CLI failure did not spell the fix: $CLI_OUT" ;;
esac

# ── The run-time form ────────────────────────────────────────────────────────
reset_repo
crate_with_build "crates/thing" "$RUNTIME"
commit
expect_clean "$(build_script_path_scan "$REPO")" "the run-time form is clean"
run_cli
expect_rc "$CLI_RC" 0 "the CLI passes a clean tree"

# ── option_env! bakes just the same ──────────────────────────────────────────
reset_repo
crate_with_build "crates/thing" '    let d = option_env!("CARGO_MANIFEST_DIR");'
commit
expect_hit "$(build_script_path_scan "$REPO")" "option_env! is flagged too"

# ── CARGO_MANIFEST_PATH is the same hazard ───────────────────────────────────
reset_repo
crate_with_build "crates/thing" '    let d = env!("CARGO_MANIFEST_PATH");'
commit
expect_hit "$(build_script_path_scan "$REPO")" "CARGO_MANIFEST_PATH is flagged"

# ── A build script outside crates/ is still a build script ───────────────────
reset_repo
crate_with_build "tools/helper" "$BAKED"
commit
expect_hit "$(build_script_path_scan "$REPO")" "a build script outside crates/ is covered"

# ── A build.rs under src/ is NOT a build script ──────────────────────────────
# This repo really has one (crates/lucidos-engine/src/core/store/messages/build.rs).
# Cargo never runs it, so compile-time env! there is legal and must not block.
reset_repo
crate_with_build "crates/thing" "$RUNTIME"
write "crates/thing/src/core/build.rs" "$BAKED"
commit
expect_clean "$(build_script_path_scan "$REPO")" "a build.rs under src/ is never scanned"

# ── The false positive that matters: ordinary source is never scanned ────────
# `repo_root_or_compile_time_fallback` in the real paths.rs uses exactly this
# form, legitimately, because no cargo variable is set when the engine runs.
reset_repo
crate_with_build "crates/thing" "$RUNTIME"
write "crates/thing/src/paths.rs" "$BAKED"
commit
expect_clean "$(build_script_path_scan "$REPO")" "ordinary source keeping compile-time env! is never flagged"

# ── Untracked files are not yet part of the tree ─────────────────────────────
reset_repo
crate_with_build "crates/thing" "$RUNTIME"
commit
crate_with_build "crates/later" "$BAKED"
expect_clean "$(build_script_path_scan "$REPO")" "an uncommitted build script is not scanned"

# ── Fail closed when discovery finds nothing ─────────────────────────────────
# A repo with no build script at all means discovery is broken as far as this
# gate is concerned. Reporting "clean" would be a claim about files nobody read.
EMPTY="$(mktemp -d)"
git -C "$EMPTY" init -q -b main
expect_cannot_run "$EMPTY" "a tree with no build script fails closed"
rm -rf "$EMPTY"

# ── A non-repo cannot be scanned, and must not read as clean ─────────────────
NOTREPO="$(mktemp -d)"
expect_cannot_run "$NOTREPO" "a non-git directory fails closed"
rm -rf "$NOTREPO"

echo
echo "  $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
