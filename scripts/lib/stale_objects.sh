#!/bin/bash
# Prunes superseded debug-object generations from a cargo `deps` directory.
#
# Under split-debuginfo "unpacked", cargo's macOS dev default, every build of a
# binary or test target writes `<artifact>.<cgu>.<generation>.rcgu.o` files.
# Nothing deletes the previous generation. Gatekeeper's library check reads the
# whole directory on each proc-macro load, so a large one stalls every rustc
# (ADR 0408). Temporary measure: docs/temporary-measures.md.

# The linked generation plus the newest other one, so an engine still running
# from the last build usually keeps its line numbers until the user switches.
STALE_OBJECT_GENERATIONS_KEPT=2

# Prints `<mtime> ./<name>` for every object directly in the current directory.
# A rustc in the same target deletes its own temporary objects, so a file can
# vanish before `stat` reads it. That only drops its line, so the status is
# not checked.
_list_debug_objects() {
    if stat -c '%Y' . >/dev/null 2>&1; then
        find . -maxdepth 1 -type f -name '*.rcgu.o' -exec stat -c '%Y %n' {} +
    else
        find . -maxdepth 1 -type f -name '*.rcgu.o' -exec stat -f '%m %N' {} +
    fi
    return 0
}

# Prints why the prune must wait, or nothing when no other cargo builds in this
# profile directory. Cargo holds `.cargo-lock` open for a whole build,
# including while it waits for the lock, and a build mid-link needs every name
# it made.
_cargo_build_lock_holder() {
    local lock="$1/../.cargo-lock"
    [ -e "$lock" ] || return 0
    if ! command -v lsof >/dev/null 2>&1; then
        echo "no lsof to tell whether another cargo is building here"
    elif lsof -t -- "$lock" >/dev/null 2>&1; then
        echo "another cargo is building here"
    fi
}

# Reads `_list_debug_objects` lines and prints each artifact holding more
# generations than are kept. Only those need their binary read.
_crowded_debug_artifacts() {
    awk -v keep="$STALE_OBJECT_GENERATIONS_KEPT" '
        $2 ~ /^\.\/[A-Za-z0-9_][A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9]+\.rcgu\.o$/ {
            split(substr($2, 3), part, ".")
            if (!((part[1] "." part[3]) in seen)) {
                seen[part[1] "." part[3]] = 1
                if (++gens[part[1]] == keep + 1) print part[1]
            }
        }'
}

# Prints `<artifact>.<generation>` for each generation an artifact's binary in
# the current directory names in its OSO stabs. Rustc hardlinks reused codegen
# units, so generations can share an mtime and only the binary tells them apart.
_linked_debug_generations() {
    local artifact
    command -v nm >/dev/null 2>&1 || return 0
    while read -r artifact; do
        [ -f "$artifact" ] || continue
        nm -ap "$artifact" 2>/dev/null | awk -v artifact="$artifact" '
            index($0, " OSO ") {
                n = split(substr($0, index($0, " OSO ") + 5), path, "/")
                split(path[n], part, ".")
                if (part[1] == artifact && path[n] ~ /\.rcgu\.o$/) print part[1] "." part[3]
            }'
    done | sort -u
}

# Reads `_list_debug_objects` lines and prints the names to delete. The name
# pattern admits no space or leading dash, so the output is safe for xargs.
_select_stale_debug_objects() {
    awk -v linked="$1" -v keep="$STALE_OBJECT_GENERATIONS_KEPT" '
        BEGIN {
            while ((getline g < linked) > 0) pinned[g] = 1
        }
        {
            name = substr($2, 3)
            if (name !~ /^[A-Za-z0-9_][A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9]+\.rcgu\.o$/) next
            split(name, part, ".")
            gen = part[1] "." part[3]
            if (!(gen in newest)) gens[part[1]] = gens[part[1]] " " gen
            if ($1 > newest[gen]) newest[gen] = $1
            gen_of[name] = gen
        }
        END {
            for (artifact in gens) {
                n = split(substr(gens[artifact], 2), list, " ")
                held = 0
                for (i = 1; i <= n; i++)
                    if (list[i] in pinned) { kept[list[i]] = 1; held++ }
                for (; held < keep && held < n; held++) {
                    best = ""
                    for (i = 1; i <= n; i++) {
                        g = list[i]
                        if (g in kept) continue
                        if (best == "" || newest[g] > newest[best]) best = g
                    }
                    kept[best] = 1
                }
            }
            for (name in gen_of)
                if (!(gen_of[name] in kept)) print name
        }'
}

# Never fails: a prune problem prints a warning and the build goes on.
prune_stale_debug_objects() {
    local deps="$1" work count holder
    if [ ! -d "$deps" ]; then
        echo "Stale debug objects: no $deps, nothing to prune."
        return 0
    fi
    holder="$(_cargo_build_lock_holder "$deps")"
    if [ -n "$holder" ]; then
        echo "Stale debug objects: $holder; left for the next build."
        return 0
    fi
    if ! work="$(mktemp -d "${TMPDIR:-/tmp}/stale-debug-objects.XXXXXX")"; then
        echo "WARNING: no temp directory for the stale debug object prune; skipped it."
        return 0
    fi
    if ! (cd "$deps" && _list_debug_objects > "$work/objects" &&
        _crowded_debug_artifacts < "$work/objects" | _linked_debug_generations > "$work/linked" &&
        _select_stale_debug_objects "$work/linked" < "$work/objects" > "$work/stale") 2>/dev/null; then
        echo "WARNING: could not list the debug objects in $deps; skipped the prune."
        rm -rf "$work"
        return 0
    fi
    # Reading the binaries takes seconds, long enough for another cargo to start.
    # One that starts after this check writes new generation names and links
    # from the incremental cache, so it never needs a name on this list.
    holder="$(_cargo_build_lock_holder "$deps")"
    if [ -n "$holder" ]; then
        echo "Stale debug objects: $holder; left for the next build."
        rm -rf "$work"
        return 0
    fi
    count="$(wc -l < "$work/stale" | tr -d ' ')"
    if [ "$count" -gt 0 ] && ! (cd "$deps" && xargs rm -f -- < "$work/stale"); then
        echo "WARNING: could not delete every stale debug object in $deps."
    fi
    rm -rf "$work"
    echo "Pruned $count stale debug objects from $deps."
}
