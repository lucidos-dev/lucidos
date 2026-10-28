#!/usr/bin/env bash
#
# harden_suites.sh: the rules behind scripts/harden-suites.sh, which runs the
# `/harden` Phase 4.5 test suites and owns the early suite run (ADR 0292).
# Sourced by that script and by harden_suites_test.sh. Defines functions only.
#
# Eight rules live here and nowhere else:
#   - which workspace crates a list of changed paths reaches (hs_reached_crates):
#     the owners of each path, then every crate that depends on one, read from
#     the graph `cargo metadata` gives (hs_crate_graph, hs_compile_owners);
#   - which suites a list of changed paths selects (hs_select_suites), the
#     Rust ones from those crates and the two touched-spec suites among them
#     (the specs come from touched_specs.sh);
#   - whether the branch changes what codegen renders (hs_generated_drift);
#   - what the fast tier drops from that selection (hs_fast_tier_suite);
#   - the command each suite runs, whole or for its failed tests alone
#     (hs_suite_command);
#   - which tests a failed suite names, so the next round reruns those first
#     (hs_failed_tests);
#   - HARDEN_SAFE_PATHS, the paths no suite reads (hs_is_safe_path);
#   - the read check that turns that list off when a source may read it.

# shellcheck source=scripts/lib/touched_specs.sh
source "$(dirname "${BASH_SOURCE[0]}")/touched_specs.sh"
# shellcheck source=scripts/lib/e2e_projects.sh
source "$(dirname "${BASH_SOURCE[0]}")/e2e_projects.sh"
# shellcheck source=scripts/lib/e2e_flaky.sh
source "$(dirname "${BASH_SOURCE[0]}")/e2e_flaky.sh"
# shellcheck source=scripts/lib/e2e_lock.sh
source "$(dirname "${BASH_SOURCE[0]}")/e2e_lock.sh"

# The two Codex driver modules. An early run skips them and `wait` starts them
# alone once the other-model review ends: their timeouts fail under load.
HS_DRIVER_MODULES="runtime::codex::driver_tests runtime::codex_app_server::driver_tests"

# How long one `wait` blocks by default, in seconds. The Claude Code prompt
# cache lives 5 minutes from the start of the last request, so a join returns
# well inside that and the agent re-issues it (ADR 0470).
# shellcheck disable=SC2034 # read by scripts/harden-suites.sh and its test
HS_WAIT_BUDGET=230

# Suites that run Cargo share a lane. They run one at a time, since
# overlapping Cargo tests can OOM the host. Every other suite gets a lane of
# its own.
hs_is_cargo_suite() {
    case "$1" in
        lint | shell-lint | crates | gateway | cli | app-lib | eval | engine | engine-filtered | driver) return 0 ;;
    esac
    return 1
}

# Suites that read codegen's output. They never run codegen themselves: the
# cargo lane runs it once, as its first step, and they wait for that step.
hs_waits_for_codegen() { # <suite>
    case "$1" in
        ts | vitest | knowhow-generated) return 0 ;;
    esac
    return 1
}

# True when a selection holds a suite that waits for the codegen step.
hs_needs_codegen() { # <newline-separated suites>
    local s
    for s in $1; do
        hs_waits_for_codegen "$s" && return 0
    done
    return 1
}

# The tier of `/harden` a branch runs, from whatever named it: `fast` for an
# integrated sub-thread, `full` for anything else, an empty answer included.
hs_normalize_tier() { # <answer>
    if [ "${1:-}" = fast ]; then echo fast; else echo full; fi
}

# The tier a branch runs. The engine's answer wins, so no variable can drop
# a full branch's suites. HARDEN_TIER names the tier only when no engine
# answers, which is the hermetic tests' case.
hs_resolve_tier() { # <engine-answer> <HARDEN_TIER>
    if [ -n "${1:-}" ]; then
        hs_normalize_tier "$1"
    else
        hs_normalize_tier "${2:-}"
    fi
}

# What the fast tier runs in place of a full-tier suite, or nothing for a
# suite it drops. The fast tier keeps the lint, the frontend checks and the
# script tests. It drops every crate's test suite, the installer, the release
# path, the first-paint check and the touched browser specs: the integrating
# parent runs them all once.
hs_fast_tier_suite() { # <suite>
    case "$1" in
        codegen | knowhow-generated | lint | shell-lint | ts | vite | vitest | harden-suites | harden-scope | harden-other-model-review) echo "$1" ;;
    esac
    return 0
}

# Files no suite reads, at compile time or at run time, as an extended regex:
# every markdown file under docs/, and anything under the plans and ADR folders.
# A change to anything else voids an early result, so a fix to those docs alone
# never restarts the suites. Widen it only with proof that no test reads the new
# path. hs_read_check and the dep-info check in hs_unsafe_paths are the floor
# under that proof.
HARDEN_SAFE_PATHS="docs/[^\"'\`[:space:]]*\.md|docs/(plans|adr)/[^\"'\`[:space:]]*"

# The docs a test does read, which HARDEN_SAFE_PATHS would otherwise cover.
HARDEN_READ_DOCS='docs/glossary\.md'

hs_is_safe_path() {
    printf '%s\n' "$1" | grep -q -x -E "$HARDEN_SAFE_PATHS" \
        && ! printf '%s\n' "$1" | grep -q -x -E "$HARDEN_READ_DOCS"
}

# Filtered cargo runs. One that matches no test passes vacuously, so each must
# report running at least one.
hs_is_filtered_suite() {
    case "$1" in
        driver | engine-filtered) return 0 ;;
    esac
    return 1
}

# Print the filters no passing test's name contains, space-separated, from a
# cargo test log. One live filter must not hide a dead one beside it.
hs_dead_filters() { # <log> <filter>...
    local log=$1 f dead=""
    shift
    for f in "$@"; do
        grep -q -E "^test [^ ]*${f}[^ ]* \.\.\. ok$" "$log" 2>/dev/null || dead="$dead $f"
    done
    printf '%s' "${dead# }"
}

# A file matching this can reach a repo file: it anchors a path at the checkout.
HS_ANCHOR_RE='CARGO_MANIFEST_DIR|repo_root\(|include_str!|include_bytes!|__dirname|import\.meta\.url'

# Files that name a safe path beside an anchor without reading it, and how many
# such lines each has. A new file, or a higher count, trips the read check.
HS_READ_CHECK_KNOWN='crates/lucidos-app/e2e/markdown-table-columns-all.spec.ts 1
crates/lucidos-app/src/components/chat/__tests__/chat-link-click.test.ts 1
crates/lucidos-app/src/components/plugins/__tests__/plugin-detail-page.test.tsx 1
crates/lucidos-app/src/styles/__tests__/iframe-css-no-shared-component-restyle.test.ts 1
crates/lucidos-engine/src/core/changes.rs 1
crates/lucidos-engine/src/engine/agent_session/lifecycle_tests/propose.rs 1
crates/lucidos-engine/src/engine/agent_session/prompts.rs 6
crates/lucidos-engine/src/engine/change_ops_withheld_work_tests.rs 3
crates/lucidos-engine/src/engine/event_bus_tests/proposed_apply_cycle.rs 1
crates/lucidos-engine/src/engine/frontend_refresh.rs 1
crates/lucidos-engine/src/engine/git_ops/plan_marker.rs 1
crates/lucidos-engine/src/engine/successor_tests.rs 3
crates/lucidos-engine/src/engine/thread_events_tests/ts_codegen.rs 1
crates/lucidos-engine/src/engine/thread_lifecycle_tests/invariants.rs 1
crates/lucidos-engine/src/runtime/startup_instructions/tests.rs 3'

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
        # A doc on HARDEN_READ_DOCS is off the allowlist already, so naming it
        # is no read of a safe path.
        if grep -q -E "$HS_ANCHOR_RE" "$root/$f"; then
            n=$(grep -v -E '^[[:space:]]*(//|\*|/\*)' "$root/$f" | sed -E "s#$HARDEN_READ_DOCS##g" \
                | grep -c -E "$HARDEN_SAFE_PATHS")
        else
            n=$(grep -v -E '^[[:space:]]*(//|\*|/\*)' "$root/$f" | sed -E "s#$HARDEN_READ_DOCS##g" \
                | grep -c -E "\.\./[^\"'\`]*($HARDEN_SAFE_PATHS)")
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
# the root prefix stripped, and `.` and `..` segments resolved. A line may
# carry more tab-separated fields after the path, which pass through.
hs_normalize_paths() { # <root>
    awk -v root="$1/" '
        {
            rest = ""; t = index($0, "\t")
            if (t) { rest = substr($0, t); $0 = substr($0, 1, t - 1) }
            if (index($0, root) == 1) $0 = substr($0, length(root) + 1)
            n = split($0, part, "/"); k = 0
            for (i = 1; i <= n; i++) {
                if (part[i] == "..") { if (k > 0) k-- }
                else if (part[i] != "." && part[i] != "") seg[++k] = part[i]
            }
            out = ""
            for (i = 1; i <= k; i++) out = out (i > 1 ? "/" : "") seg[i]
            if (out != "") print out rest
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

# Print one "<path> <kind> <test-fn> <crate>" line, tab-separated, for each
# include_str! or include_bytes! literal in source. So a worktree with no build
# still knows its compile inputs. A plain literal resolves against its file,
# and a CARGO_MANIFEST_DIR concat against its crate. The pathspec narrows the
# search to one crate's sources.
#
# The kind is `test` for a site inside a fn carrying a test attribute, which
# compiles only under cfg(test), with that fn's name. Any other site is `prod`
# with no name. rustfmt indentation finds the enclosing fn, and a layout the
# scan cannot read comes out `prod`, which selects more, never less.
hs_include_sites() { # <root> [pathspec]
    local root=$1 pathspec=${2:-crates/*.rs} f dir crate base lit kind fn
    git -C "$root" grep -l -E 'include_(str|bytes)!' -- "$pathspec" 2>/dev/null \
        | while IFS= read -r f; do
            dir="$(dirname "$f")"
            crate="$dir"
            while [ "$crate" != . ] && [ ! -f "$root/$crate/Cargo.toml" ]; do crate="$(dirname "$crate")"; done
            awk '
                function indent(s) { match(s, /^[ \t]*/); return RLENGTH }
                function fn_name(s) {
                    if (!match(s, /^[ \t]*(pub(\([a-z]+\))?[ \t]+)?(async[ \t]+)?fn[ \t]+[A-Za-z0-9_]+/)) return ""
                    s = substr(s, 1, RLENGTH); sub(/^.*fn[ \t]+/, "", s); return s
                }
                function test_fn(j,   a) {
                    for (a = j - 1; a >= 1 && L[a] ~ /^[ \t]*(#\[|\/\/)/; a--)
                        if (L[a] ~ /^[ \t]*#\[([A-Za-z_]+::)*test(\]|\()/) return "test\t" fn_name(L[j])
                    return "prod"
                }
                function classify(i,   j, s, cur) {
                    if (fn_name(L[i]) != "") return test_fn(i)
                    cur = indent(L[i])
                    for (j = i - 1; j >= 1; j--) {
                        s = L[j]
                        if (s ~ /^[ \t]*$/ || indent(s) >= cur) continue
                        if (fn_name(s) != "") return test_fn(j)
                        if (indent(s) == 0 || s ~ /^[ \t]*(pub(\([a-z]+\))?[ \t]+)?(mod|impl|trait)[ \t<]/) return "prod"
                        cur = indent(s)
                    }
                    return "prod"
                }
                { L[NR] = $0 }
                END {
                    for (i = 1; i <= NR; i++) {
                        line = L[i]
                        while (match(line, /include_(str|bytes)!\(/)) {
                            line = substr(line, RSTART + RLENGTH)
                            rest = line
                            for (k = i + 1; k <= NR && length(rest) < 400; k++) rest = rest "\n" L[k]
                            rest = substr(rest, 1, 400)
                            if (match(rest, /^[ \t\n]*concat!\([ \t\n]*env!\("CARGO_MANIFEST_DIR"\)[ \t\n]*,[ \t\n]*"[^"]*"/)) {
                                lit = substr(rest, 1, RLENGTH); sub(/^.*"CARGO_MANIFEST_DIR"\)[ \t\n]*,[ \t\n]*"/, "", lit)
                                print "crate\t" substr(lit, 1, length(lit) - 1) "\t" classify(i)
                            } else if (match(rest, /^[ \t\n]*"[^"]*"/)) {
                                lit = substr(rest, 1, RLENGTH); sub(/^[ \t\n]*"/, "", lit)
                                print "file\t" substr(lit, 1, length(lit) - 1) "\t" classify(i)
                            }
                        }
                    }
                }' "$root/$f" | while IFS="$(printf '\t')" read -r base lit kind fn; do
                if [ "$base" = crate ]; then base="$crate/$lit"; else base="$dir/$lit"; fi
                # A --lib suite never runs a test outside the lib target.
                case "${f#"$crate"/}" in src/main.rs | src/bin/* | tests/* | benches/* | examples/*) kind=prod ;; esac
                printf '%s\t%s\t%s\t%s\n' "$base" "$kind" "$fn" "$crate"
            done
        done | hs_normalize_paths "$root" | sort -u
}

# Print every file an include site names (hs_include_sites).
hs_source_includes() { # <root> [pathspec]
    hs_include_sites "$@" | cut -f1 | sort -u
}

# Print "<path> <suite> <test-fn>", tab-separated, for each site of a test-only
# include: a path whose every include site is a test fn in a crate with a
# suite of its own. The test fn is empty for any suite but engine-filtered.
# A path a build script names stays out: the build may read it.
hs_test_inputs() { # <root>
    local root=$1
    # The leading echo keeps the first file non-empty, so NR == FNR marks it.
    hs_include_sites "$root" | awk -F '\t' '
        NR == FNR { builds = builds $0 "\n"; next }
        function suite(c) {
            if (c == "crates/lucidos-engine") return "engine-filtered"
            if (c == "crates/lucidos-cli") return "cli"
            if (c == "crates/lucidos-gateway") return "gateway"
            if (c == "crates/lucidos-app") return "app-lib"
            return ""
        }
        {
            s = suite($4)
            n = split($1, part, "/")
            if ($2 != "test" || s == "" || index(builds, part[n])) { bad[$1] = 1; next }
            line[++count] = $1 "\t" s "\t" (s == "engine-filtered" ? $3 : "")
        }
        END {
            for (i = 1; i <= count; i++) {
                split(line[i], f, "\t")
                if (!(f[1] in bad)) print line[i]
            }
        }' <(echo; git -C "$root" grep -h -I -e '' -- 'crates/*build.rs' 2>/dev/null) - | sort -u
}

# Print "<crate><TAB><dir><TAB><path dependencies>" for each workspace member:
# its directory relative to the root, and the members it depends on through a
# path, of any kind (normal, dev or build), space-separated. cargo is the only
# source, so a new crate or edge needs no edit here. Prints nothing when cargo
# or the parse fails: hs_reached_crates then selects every crate.
# HS_CARGO_METADATA_CMD replaces the cargo call in the tests.
hs_crate_graph() { # <root>
    local cmd=${HS_CARGO_METADATA_CMD:-cargo metadata --no-deps --offline --locked --format-version 1}
    (cd "$1" && bash -c "$cmd") 2> /dev/null | python3 -I -c '
import json, os, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"]
members = {}
for pkg in meta["packages"]:
    rel = os.path.relpath(os.path.dirname(pkg["manifest_path"]), root)
    if not rel.startswith(".."):
        members[pkg["name"]] = (rel, pkg["dependencies"])
for name in sorted(members):
    rel, deps = members[name]
    path_deps = sorted({d["name"] for d in deps if d.get("path") and d["name"] in members})
    print("\t".join([name, rel, " ".join(path_deps)]))
' 2> /dev/null
}

# Print "<path><TAB><crate dir>" for each path that a member's build compiles
# in from outside its own directory. A dep-info file belongs to the members
# whose own .rs sources it lists: its name says nothing, since an integration
# test or a build script is named after neither crate. Every include site adds
# its crate too, so a worktree with no build still has owners.
hs_compile_owners() { # <root> <graph-file>
    local root=$1 graph=$2 deps="${CARGO_TARGET_DIR:-$1/target}/debug/deps"
    [ -r "$graph" ] || graph=/dev/null
    {
        if [ -d "$deps" ]; then
            find "$deps" -name '*.d' -exec awk -v root="$root/" '
                /^#/ { next }
                {
                    n = split($0, tok, " ")
                    for (i = 1; i <= n; i++) {
                        t = tok[i]; sub(/:$/, "", t)
                        if (t == "" || (substr(t, 1, 1) == "/" && index(t, root) != 1)) continue
                        print t "\t" FILENAME
                    }
                }' {} + 2> /dev/null | hs_normalize_paths "$root" | awk -F '\t' '
                function owner_dir(p,   q) {
                    for (q = p; q != ""; ) {
                        if (q in dir) return q
                        if (!sub(/\/[^\/]*$/, "", q)) break
                    }
                    return ""
                }
                sect == "graph" { dir[$2] = 1; next }
                $1 ~ /^target\// { next }
                {
                    f = $2
                    if (!(f in count)) order[++files] = f
                    tokens[f, ++count[f]] = $1
                    if ($1 ~ /\.rs$/ && (od = owner_dir($1)) != "" && index(own[f], " " od " ") == 0) own[f] = own[f] " " od " "
                }
                END {
                    for (k = 1; k <= files; k++) {
                        f = order[k]
                        m = split(own[f], d, " ")
                        for (j = 1; j <= m; j++)
                            for (i = 1; i <= count[f]; i++)
                                if (index(tokens[f, i], d[j] "/") != 1) print tokens[f, i] "\t" d[j]
                    }
                }' sect=graph "$graph" sect=deps -
        fi
        hs_include_sites "$root" | cut -f1,4
    } | sort -u
}

# Read changed paths on stdin and print the workspace crates they reach, one
# per line: the owners of each Rust-relevant path, and every crate that
# depends on an owner, however far down. Prints `*` when the diff can reach
# any crate: a path no member owns, one of the files that configure every
# build, or a graph that is missing or empty. Prints nothing when no path
# is Rust-relevant. A Rust-relevant path is Rust source, a Cargo manifest,
# Cargo.lock, a migration, or a compile input that no test alone includes.
hs_reached_crates() { # <compile-inputs> <test-inputs> <graph> <owners>
    local inputs=$1 test_inputs=$2 graph=$3 owners=$4
    [ -r "$inputs" ] || inputs=/dev/null
    [ -r "$test_inputs" ] || test_inputs=/dev/null
    [ -r "$graph" ] || graph=/dev/null
    [ -r "$owners" ] || owners=/dev/null
    awk -F '\t' '
        function owner_dir(p,   q) {
            for (q = p; q != ""; ) {
                if (q in name) return q
                if (!sub(/\/[^\/]*$/, "", q)) break
            }
            return ""
        }
        function seed(dirs,   n, i, e, found) {
            n = split(dirs, e, " ")
            for (i = 1; i <= n; i++) if (e[i] in name) { want[name[e[i]]] = 1; found++ }
            return found
        }
        sect == "inputs" { input[$1] = 1; next }
        sect == "tests" { testonly[$1] = 1; next }
        sect == "graph" {
            name[$2] = $1; deps[$1] = $3
            n = split($3, d, " ")
            for (i = 1; i <= n; i++) users[d[i]] = users[d[i]] " " $1
            next
        }
        sect == "owners" { owned[$1] = owned[$1] " " $2; next }
        {
            p = $0
            if (p == "Cargo.lock" || p == "Cargo.toml" || p == "rust-toolchain.toml" || p == "clippy.toml" || p == "Makefile" \
                || p ~ /^\.cargo\// || p ~ /^crates\/vendor\//) { all = 1; next }
            if (!(p ~ /\.rs$/ || p ~ /(^|\/)Cargo\.toml$/ || p ~ /\.sql$/ || ((p in input) && !(p in testonly)))) next
            hit = 0
            if ((od = owner_dir(p)) != "") { want[name[od]] = 1; hit = 1 }
            if (p in owned && seed(owned[p]) > 0) hit = 1
            if (!hit) all = 1
            any = 1
        }
        END {
            if (all) { print "*"; exit }
            if (!any) exit
            for (c in want) { todo[++top] = c; reached[c] = 1 }
            while (top > 0) {
                c = todo[top--]
                n = split(users[c], u, " ")
                for (i = 1; i <= n; i++) if (!(u[i] in reached)) { reached[u[i]] = 1; todo[++top] = u[i] }
            }
            for (c in reached) print c | "sort"
        }' sect=inputs "$inputs" sect=tests "$test_inputs" sect=graph "$graph" sect=owners "$owners" sect=paths -
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
HS_GATEWAY_PINNED="install.sh scripts/lib/service.sh scripts/lib/workspace_constants.sh crates/lucidos-app/tauri.conf.json crates/lucidos-app/e2e/db-helpers.ts crates/lucidos-e2e/tests/api_support/mod.rs README.md"
HS_ENGINE_PINNED="scripts/lib/workspace_constants.sh scripts/status.sh README.md"

hs_is_listed() { # <path> <space-separated list>
    case " $2 " in
        *" $1 "*) return 0 ;;
    esac
    return 1
}

# The engine tests a system-knowhow or engine-pinned edit needs. The engine
# reads those files at run time rather than compiling them in, so no compile
# input selects them. A test-only include (hs_test_inputs) adds the test fns
# that include it. The full engine suite subsumes the filter when Rust is
# selected anyway.
hs_engine_filters() { # [test-inputs-file], changed paths on stdin
    local test_inputs=${1:-/dev/null} p knowhow="" pinned="" filters="" fns=""
    while IFS= read -r p; do
        case "$p" in system-knowhow/*) knowhow=1 ;; esac
        if hs_is_listed "$p" "$HS_ENGINE_PINNED"; then pinned=1; fi
        fns="$fns$(awk -F '\t' -v p="$p" '$1 == p && $2 == "engine-filtered" { printf " %s", $3 }' "$test_inputs" 2>/dev/null)"
    done
    if [ -n "$knowhow" ]; then
        filters="always_loaded_context_stays_under_budget system_knowhow_descriptions_stay_routing_sized "
    fi
    if [ -n "$knowhow$pinned" ]; then
        filters="${filters}value_pins_tests "
    fi
    # Sorted and unique, so the command is the same whatever order paths came in.
    # shellcheck disable=SC2086 # split the fn names into one per line
    fns="$(printf '%s\n' $fns | sed '/^$/d' | sort -u | tr '\n' ' ')"
    filters="$(printf '%s' "$filters$fns" | sed 's/ *$//')"
    [ -n "$filters" ] && echo "$filters"
    return 0
}

# Print "drift" when the branch changes a generated file, which a changed path
# need not show: generated files are build outputs. A Rust change that
# renames a wire field must still run the TS suites. Renders the branch when
# its codegen fingerprint differs from the merge base's, then compares the two
# renderings. No rendering for the base, or a render that fails, counts as drift.
# HS_CODEGEN replaces scripts/codegen.sh in the tests.
hs_generated_drift() { # <root>
    local root=$1 codegen=${HS_CODEGEN:-$1/scripts/codegen.sh} base head_dir base_dir
    base=$(git -C "$root" merge-base main HEAD 2>/dev/null) || { echo drift; return 0; }
    [ "$("$codegen" --fingerprint "$base")" = "$("$codegen" --fingerprint)" ] && return 0
    if ! "$codegen" >&2 \
        || ! head_dir=$("$codegen" --cache-dir) \
        || ! base_dir=$("$codegen" --cache-dir "$base") \
        || ! diff -rq "$base_dir" "$head_dir" >/dev/null; then
        echo drift
    fi
    return 0
}

# The workspace crates with a suite of their own, or no harden suite at all
# (lucidos-e2e needs a running engine). Every other member is a small crate,
# which the `crates` suite tests together.
HS_SUITE_CRATES="lucidos-engine lucidos-eval lucidos-gateway lucidos-app lucidos-cli lucidos-e2e"

# True when a reached set (hs_reached_crates) holds <crate>: `*` holds them all.
hs_reach_has() { # <reach> <crate>
    [ "$1" = '*' ] && return 0
    printf '%s\n' "$1" | grep -q -x -F -- "$2"
}

# Print the small crates a reached set holds, one per line, or `*` for every
# one. An empty set prints nothing.
hs_small_crates() { # <reach>
    local c
    if [ "$1" = '*' ]; then
        echo '*'
        return 0
    fi
    for c in $1; do
        hs_is_listed "$c" "$HS_SUITE_CRATES" || echo "$c"
    done
    return 0
}

# Print the crates a run's changed paths reach: hs_reached_crates over the
# state directory's records. `start` writes them all before any suite runs.
hs_state_reach() { # <state-dir>
    hs_reached_crates "$1/compile-inputs" "$1/test-inputs" "$1/crate-graph" "$1/compile-owners" < "$1/paths"
}

# Read changed paths on stdin and print the selected suites, one per line.
# This is the `/harden` Phase 4.5 test-selection table. The crates a diff
# reaches (hs_reached_crates) pick the lint scope and each crate's tests. A path
# listed in the compile-inputs file reaches the crates that compile it in,
# whatever its extension: a docs-only or frontend-only edit to it can break a
# build. A compile input the test-inputs file lists (hs_test_inputs) only a
# test fn includes, so it selects that crate's suite instead, and no production
# build can break. A path in the cli-inputs file also selects the CLI tests: the
# CLI pins engine files that way, and the crate graph does not show it. A path in the vitest-inputs file (hs_vitest_inputs) selects the Vitest
# suite. A non-empty drift file (hs_generated_drift) selects the TS suite. A
# non-empty touched-specs file (ts_touched_specs) selects the e2e-touched suite,
# and a spec mobile-webkit runs among them selects e2e-touched-webkit too. The
# `fast` tier narrows the selection through hs_fast_tier_suite.
hs_select_suites() { # [compile-inputs-file] [cli-inputs-file] [vitest-inputs-file] [drift-file] [tier] [touched-specs-file] [test-inputs-file] [crate-graph-file] [compile-owners-file]
    local suite
    if [ "$(hs_normalize_tier "${5:-}")" = fast ]; then
        hs_select_full_tier_suites "${1:-}" "${2:-}" "${3:-}" "${4:-}" "${6:-}" "${7:-}" "${8:-}" "${9:-}" | while IFS= read -r suite; do
            hs_fast_tier_suite "$suite"
        done
        echo knowhow-generated
        return 0
    fi
    hs_select_full_tier_suites "${1:-}" "${2:-}" "${3:-}" "${4:-}" "${6:-}" "${7:-}" "${8:-}" "${9:-}"
    echo knowhow-generated
}

# The full tier's selection, which hs_select_suites narrows for the fast tier.
# The cargo suites come first, in the order their lane runs them.
hs_select_full_tier_suites() { # [compile-inputs-file] [cli-inputs-file] [vitest-inputs-file] [drift-file] [touched-specs-file] [test-inputs-file] [crate-graph-file] [compile-owners-file]
    local inputs=${1:-/dev/null} cli_inputs=${2:-/dev/null} vitest_inputs=${3:-/dev/null} drift=${4:-/dev/null} touched=${5:-/dev/null} test_inputs=${6:-/dev/null} graph=${7:-/dev/null} owners=${8:-/dev/null} p s reach shell="" app="" cli="" gateway="" ts="" css="" vitest="" install="" release="" firstpaint="" self="" scope="" other_model="" codegen="" ort_download="" paths=""
    while IFS= read -r p; do
        [ -n "$p" ] || continue
        paths="$paths$p
"
        case "$p" in
            *.rs | Cargo.toml | */Cargo.toml | Cargo.lock | *.sql) ;;
            *)
                if grep -q -x -F -- "$p" "$inputs"; then
                    # A test-only include may name several crates' suites.
                    # engine-filtered comes from hs_engine_filters, below.
                    s=" $(awk -F '\t' -v p="$p" '$1 == p { printf "%s ", $2 }' "$test_inputs" 2>/dev/null)"
                    case "$s" in *" cli "*) cli=1 ;; esac
                    case "$s" in *" gateway "*) gateway=1 ;; esac
                    case "$s" in *" app-lib "*) app=1 ;; esac
                fi
                ;;
        esac
        case "$p" in
            # The CLI's build script renders its commands from the manifest.
            crates/lucidos-cli/* | crates/lucidos-capabilities/*) cli=1 ;;
            *) grep -q -x -F -- "$p" "$cli_inputs" 2>/dev/null && cli=1 ;;
        esac
        case "$p" in
            *.sh | .shellcheckrc | Makefile) shell=1 ;;
        esac
        case "$p" in
            crates/vendor/ort-sys/* | scripts/lib/onnxruntime_mirror*.sh) ort_download=1 ;;
        esac
        if hs_is_listed "$p" "$HS_GATEWAY_PINNED"; then gateway=1; fi
        case "$p" in
            scripts/codegen.sh | scripts/lib/codegen*.sh | scripts/lib/codegen_blocks.py | crates/lucidos-engine/build.rs) codegen=1 ;;
            install.sh | uninstall.sh | scripts/lib/service.sh | scripts/lib/stage_runtime*.sh | \
                scripts/lib/stage_runtime*.py | scripts/build-headless.sh | scripts/lib/install_test.sh | \
                scripts/lib/headless_tarball.sh | scripts/lib/install_common.sh) install=1 ;;
            scripts/release.sh | scripts/lib/release_draft.sh) release=1 ;;
            scripts/harden-suites.sh | scripts/lib/harden_suites*.sh | scripts/lib/proc_tree.sh | scripts/lib/touched_specs*.sh | \
                scripts/lib/e2e_projects.sh | scripts/lib/e2e_flaky.sh) self=1 ;;
            scripts/harden-scope.sh | scripts/lib/harden_scope_test.sh) scope=1 ;;
            scripts/harden-other-model-review.sh | scripts/lib/other_model_review.sh | scripts/lib/harden_other_model_review_test.sh) other_model=1 ;;
        esac
        case "$p" in
            *.ts | *.tsx) ts=1 ;;
            crates/lucidos-app/src/*.css) css=1 ;;
        esac
        # A superset of first-paint-growth.mjs's isFirstPaintInput, which decides.
        case "$p" in
            *.rs | */Cargo.toml) ;;
            crates/lucidos-app/* | packages/* | system-widgets/* | package.json | package-lock.json) firstpaint=1 ;;
        esac
    done
    # A changed path selects Vitest when it, or a directory above it, is named.
    printf '%s' "$paths" | awk 'NR == FNR { want[$0] = 1; next }
        { p = $0; while (p != "") { if (p in want) { hit = 1; exit } if (!sub(/\/[^\/]*$/, "", p)) break } }
        END { exit !hit }' "$vitest_inputs" - 2>/dev/null && vitest=1
    [ -s "$drift" ] && ts=1
    reach="$(printf '%s' "$paths" | hs_reached_crates "$inputs" "$test_inputs" "$graph" "$owners")"
    hs_reach_has "$reach" lucidos-app && app=1
    hs_reach_has "$reach" lucidos-cli && cli=1
    hs_reach_has "$reach" lucidos-gateway && gateway=1

    if [ -n "$reach" ]; then
        echo lint
    elif [ -n "$shell" ]; then
        echo shell-lint
    fi
    [ -n "$(hs_small_crates "$reach")" ] && echo crates
    [ -n "$gateway" ] && echo gateway
    [ -n "$cli" ] && echo cli
    [ -n "$app" ] && echo app-lib
    hs_reach_has "$reach" lucidos-eval && echo eval
    if hs_reach_has "$reach" lucidos-engine; then
        echo engine
    elif [ -n "$(printf '%s' "$paths" | hs_engine_filters "$test_inputs")" ]; then
        echo engine-filtered
    fi
    if [ -n "$ts" ]; then
        echo ts
    else
        [ -n "$css" ] && echo vite
        [ -n "$vitest" ] && echo vitest
    fi
    [ -n "$firstpaint" ] && echo first-paint
    [ -s "$touched" ] && echo e2e-touched
    [ -n "$(hs_webkit_specs < "$touched")" ] && echo e2e-touched-webkit
    [ -n "$install" ] && echo install
    [ -n "$release" ] && echo release
    [ -n "$self" ] && echo harden-suites
    [ -n "$scope" ] && echo harden-scope
    [ -n "$other_model" ] && echo harden-other-model-review
    [ -n "$ort_download" ] && echo ort-download
    [ -n "$codegen" ] && echo codegen
    return 0
}

# Read spec names on stdin and print the ones mobile-webkit runs.
hs_webkit_specs() {
    local spec
    while IFS= read -r spec; do
        [ -n "$spec" ] && project_runs_spec mobile-webkit "$spec" && echo "$spec"
    done
    return 0
}

# The e2e-browser.sh command for a list of specs: one -f each, after <flags>.
hs_specs_command() { # <specs-file> [flags]
    local spec cmd="./scripts/e2e-browser.sh${2:+ $2}"
    while IFS= read -r spec; do
        [ -n "$spec" ] && cmd="$cmd -f $(printf '%q' "$spec")"
    done < "$1"
    echo "$cmd"
}

# The engine tests a failed round named, by exact name.
hs_engine_tests_command() { # <names-file>
    echo "./scripts/test-engine.sh -- -- --exact $(tr '\n' ' ' < "$1" | sed 's/ *$//')"
}

# The suites whose failed tests hs_failed_tests can name.
hs_is_e2e_suite() {
    case "$1" in
        e2e-touched | e2e-touched-webkit) return 0 ;;
    esac
    return 1
}

hs_names_engine_tests() {
    case "$1" in
        engine | engine-filtered | driver) return 0 ;;
    esac
    return 1
}

# The lint command: clippy scoped to the crates the diff reaches. A diff that
# reaches every crate, or one the state cannot attribute, lints the workspace.
hs_lint_command() { # <state-dir>
    local reach crates="" c
    reach="$(hs_state_reach "$1" 2> /dev/null)"
    if [ -z "$reach" ] || [ "$reach" = '*' ] \
        || [ "$(printf '%s\n' "$reach" | wc -l)" -ge "$(wc -l < "$1/crate-graph")" ]; then
        echo "make lint"
        return 0
    fi
    for c in $reach; do crates="$crates -p $c"; done
    echo "make lint CLIPPY_SCOPE='${crates# }'"
}

# The small crates' test command, for the ones the diff reaches.
hs_crates_command() { # <state-dir>
    local small crates="" c
    small="$(hs_small_crates "$(hs_state_reach "$1" 2> /dev/null)")"
    if [ -z "$small" ] || [ "$small" = '*' ]; then
        echo "make test-crates"
        return 0
    fi
    for c in $small; do crates="$crates -p $c"; done
    echo "cargo test --locked${crates}"
}

# Print the shell command a suite runs, from the run's state directory: its
# changed paths, its touched specs, its start commit, and <suite>.retry. A
# non-empty <suite>.retry names the tests a failed round left, and the suite
# then runs those alone. HS_STUB_CMD, when set, replaces every command with
# "<stub> <suite>", so the tests never start a real suite. codegen-step is
# the cargo lane's first step, not a suite (hs_waits_for_codegen).
#
# e2e-touched runs on this host, a filtered run without mobile-webkit
# (e2e_github_handoff), which sends a busy host's run to GitHub (ADR 0584).
# e2e-touched-webkit sends the specs mobile-webkit runs
# to GitHub as the run starts, pinned to its start commit.
hs_suite_command() { # <suite> <early|normal> <state-dir>
    local suite=$1 mode=$2 state=$3 skips="" m specs
    if [ -n "${HS_STUB_CMD:-}" ]; then
        printf '%q %q\n' "$HS_STUB_CMD" "$suite"
        return 0
    fi
    if [ -s "$state/$suite.retry" ]; then
        if hs_names_engine_tests "$suite"; then
            hs_engine_tests_command "$state/$suite.retry"
            return 0
        fi
        hs_is_e2e_suite "$suite" && specs="$state/$suite.retry"
    fi
    case "$suite" in
        lint) hs_lint_command "$state" ;;
        ort-download) echo "./scripts/lib/onnxruntime_mirror_test.sh" ;;
        shell-lint) echo "make lint-shell lint-eval lint-knowhow lint-fmt" ;;
        crates) hs_crates_command "$state" ;;
        eval) echo "cargo test --locked -p lucidos-eval" ;;
        engine)
            if [ "$mode" = early ]; then
                for m in $HS_DRIVER_MODULES; do skips="$skips --skip $m"; done
                echo "./scripts/test-engine.sh -- --$skips"
            else
                echo "./scripts/test-engine.sh"
            fi
            ;;
        codegen-step) echo "./scripts/codegen.sh" ;;
        knowhow-generated) echo "./scripts/check-knowhow-generated.sh --cached-only" ;;
        app-lib) echo "cargo test --locked -p lucidos-app --lib" ;;
        cli) echo "cargo test --locked -p lucidos-cli" ;;
        gateway) echo "cargo test --locked -p lucidos-gateway" ;;
        engine-filtered) echo "./scripts/test-engine.sh -- -- $(hs_engine_filters "$state/test-inputs" < "$state/paths")" ;;
        driver) echo "./scripts/test-engine.sh -- -- $HS_DRIVER_MODULES" ;;
        ts) echo "cd crates/lucidos-app && npx tsc --noEmit && npm test --ignore-scripts && npx vite build" ;;
        vite) echo "cd crates/lucidos-app && npx vite build" ;;
        vitest) echo "cd crates/lucidos-app && npm test --ignore-scripts" ;;
        first-paint) echo "cd crates/lucidos-app && node first-paint-growth.mjs check" ;;
        e2e-touched) hs_specs_command "${specs:-$state/touched-specs}" ;;
        e2e-touched-webkit)
            if [ -z "${specs:-}" ]; then
                specs="$state/webkit-specs"
                hs_webkit_specs < "$state/touched-specs" > "$specs"
            fi
            hs_specs_command "$specs" "--github --webkit --commit $(cat "$state/start-sha")"
            ;;
        install) echo "bash scripts/lib/install_test.sh" ;;
        release) echo "bash scripts/lib/release_draft_test.sh && bash scripts/lib/release_rc_gate_test.sh" ;;
        harden-suites) echo "bash scripts/lib/harden_suites_test.sh && bash scripts/lib/touched_specs_test.sh" ;;
        harden-scope) echo "bash scripts/lib/harden_scope_test.sh" ;;
        harden-other-model-review) echo "bash scripts/lib/harden_other_model_review_test.sh" ;;
        codegen) echo "bash scripts/lib/codegen_test.sh && bash scripts/lib/codegen_blocks_test.sh" ;;
        *)
            echo "unknown suite: $suite" >&2
            return 1
            ;;
    esac
}

# Read a failure record on stdin ("<suite>" or "<suite> <test>" lines) and plan
# a retry round: write each suite's tests to <state>/<suite>.retry and print the
# suites to run. A spec the branch no longer has is dropped, and so is a
# touched-spec suite left with none. So is a record naming a suite that no
# longer exists, which leaves the next run a full one.
hs_retry_plan() { # <root> <state-dir>
    local root=$1 state=$2 suite name suites=""
    while read -r suite name; do
        [ -n "$suite" ] || continue
        # A suite this script no longer has, from a record an older run left.
        hs_suite_command "$suite" normal /dev/null > /dev/null 2>&1 || continue
        if [ -n "$name" ]; then
            if hs_is_e2e_suite "$suite" && [ ! -f "$root/crates/lucidos-app/e2e/$name" ]; then
                continue
            fi
            echo "$name" >> "$state/$suite.retry"
        elif hs_is_e2e_suite "$suite"; then
            continue
        fi
        case $'\n'"$suites" in
            *$'\n'"$suite"$'\n'*) ;;
            *) suites="$suites$suite"$'\n' ;;
        esac
    done
    printf '%s' "$suites"
}

# Print the tests a failed suite's log names, one per line: engine tests by
# their full name, browser specs by file. Prints "?" for a failure it cannot
# name, such as a test in another crate, so the caller reruns the whole suite.
# Prints nothing for a suite whose tests it never names.
hs_failed_tests() { # <suite> <log>
    if hs_names_engine_tests "$1"; then
        # A failure belongs to the test binary named by the last "Running" line.
        # test-engine.sh reruns only the engine library's tests.
        if awk '/^ *Running /{ engine = ($0 ~ /unittests src\/lib\.rs/ && $0 ~ /\/lucidos_engine-[0-9a-f]+\)/) }
            /^test .* \.\.\. FAILED$/ && !engine { found = 1 }
            END { exit !found }' "$2" 2>/dev/null; then
            echo "?"
        else
            e2e_failed_engine_tests "$2" 2>/dev/null
        fi
    elif hs_is_e2e_suite "$1"; then
        # Playwright's closing summary lists each failed test under "N failed".
        awk '/^ *[0-9]+ failed$/ { listing = 1; next }
            listing && /^ *\[[^]]+\] › / { sub(/^ *\[[^]]+\] › /, ""); sub(/:.*/, ""); n = split($0, part, "/"); print part[n]; next }
            { listing = 0 }' "$2" 2>/dev/null | sort -u
    fi
    return 0
}
