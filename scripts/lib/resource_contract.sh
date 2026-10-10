#!/usr/bin/env bash
# resource_contract.sh: the ONE list of staged runtime resources, and a check
# that can actually fail.
#
# WHY THIS EXISTS. build-dmg.sh's check_resource_contract used to compare
# RESOURCE_NAMES against resource_map_json(), two literals in the same file, so
# editing both together passed --check. build-headless.sh's --check was a printf
# and an exit 0, which cannot fail at all. The literals were unlinked, nothing
# compared the two vehicles, and the net effect was that system-knowhow could be
# dropped from Contents/Resources AND from the tarball with every gate green.
# The engine reads those docs live on every chat turn, so that bundle ships an
# assistant with no reference material. See ADR 0121.
#
# THE FIX IS WHAT THE CHECK COMPARES AGAINST. A list checked against another
# list in the same repo half proves nothing. resource_contract_check asserts a
# three-way set equality against the two RUNTIME launchers, neither of which the
# build scripts own:
#
#   * service_runtime_env_pairs (scripts/lib/service.sh): the env the headless
#     install's service runs the gateway with. Every resource appears in it as a
#     <runtime-root>/<name> path.
#   * spawn_gateway in crates/lucidos-app/src/desktop.rs: the packaged .app's own
#     launcher, whose env block and program resolve the same set relative to
#     Contents/Resources.
#
# BOTH ARMS READ USE, NOT DECLARATIONS. The packaged arm used to grep the
# *_RESOURCE_NAME constants, which proves a name exists rather than that anything
# is told where it lives. A resource could be declared, staged and signed while
# spawn_gateway never passed its path on, and every gate stayed green.
#
# Delete a name from RESOURCE_NAMES below and both other sources still carry it,
# so the check goes red. That is the property scripts/lib/resource_contract_test.sh
# proves, in all three directions.
#
# Sourced by build-dmg.sh and build-headless.sh only. Deliberately NOT by
# stage_runtime.sh: install.sh fetches that lib over the network when piped, so a
# new transitive dependency would have to be published beside it.

# service_runtime_env_pairs lives here, and is the headless launcher half of the
# check. Guarded so a caller that already sourced it does not pay for it twice.
if ! declare -f service_runtime_env_pairs >/dev/null 2>&1; then
    # shellcheck source=scripts/lib/service.sh
    source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/service.sh"
fi

# The sentinel runtime root the runtime-required set is derived against. Any
# absolute path works; this one cannot collide with a real install.
RESOURCE_CONTRACT_PROBE_ROOT="/__resource_contract_probe__"

# resource_contract_names: the staged resource set, one name per line. THE
# single source of truth. Everything else in this file derives from it, and both
# build scripts read their RESOURCE_NAMES array out of it.
resource_contract_names() {
    cat <<'EOF'
lucidos-engine
lucidos-gateway
lucidos
frontend
postgres
sdk
system-knowhow
EOF
}

# resource_contract_executables: the staged Mach-O files, one per line. Each must
# be codesigned (or notarization rejects the bundle), and they are a strict
# subset of the names above.
resource_contract_executables() {
    cat <<'EOF'
lucidos-engine
lucidos-gateway
lucidos
EOF
}

# resource_contract_tauri_map_json: the bundle.resources map as inner JSON object
# members (`"bundle-resources/<name>":"<name>"`, comma separated). DERIVED, so
# the map and the list cannot disagree. `cargo tauri build` copies only what this
# names, so a resource missing here never reaches Contents/Resources.
resource_contract_tauri_map_json() {
    local name sep=""
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        printf '%s"bundle-resources/%s":"%s"' "$sep" "$name" "$name"
        sep=","
    done < <(resource_contract_names)
}

# resource_contract_runtime_required: the resource names the HEADLESS launcher
# reaches for, derived from the two functions that define it. Each env pair whose
# value starts with the probe root contributes that path's first segment, e.g.
# LUCIDOS_PG_BIN_DIR=<root>/postgres/bin yields `postgres`. service_runtime_program
# supplies the gateway, which is the program rather than a variable and so appears
# in no pair. Sorted and unique.
resource_contract_runtime_required() {
    {
        service_runtime_env_pairs "$RESOURCE_CONTRACT_PROBE_ROOT" /probe-data 5252
        service_runtime_program "$RESOURCE_CONTRACT_PROBE_ROOT" | sed 's/^/PROGRAM=/'
    } | sed -n "s|^[A-Z0-9_]*=$RESOURCE_CONTRACT_PROBE_ROOT/\([^/]*\).*|\1|p" | sort -u
}

# resource_contract_desktop_names <desktop.rs>: the resource names the PACKAGED
# launcher USES, read out of spawn_gateway. Sorted and unique. A source scan
# rather than a link, because lucidos-app is a Rust crate and this is shell.
#
# Three hops, all inside desktop.rs:
#
#   1. spawn_gateway's `.env(…)` lines and its `Command::new(&bundle.…)` program
#      give the BundledResources fields the launcher hands to the gateway.
#   2. bundled_resources maps each field to a `*_RESOURCE_NAME` constant, through
#      at most one local binding (`let postgres = resources.join(…)`).
#   3. that constant's own declaration gives the staged name.
#
# Anything it cannot follow is an ERROR, never a silent drop. A scan that stopped
# reading the launcher would report an empty gap as a clean contract. So a
# refactor past these shapes (a field bound to a local before the `.env` call,
# say) goes red here and is fixed by teaching this scan the new shape.
resource_contract_desktop_names() {
    local src="$1" out rc
    [ -f "$src" ] || { echo "ERROR: desktop launcher source not found: $src" >&2; return 1; }
    out="$(awk -v src="$src" '
        # The patterns reach grab as STRINGS. A /re/ literal passed as an
        # argument is evaluated against $0 first, so grab would receive 0 or 1.
        function grab(s, re) {
            if (match(s, re)) return substr(s, RSTART, RLENGTH)
            return ""
        }
        /^const [A-Z0-9_]+: &str = "/ {
            key = $2; sub(/:$/, "", key)
            val = $0; sub(/^[^"]*"/, "", val); sub(/".*$/, "", val)
            value[key] = val
            next
        }
        /^fn bundled_resources\(/ { in_map = 1; saw_map = 1; next }
        in_map && /^}/ { in_map = 0; next }
        in_map {
            const_ref = grab($0, "join\\([A-Z0-9_]+\\)")
            sub(/^join\(/, "", const_ref); sub(/\)$/, "", const_ref)
            local_name = grab($0, "^[ \t]*let [a-z_][a-z0-9_]*")
            sub(/^[ \t]*let /, "", local_name)
            if (local_name != "") {
                if (const_ref != "") local_const[local_name] = const_ref
                next
            }
            field = grab($0, "^[ \t]*[a-z_][a-z0-9_]*:")
            sub(/^[ \t]*/, "", field); sub(/:$/, "", field)
            if (field == "") next
            if (const_ref == "") {
                receiver = grab($0, "[a-z_][a-z0-9_]*\\.join\\(")
                sub(/\.join\($/, "", receiver)
                const_ref = local_const[receiver]
            }
            if (const_ref != "") field_const[field] = const_ref
            next
        }
        /^fn spawn_gateway\(/ { in_spawn = 1; saw_spawn = 1; next }
        in_spawn && /^}/ { in_spawn = 0; next }
        in_spawn && (/\.env\(/ || /Command::new\(/) {
            rest = $0
            while (match(rest, /bundle\.[a-z_][a-z0-9_]*/)) {
                used[substr(rest, RSTART + 7, RLENGTH - 7)] = 1
                rest = substr(rest, RSTART + RLENGTH)
            }
        }
        END {
            if (!saw_spawn) { print "ERROR: " src ": no top-level spawn_gateway to read"; exit 1 }
            if (!saw_map) { print "ERROR: " src ": no top-level bundled_resources to read"; exit 1 }
            names = ""; count = 0
            for (f in used) {
                c = field_const[f]
                if (c == "") { print "ERROR: " src ": spawn_gateway uses bundle." f ", which bundled_resources maps to no resource"; exit 1 }
                if (value[c] == "") { print "ERROR: " src ": bundle." f " needs " c ", which no const declares"; exit 1 }
                names = names value[c] "\n"
                count++
            }
            if (count == 0) { print "ERROR: " src ": spawn_gateway hands the gateway no bundled resource"; exit 1 }
            printf "%s", names
        }
    ' "$src")"; rc=$?
    [ "$rc" -eq 0 ] || { printf '%s\n' "$out" >&2; return 1; }
    printf '%s\n' "$out" | sort -u
}

# _resource_contract_diff <label> <staged-lines> <required-lines>: report a set
# mismatch in the two directions a reader needs, and return non-zero on any.
_resource_contract_diff() {
    local label="$1" staged="$2" required="$3" name rc=0
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        case $'\n'"$required"$'\n' in
            *$'\n'"$name"$'\n'*) ;;
            *) echo "ERROR: '$name' is staged but $label does not use it" >&2; rc=1 ;;
        esac
    done <<< "$staged"
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        case $'\n'"$staged"$'\n' in
            *$'\n'"$name"$'\n'*) ;;
            *) echo "ERROR: $label needs '$name' at runtime but nothing stages it" >&2; rc=1 ;;
        esac
    done <<< "$required"
    return "$rc"
}

# resource_contract_check <desktop.rs>: assert the staged set equals what BOTH
# launchers reach for, and that every bundled executable is staged. Prints the
# contract on success. Returns non-zero, naming each offending resource, on any
# mismatch; the caller decides whether that is a die or a return.
resource_contract_check() {
    local desktop_rs="$1" staged rc=0
    staged="$(resource_contract_names | sort -u)"

    _resource_contract_diff "the headless service env (service_runtime_env_pairs)" \
        "$staged" "$(resource_contract_runtime_required)" || rc=1

    local desktop
    desktop="$(resource_contract_desktop_names "$desktop_rs")" || return 1
    _resource_contract_diff "the packaged launcher (desktop.rs)" "$staged" "$desktop" || rc=1

    local exe
    while IFS= read -r exe; do
        [ -n "$exe" ] || continue
        case $'\n'"$staged"$'\n' in
            *$'\n'"$exe"$'\n'*) ;;
            *) echo "ERROR: bundled executable '$exe' is not a staged resource" >&2; rc=1 ;;
        esac
    done < <(resource_contract_executables)

    [ "$rc" -eq 0 ] || return 1
    printf 'OK: resource contract holds for %s\n' \
        "$(resource_contract_names | tr '\n' ' ' | sed 's/ $//')"
}

# resource_contract_assert_staged <stage-dir>: assert the tree that was ACTUALLY
# written holds exactly the contract set. The check above reads declarations;
# this one reads the disk, so a caller that forgot to pass a resource to
# stage_runtime_assemble is caught by the stage rather than by a user.
resource_contract_assert_staged() {
    local stage="$1" name rc=0
    [ -d "$stage" ] || { echo "ERROR: stage dir not found: $stage" >&2; return 1; }
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        [ -e "$stage/$name" ] || { echo "ERROR: staged tree is missing '$name': $stage/$name" >&2; rc=1; }
    done < <(resource_contract_names)

    local staged_names entry
    staged_names="$(resource_contract_names)"
    for entry in "$stage"/*; do
        [ -e "$entry" ] || continue
        name="$(basename "$entry")"
        case $'\n'"$staged_names"$'\n' in
            *$'\n'"$name"$'\n'*) ;;
            *) echo "ERROR: staged tree holds '$name', which is not in the resource contract" >&2; rc=1 ;;
        esac
    done
    return "$rc"
}
