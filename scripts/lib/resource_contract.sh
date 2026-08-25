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
#   * the *_RESOURCE_NAME constants in crates/lucidos-app/src/desktop.rs: the
#     packaged .app's own launcher, which resolves the same set relative to
#     Contents/Resources.
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
# launcher reaches for, read out of its *_RESOURCE_NAME constants. Sorted and
# unique. A source scan rather than a link, because lucidos-app is a Rust crate
# and this is shell; the build scripts are the only thing that can compare them.
resource_contract_desktop_names() {
    local src="$1"
    [ -f "$src" ] || { echo "ERROR: desktop launcher source not found: $src" >&2; return 1; }
    sed -n 's|^const [A-Z0-9_]*_RESOURCE_NAME: &str = "\([^"]*\)";.*|\1|p' "$src" | sort -u
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
