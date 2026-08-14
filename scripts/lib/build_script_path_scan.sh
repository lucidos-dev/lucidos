#!/usr/bin/env bash
# Canonical detection for a build script that BAKES a checkout path. The single
# source of truth for the deterministic side of the rule; sourced by:
#   - scripts/check-build-script-paths.sh          (gate, /harden Phase 4.5)
#   - scripts/lib/build_script_path_scan_test.sh   (its test)
#
# WHAT IS BANNED, AND WHY IT IS ONLY BANNED HERE
#
# `env!("CARGO_MANIFEST_DIR")` expands when the build script is COMPILED, so the
# resulting binary remembers the checkout it was built in. Two checkouts of one
# package share a `-C metadata` hash, so with a shared `CARGO_TARGET_DIR` cargo
# considers the first checkout's artifact fresh for the second and hands it
# over. The baked path then names somebody else's tree, or a deleted one.
#
# Read the same variable at RUN time (`std::env::var`) and the question does not
# arise: cargo sets it when it runs the script, so a reused binary is correct.
#
# Scoped to cargo build scripts, deliberately. Compile-time `env!` is right in
# ordinary crate code, where no cargo variable is set at run time at all (see
# `repo_root_or_compile_time_fallback` in crates/lucidos-engine/src/paths.rs).
# Widening this to the whole tree would flag that legitimate site.
#
# Background, including the reproduction:
# docs/plans/2026-08-14-build-script-paths-and-actionable-build-failure.md

# The cargo variables that name a path INTO the checkout. Both are set when a
# build script is compiled, so both can be baked. Deliberately short: a
# variable cargo does not set at build-script compile time cannot be baked, so
# listing it would be noise rather than caution.
BUILD_SCRIPT_PATH_VARS='CARGO_MANIFEST_DIR|CARGO_MANIFEST_PATH'

# One sentence, printed verbatim by the gate so the fix is always spelled the
# same way.
# shellcheck disable=SC2034 # printed by scripts/check-build-script-paths.sh
BUILD_SCRIPT_PATH_ADVICE='Read it at run time instead: std::env::var("CARGO_MANIFEST_DIR"). See docs/plans/2026-08-14-build-script-paths-and-actionable-build-failure.md.'

# build_script_path_files [repo-root]
# Print every tracked cargo build script, one per line.
#
# Discovery is `git ls-files`, NOT a hand-maintained list, so a build script
# added to a new crate is covered the day it is committed.
#
# A build script is a `build.rs` sitting NEXT TO a `Cargo.toml`, which is
# cargo's own rule, and the sibling test is what keeps this gate off ordinary
# source. The name alone is not enough: this repo already has an unrelated
# `crates/lucidos-engine/src/core/store/messages/build.rs`, which cargo never
# runs and where compile-time `env!` would be perfectly legal.
#
# A crate that renames its script via `package.build` in Cargo.toml is not
# found. Parsing TOML to catch it is not worth it while no crate here does.
build_script_path_files() {
    local repo="${1:-.}" f
    git -C "$repo" ls-files 'build.rs' '*/build.rs' | while IFS= read -r f; do
        [ -n "$f" ] || continue
        [ -f "$repo/$(dirname "$f")/Cargo.toml" ] || continue
        printf '%s\n' "$f"
    done
}

# build_script_path_scan [repo-root]
# Print `path:line:content` for every build script line that bakes a path.
#
# WHOLE-TREE, not diff-scoped, matching check-adrs.sh and check-context-budget.sh:
# the question is what the tree does today, and the answer does not depend on
# which branch introduced it. A merge that reintroduces the form is then caught
# by the next branch to run the gate.
#
# Exit status is load-bearing. Non-zero means the scan could not run, which a
# caller must never read as "clean".
build_script_path_scan() {
    local repo="${1:-.}" listing rc f hits count=0

    if listing="$(build_script_path_files "$repo")"; then
        rc=0
    else
        rc=$?
    fi
    if [ "$rc" -ne 0 ]; then
        echo "build_script_path_scan: git ls-files failed (status $rc) in: $repo" >&2
        return 1
    fi

    while IFS= read -r f; do
        [ -n "$f" ] || continue
        count=$((count + 1))
        # `env!` matches `option_env!` as a substring too, which is intended:
        # both bake at compile time.
        if hits="$(grep -nE "env![[:space:]]*\([[:space:]]*\"($BUILD_SCRIPT_PATH_VARS)\"" -- "$repo/$f")"; then
            rc=0
        else
            rc=$?
        fi
        case "$rc" in
            0) printf '%s\n' "$hits" | awk -v p="$f" '{ print p ":" $0 }' ;;
            1) ;; # no match, the clean case
            *)
                echo "build_script_path_scan: grep failed (status $rc) on: $f" >&2
                return 1
                ;;
        esac
    done << EOF
$listing
EOF

    # Fail closed on broken discovery. This repo always has build scripts, so an
    # empty set means `git ls-files` did not do what we think, and reporting
    # "clean" would be a lie about files nobody looked at.
    if [ "$count" -eq 0 ]; then
        echo "build_script_path_scan: found no build.rs at all in: $repo" >&2
        return 1
    fi
}
