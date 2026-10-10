#!/usr/bin/env bash
#
# harden_suites.sh: the rules behind scripts/harden-suites.sh, which runs the
# `/harden` Phase 4.5 test suites and owns the early suite run (ADR 0292).
# Sourced by that script and by harden_suites_test.sh. Defines functions only.
#
# Four rules live here and nowhere else:
#   - which suites a list of changed paths selects (hs_select_suites);
#   - the command each suite runs (hs_suite_command);
#   - HARDEN_SAFE_PATHS, the paths no suite reads (hs_is_safe_path);
#   - the read check that turns that list off when a source may read it.

# The two Codex driver modules. An early run skips them and `wait` runs them
# alone once the Codex review is joined: their timeouts fail under load.
HS_DRIVER_MODULES="runtime::codex::driver_tests runtime::codex_app_server::driver_tests"

# True for a suite that runs cargo. Those share one lane and run one at a
# time, since two cargo test runs in one worktree can OOM the host. Every other
# suite gets a lane of its own.
hs_is_cargo_suite() {
    case "$1" in
        rust | shell-lint | app-lib | cli | gateway | engine-filtered | driver) return 0 ;;
    esac
    return 1
}

# Files no suite reads, at compile time or at run time, as an extended regex.
# A change to anything else voids an early result. Widen it only with proof that
# no test reads the new path. hs_read_check and the dep-info check in
# hs_unsafe_paths are the floor under that proof.
HARDEN_SAFE_PATHS="docs/(plans|adr)/[^\"'\`[:space:]]*|docs/code-review-priors\.md|docs/temporary-measures\.md"

hs_is_safe_path() {
    printf '%s\n' "$1" | grep -q -x -E "$HARDEN_SAFE_PATHS"
}

# Filtered cargo runs. One that matches no test passes vacuously, so each must
# report running at least one.
hs_is_filtered_suite() {
    case "$1" in
        driver | engine-filtered) return 0 ;;
    esac
    return 1
}

# A file matching this can reach a repo file: it anchors a path at the checkout.
HS_ANCHOR_RE='CARGO_MANIFEST_DIR|repo_root\(|include_str!|include_bytes!|__dirname|import\.meta\.url'

# Files that name a safe path beside an anchor without reading it, and how many
# such lines each has. A new file, or a higher count, trips the read check.
HS_READ_CHECK_KNOWN='crates/lucidos-app/e2e/markdown-table-columns.spec.ts 1
crates/lucidos-engine/src/engine/event_bus_tests/proposed_apply_cycle.rs 1
crates/lucidos-engine/src/engine/frontend_refresh.rs 1
crates/lucidos-engine/src/engine/git_ops/plan_marker.rs 1
crates/lucidos-engine/src/engine/thread_events_tests/ts_codegen.rs 1'

# Print "path count" for each source file that may read a safe path and is not
# in HS_READ_CHECK_KNOWN at that count. Returns 1 when it prints anything, and
# also when git cannot answer: a check that cannot run must not pass.
hs_read_check() { # <root>
    local root=$1 files rc f n known hits=0
    files=$(git -C "$root" grep -l --untracked -E "$HARDEN_SAFE_PATHS" -- \
        'crates/*.rs' 'crates/*.ts' 'crates/*.tsx' 'packages/*.ts')
    rc=$?
    # git grep exits 1 for "no match" and above 1 for a real failure.
    if [ "$rc" -eq 1 ]; then
        return 0
    elif [ "$rc" -ne 0 ]; then
        echo "git grep failed"
        return 1
    fi
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        if grep -q -E "$HS_ANCHOR_RE" "$root/$f"; then
            n=$(grep -v -E '^[[:space:]]*(//|\*|/\*)' "$root/$f" | grep -c -E "$HARDEN_SAFE_PATHS")
        else
            n=$(grep -v -E '^[[:space:]]*(//|\*|/\*)' "$root/$f" | grep -c -E "\.\./[^\"'\`]*($HARDEN_SAFE_PATHS)")
        fi
        [ "$n" -gt 0 ] || continue
        known=$(printf '%s\n' "$HS_READ_CHECK_KNOWN" | awk -v f="$f" '$1 == f { print $2 }')
        if [ -z "$known" ] || [ "$n" -gt "$known" ]; then
            echo "$f $n"
            hits=1
        fi
    done <<EOF
$files
EOF
    [ "$hits" -eq 0 ]
}

# Print one "<hash><TAB><path>" line per untracked, non-ignored file.
hs_untracked_fingerprint() { # <root>
    (
        cd "$1" || exit 1
        git ls-files -o --exclude-standard | while IFS= read -r p; do
            printf '%s\t%s\n' "$(git hash-object -- "$p")" "$p"
        done
    )
}

# Print every path that differs from the start commit: commits since, staged
# and unstaged edits, deletions, and untracked files that are new, changed or
# gone since the start fingerprint.
hs_changed_paths() { # <root> <start-sha> <untracked-at-start-file>
    local root=$1 sha=$2 start_untracked=$3 now
    git -C "$root" diff --no-renames --name-only "$sha" -- || return 1
    now="$(hs_untracked_fingerprint "$root")" || return 1
    printf '%s\n' "$now" | grep -v -x -F -f "$start_untracked" | cut -f2-
    cut -f2- "$start_untracked" | grep -v -x -F -f <(printf '%s\n' "$now" | cut -f2-)
    return 0
}

# Read paths on stdin and print each as a normalised repo-relative path:
# the root prefix stripped, and `.` and `..` segments resolved.
hs_normalize_paths() { # <root>
    awk -v root="$1/" '
        index($0, root) == 1 { $0 = substr($0, length(root) + 1) }
        {
            n = split($0, part, "/"); k = 0
            for (i = 1; i <= n; i++) {
                if (part[i] == "..") { if (k > 0) k-- }
                else if (part[i] != "." && part[i] != "") seg[++k] = part[i]
            }
            out = ""
            for (i = 1; i <= k; i++) out = out (i > 1 ? "/" : "") seg[i]
            if (out != "") print out
        }'
}

# Print every compile input cargo's dep-info lists. rustc writes an
# include_str! input relative to the workspace, with `..` segments, and a
# migration as an absolute path. Nothing prints before the first build.
hs_compile_inputs() { # <root>
    local root=$1 deps="${CARGO_TARGET_DIR:-$1/target}/debug/deps"
    [ -d "$deps" ] || return 0
    find "$deps" -name '*.d' -exec grep -h -v '^#' {} + 2>/dev/null | tr ' ' '\n' | sed 's/:$//' \
        | hs_normalize_paths "$root" | sort -u
}

# Print every file an include_str! or include_bytes! literal in source names,
# so a worktree with no build still knows its compile inputs. A plain literal
# resolves against its file, and a CARGO_MANIFEST_DIR concat against its crate.
# The pathspec narrows the search to one crate's sources.
hs_source_includes() { # <root> [pathspec]
    local root=$1 pathspec=${2:-crates/*.rs} f dir crate base lit
    git -C "$root" grep -l -E 'include_(str|bytes)!' -- "$pathspec" 2>/dev/null \
        | while IFS= read -r f; do
            dir="$(dirname "$f")"
            crate="$dir"
            while [ "$crate" != . ] && [ ! -f "$root/$crate/Cargo.toml" ]; do crate="$(dirname "$crate")"; done
            awk '{ buf = buf $0 "\n" }
                END {
                    while (match(buf, /include_(str|bytes)!\(/)) {
                        buf = substr(buf, RSTART + RLENGTH)
                        rest = substr(buf, 1, 400)
                        if (match(rest, /^[ \t\n]*concat!\([ \t\n]*env!\("CARGO_MANIFEST_DIR"\)[ \t\n]*,[ \t\n]*"[^"]*"/)) {
                            lit = substr(rest, 1, RLENGTH); sub(/^.*"CARGO_MANIFEST_DIR"\)[ \t\n]*,[ \t\n]*"/, "", lit)
                            print "crate\t" substr(lit, 1, length(lit) - 1)
                        } else if (match(rest, /^[ \t\n]*"[^"]*"/)) {
                            lit = substr(rest, 1, RLENGTH); sub(/^[ \t\n]*"/, "", lit)
                            print "file\t" substr(lit, 1, length(lit) - 1)
                        }
                    }
                }' "$root/$f" | while IFS="$(printf '\t')" read -r base lit; do
                if [ "$base" = crate ]; then echo "$crate/$lit"; else echo "$dir/$lit"; fi
            done
        done | hs_normalize_paths "$root" | sort -u
}

# Read paths on stdin and print the ones that void an early result. With the
# allowlist disabled, every path does. An allowlisted path still voids it when
# the compile-inputs file lists it: a crate compiles it in.
hs_unsafe_paths() { # <compile-inputs-file> <allowlist-disabled: 0|1>
    local inputs=$1 disabled=$2 p
    while IFS= read -r p; do
        [ -n "$p" ] || continue
        # An unreadable inputs file voids every path: the check could not run.
        if [ "$disabled" = 1 ] || ! hs_is_safe_path "$p" || [ ! -r "$inputs" ] \
            || grep -q -x -F -- "$p" "$inputs"; then
            echo "$p"
        fi
    done
}

# Print every path a Vitest test names, read from the repo root and from the
# test's directory, normalised. A changed path under one selects the Vitest
# suite, deleted or not. A test that read a Rust enum once went red on main
# after a Rust-only change (ADR 0315). A path built at run time is missed.
hs_vitest_inputs() { # <root>
    git -C "$1" grep -o -I --full-name --no-line-number --no-column --no-color -E "['\"\`][A-Za-z0-9_@.][A-Za-z0-9_@./-]*['\"\`]" -- \
        'crates/lucidos-app/src/*.test.ts' 'crates/lucidos-app/src/*.test.tsx' 'packages/lucidos-sdk/src/*.test.ts' \
        | awk '{
            i = index($0, ":"); f = substr($0, 1, i - 1); lit = substr($0, i + 2)
            lit = substr(lit, 1, length(lit) - 1)
            if (lit !~ /[.\/]/) next
            dir = f; sub(/\/[^\/]*$/, "", dir)
            print lit; print dir "/" lit
        }' | hs_normalize_paths "$1" | sort -u
}

# The files outside system-knowhow/ that a `value_pins_tests` module reads, one
# list per crate. Each holds a copy of a Rust constant it cannot import (ADR
# 0368). harden_suites_test.sh checks each list against the paths its test names.
HS_GATEWAY_PINNED="install.sh scripts/lib/service.sh scripts/lib/workspace_constants.sh crates/lucidos-app/tauri.conf.json README.md"
HS_ENGINE_PINNED="scripts/lib/workspace_constants.sh scripts/status.sh README.md"

hs_is_listed() { # <path> <space-separated list>
    case " $2 " in
        *" $1 "*) return 0 ;;
    esac
    return 1
}

# The engine tests a system-knowhow or engine-pinned edit needs. The engine
# reads those files at run time rather than compiling them in, so no compile
# input selects them. The full engine suite subsumes the filter when Rust is
# selected anyway.
hs_engine_filters() { # changed paths on stdin
    local p knowhow="" pinned="" filters=""
    while IFS= read -r p; do
        case "$p" in system-knowhow/*) knowhow=1 ;; esac
        if hs_is_listed "$p" "$HS_ENGINE_PINNED"; then pinned=1; fi
    done
    if [ -n "$knowhow" ]; then
        filters="always_loaded_context_stays_under_budget system_knowhow_descriptions_stay_routing_sized "
    fi
    if [ -n "$knowhow$pinned" ]; then
        echo "${filters}value_pins_tests"
    fi
    return 0
}

# Read changed paths on stdin and print the selected suites, one per line.
# This is the `/harden` Phase 4.5 test-selection table. A path listed in the
# compile-inputs file selects the Rust suite whatever its extension: a crate
# compiles it in, so a docs-only or frontend-only edit to it can break a build.
# A path in the cli-inputs file also selects the CLI tests. The CLI pins engine
# files that way, and `make test` never runs its tests. A path in the
# vitest-inputs file (hs_vitest_inputs) selects the Vitest suite.
hs_select_suites() { # [compile-inputs-file] [cli-inputs-file] [vitest-inputs-file]
    local inputs=${1:-/dev/null} cli_inputs=${2:-/dev/null} vitest_inputs=${3:-/dev/null} p rust="" shell="" app="" cli="" gateway="" ts="" css="" vitest="" install="" release="" self="" scope="" codex="" paths=""
    while IFS= read -r p; do
        [ -n "$p" ] || continue
        paths="$paths$p
"
        case "$p" in
            *.rs | Cargo.toml | */Cargo.toml | Cargo.lock | *.sql) rust=1 ;;
            *)
                if grep -q -x -F -- "$p" "$inputs"; then
                    rust=1
                    case "$p" in crates/lucidos-app/*) app=1 ;; esac
                fi
                ;;
        esac
        case "$p" in
            crates/lucidos-app/src/*.rs) app=1 ;;
            crates/lucidos-cli/*) cli=1 ;;
            *) grep -q -x -F -- "$p" "$cli_inputs" 2>/dev/null && cli=1 ;;
        esac
        case "$p" in
            *.sh | .shellcheckrc | Makefile) shell=1 ;;
        esac
        if hs_is_listed "$p" "$HS_GATEWAY_PINNED"; then gateway=1; fi
        case "$p" in
            install.sh | uninstall.sh | scripts/lib/service.sh | scripts/lib/stage_runtime.sh | \
                scripts/lib/headless_tarball.sh | scripts/lib/install_common.sh) install=1 ;;
            scripts/release.sh | scripts/lib/release_draft.sh) release=1 ;;
            scripts/harden-suites.sh | scripts/lib/harden_suites*.sh | scripts/lib/proc_tree.sh) self=1 ;;
            scripts/harden-scope.sh | scripts/lib/harden_scope_test.sh) scope=1 ;;
            scripts/harden-codex-review.sh | scripts/lib/harden_codex_review_test.sh) codex=1 ;;
        esac
        case "$p" in
            *.ts | *.tsx) ts=1 ;;
            crates/lucidos-app/src/*.css) css=1 ;;
        esac
    done
    # A changed path selects Vitest when it, or a directory above it, is named.
    printf '%s' "$paths" | awk 'NR == FNR { want[$0] = 1; next }
        { p = $0; while (p != "") { if (p in want) { hit = 1; exit } if (!sub(/\/[^\/]*$/, "", p)) break } }
        END { exit !hit }' "$vitest_inputs" - 2>/dev/null && vitest=1

    if [ -n "$rust" ]; then
        echo rust
    elif [ -n "$shell" ]; then
        echo shell-lint
    fi
    [ -n "$app" ] && echo app-lib
    [ -n "$cli" ] && echo cli
    if [ -z "$rust" ] && [ -n "$(printf '%s' "$paths" | hs_engine_filters)" ]; then
        echo engine-filtered
    fi
    # `make test` runs the gateway's tests, so the Rust suite subsumes this one.
    [ -z "$rust" ] && [ -n "$gateway" ] && echo gateway
    if [ -n "$ts" ]; then
        echo ts
    else
        [ -n "$css" ] && echo vite
        [ -n "$vitest" ] && echo vitest
    fi
    [ -n "$install" ] && echo install
    [ -n "$release" ] && echo release
    [ -n "$self" ] && echo harden-suites
    [ -n "$scope" ] && echo harden-scope
    [ -n "$codex" ] && echo harden-codex-review
    return 0
}

# Print the shell command a suite runs. HS_STUB_CMD, when set, replaces every
# command with "<stub> <suite>", so the tests never start a real suite.
hs_suite_command() { # <suite> <early|normal> <changed-paths-file>
    local suite=$1 mode=$2 paths_file=$3 skips="" m
    if [ -n "${HS_STUB_CMD:-}" ]; then
        printf '%q %q\n' "$HS_STUB_CMD" "$suite"
        return 0
    fi
    case "$suite" in
        rust)
            if [ "$mode" = early ]; then
                for m in $HS_DRIVER_MODULES; do skips="$skips --skip $m"; done
                echo "make lint && make test ENGINE_TEST_ARGS='-- --$skips'"
            else
                echo "make lint && make test"
            fi
            ;;
        shell-lint) echo "make lint" ;;
        app-lib) echo "cargo test --locked -p lucidos-app --lib" ;;
        cli) echo "cargo test --locked -p lucidos-cli" ;;
        gateway) echo "cargo test --locked -p lucidos-gateway" ;;
        engine-filtered) echo "./scripts/test-engine.sh -- -- $(hs_engine_filters < "$paths_file")" ;;
        driver) echo "./scripts/test-engine.sh -- -- $HS_DRIVER_MODULES" ;;
        ts) echo "cd crates/lucidos-app && npx tsc --noEmit && npm test && npx vite build" ;;
        vite) echo "cd crates/lucidos-app && npx vite build" ;;
        vitest) echo "cd crates/lucidos-app && npm test" ;;
        install) echo "bash scripts/lib/install_test.sh" ;;
        release) echo "bash scripts/lib/release_draft_test.sh && bash scripts/lib/release_rc_gate_test.sh" ;;
        harden-suites) echo "bash scripts/lib/harden_suites_test.sh" ;;
        harden-scope) echo "bash scripts/lib/harden_scope_test.sh" ;;
        harden-codex-review) echo "bash scripts/lib/harden_codex_review_test.sh" ;;
        *)
            echo "unknown suite: $suite" >&2
            return 1
            ;;
    esac
}
