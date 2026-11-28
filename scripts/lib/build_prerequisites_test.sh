#!/usr/bin/env bash
# Hermetic prerequisite arithmetic tests: all readings are fixture files.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Remove inherited target settings, including names containing hyphens, before
# the fixture starts. env -u accepts names Bash cannot pass to unset.
if [ "${BP_NATIVE_ENV_CLEAN:-0}" != 1 ]; then
    clean_env=(env)
    while IFS='=' read -r name _; do
        case "$name" in
            PKG_CONFIG*|HOST_PKG_CONFIG*|TARGET_PKG_CONFIG*) clean_env+=( -u "$name" ) ;;
        esac
    done < <(env)
    exec "${clean_env[@]}" BP_NATIVE_ENV_CLEAN=1 bash "$0"
fi
# shellcheck source=scripts/lib/build_prerequisites.sh
source "$SCRIPT_DIR/build_prerequisites.sh"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
printf 'MemAvailable: 16777216 kB\nSwapFree: 999999999 kB\n' > "$fixture/meminfo"
printf 'max\n' > "$fixture/limit"
printf '0\n' > "$fixture/usage"
assert_verdict() {
    local expected=$1 rc=0
    shift
    bp_linux_memory_verdict "$@" > "$fixture/output" 2>&1 || rc=$?
    if [ "$rc" -ne "$expected" ]; then
        cat "$fixture/output"
        echo "expected $expected, got $rc" >&2
        return 1
    fi
}
assert_verdict 0 8 "$fixture/meminfo"
assert_verdict 0 8 "$fixture/meminfo" "$fixture/limit" "$fixture/usage"
printf '10737418240\n' > "$fixture/limit"
printf '3221225472\n' > "$fixture/usage"
assert_verdict 72 8 "$fixture/meminfo" "$fixture/limit" "$fixture/usage"
grep -q 'usable=7.00 GiB.*floor=8.00 GiB' "$fixture/output"
assert_verdict 0 4 "$fixture/meminfo" "$fixture/limit" "$fixture/usage"
printf '2147483648\n' > "$fixture/parent-limit"
printf '1073741824\n' > "$fixture/parent-usage"
assert_verdict 72 4 "$fixture/meminfo" "$fixture/limit" "$fixture/usage" "$fixture/parent-limit" "$fixture/parent-usage"
printf '999999999999\n' > "$fixture/usage"
assert_verdict 72 8 "$fixture/meminfo" "$fixture/limit" "$fixture/usage"
printf 'broken\n' > "$fixture/limit"
assert_verdict 1 8 "$fixture/meminfo" "$fixture/limit" "$fixture/usage"
assert_verdict 1 8 "$fixture/meminfo" "$fixture/limit"
assert_verdict 1 8 "$fixture/missing"
assert_verdict 1 invalid "$fixture/meminfo"
printf 'SwapFree: 999999999 kB\n' > "$fixture/meminfo"
assert_verdict 1 8 "$fixture/meminfo"
printf 'MemAvailable: wrong kB\n' > "$fixture/meminfo"
assert_verdict 1 8 "$fixture/meminfo"
printf 'MemAvailable: 8388608 kB\n' > "$fixture/meminfo"
assert_verdict 0 8 "$fixture/meminfo"
printf 'MemAvailable: 8388607 kB\n' > "$fixture/meminfo"
assert_verdict 72 8 "$fixture/meminfo"
echo 'build_prerequisites_test: PASS (14 synthetic memory cases)'

# Probe discovery uses only synthetic proc and mount files.
mkdir -p "$fixture/proc/self" "$fixture/cgroup/child"
printf 'MemAvailable: 16777216 kB\n' > "$fixture/proc/meminfo"
printf '0::/child\n' > "$fixture/proc/self/cgroup"
printf '1 0 0:1 / %s rw - cgroup2 cgroup rw\n' "$fixture/cgroup" > "$fixture/proc/self/mountinfo"
printf 'max\n' > "$fixture/cgroup/child/memory.max"
printf '0\n' > "$fixture/cgroup/child/memory.current"
bp_linux_memory_check 8 "$fixture/proc" > "$fixture/output"
printf '4294967296\n' > "$fixture/cgroup/memory.max"
printf '1073741824\n' > "$fixture/cgroup/memory.current"
rc=0
bp_linux_memory_check 8 "$fixture/proc" > "$fixture/output" || rc=$?
[ "$rc" -eq 72 ]
grep -q 'usable=3.00 GiB' "$fixture/output"
# A subtree mount exposes only its own ancestry, never the host root.
printf '1 0 0:1 /child %s rw - cgroup2 cgroup rw\n' "$fixture/cgroup/child" > "$fixture/proc/self/mountinfo"
bp_linux_memory_check 8 "$fixture/proc" > "$fixture/output"
# v1 uses its controller-specific file names.
printf '2:cpu,memory:/child\n' > "$fixture/proc/self/cgroup"
printf '1 0 0:1 /child %s rw - cgroup cgroup rw,memory\n' "$fixture/cgroup/child" > "$fixture/proc/self/mountinfo"
printf '17179869184\n' > "$fixture/cgroup/child/memory.limit_in_bytes"
printf '0\n' > "$fixture/cgroup/child/memory.usage_in_bytes"
bp_linux_memory_check 8 "$fixture/proc" > "$fixture/output"
rm "$fixture/proc/self/mountinfo"
rc=0
bp_linux_memory_check 8 "$fixture/proc" > "$fixture/output" 2>&1 || rc=$?
[ "$rc" -eq 1 ]
echo 'build_prerequisites_test: PASS (5 synthetic discovery cases)'

# macOS snapshots accept actual zero counts, and reject partial/malformed data.
assert_macos_snapshot() {
    local expected=$1 rc=0
    bp_macos_vm_snapshot "$fixture/vm_stat" > "$fixture/output" 2>&1 || rc=$?
    [ "$rc" -eq "$expected" ] || { cat "$fixture/output"; return 1; }
}
write_vm_snapshot() {
    cat > "$fixture/vm_stat" <<EOF2
Mach Virtual Memory Statistics: (page size of $1 bytes)
Pages free: 524288.
Pages speculative: 0.
Pages purgeable: 0.
File-backed pages: 524288.
Pages occupied by compressor: 0.
EOF2
}
write_vm_snapshot 16384
assert_macos_snapshot 0
[ "$(cat "$fixture/output")" = '16.00 0.00' ]
write_vm_snapshot 4096
assert_macos_snapshot 0
[ "$(cat "$fixture/output")" = '4.00 0.00' ]
printf 'Mach Virtual Memory Statistics: (page size of 16384 bytes)\n' > "$fixture/vm_stat"
assert_macos_snapshot 1
write_vm_snapshot 0
assert_macos_snapshot 1
write_vm_snapshot 16384
printf 'Pages free: 1.\n' >> "$fixture/vm_stat"
assert_macos_snapshot 1
write_vm_snapshot 16384
printf 'Pages occupied by compressor: broken.\n' >> "$fixture/vm_stat"
assert_macos_snapshot 1
echo 'build_prerequisites_test: PASS (6 synthetic macOS snapshot cases)'

# Adapter fixtures never call the host: vm_stat and every shared reader are
# stubbed, including pressure confirmation (which normally sleeps/re-probes).
# shellcheck source=scripts/lib/host_memory_guard.sh
source "$SCRIPT_DIR/host_memory_guard.sh"
vm_stat() { cat "$fixture/vm_stat"; }
_host_mem_read_pressure_level() { printf '%s' "$test_pressure"; }
_host_mem_read_swap_used_gb() { printf '%s' "$test_swap"; }
_host_mem_read_physical_gb() { printf '%s' "$test_physical"; }
_host_mem_critical_persists() { return 1; }
assert_macos_memory() {
    local expected=$1 rc=0
    bp_macos_memory_check 8 > "$fixture/output" 2>&1 || rc=$?
    [ "$rc" -eq "$expected" ] || { cat "$fixture/output"; return 1; }
}
write_vm_snapshot 4096
test_pressure=1 test_swap=0 test_physical=16
assert_macos_memory 0 # low availability alone is not distress on macOS
test_pressure=2
assert_macos_memory 72 # corroborated shortage
test_pressure=1 test_swap=1
assert_macos_memory 72
test_swap='' # missing instrumentation is a refusal, not GO
assert_macos_memory 1
test_swap=0 test_pressure=broken
assert_macos_memory 1
test_pressure=1 test_physical=0
assert_macos_memory 1
echo 'build_prerequisites_test: PASS (6 synthetic macOS policy cases)'

# Critical pressure without corroboration uses the shared confirmation policy.
write_vm_snapshot 16384
test_physical=48 test_pressure=4
assert_macos_memory 0
_host_mem_critical_persists() { printf '8 8 40 critical'; return 0; }
assert_macos_memory 72
# A runaway compressor is independently refused, even at normal pressure.
test_pressure=1
sed 's/Pages occupied by compressor: 0./Pages occupied by compressor: 2000000./' "$fixture/vm_stat" > "$fixture/vm_next"
mv "$fixture/vm_next" "$fixture/vm_stat"
assert_macos_memory 72
# shellcheck disable=SC2329 # Called indirectly by assert_macos_memory before restoring the reader.
vm_stat() { return 1; }
assert_macos_memory 1
echo 'build_prerequisites_test: PASS (4 synthetic macOS confirmation/failure cases)'

# The strict adapter must judge the pressure it validated, even if a second
# boundary read would be unreadable. Confirmation still probes independently.
write_vm_snapshot 4096
vm_stat() { cat "$fixture/vm_stat"; }
_host_mem_read_pressure_level() {
    if [ ! -f "$fixture/pressure-read" ]; then
        touch "$fixture/pressure-read"
        echo 4
    else
        echo broken
    fi
}
assert_macos_memory 72
rm "$fixture/pressure-read"
echo 'build_prerequisites_test: PASS (validated pressure survives reader failure)'

# Native metadata probes use a function stub, never installed packages.
fixture_pkg_config() {
    printf '%s %s path=%s\n' "$1" "$2" "${PKG_CONFIG_PATH:-}" >> "$fixture/native-calls"
    case "$2" in
        absent) return 1 ;;
        malformed) echo broken ;;
        old) case "$1" in --modversion) echo 1.0 ;; *) return 1 ;; esac ;;
        *) case "$1" in --modversion) echo 9.0 ;; esac ;;
    esac
}
export PKG_CONFIG=fixture_pkg_config
assert_native() {
    local expected=$1 rc=0
    shift
    bp_native_packages_check "$@" > "$fixture/output" 2>&1 || rc=$?
    [ "$rc" -eq "$expected" ] || { cat "$fixture/output"; return 1; }
}
assert_native 0 host host good:2.0
assert_native 1 host host absent:2.0 old:2.0 malformed:2.0
grep -q 'missing or inaccessible absent' "$fixture/output"
grep -q 'old 1.0 is below' "$fixture/output"
grep -q 'malformed malformed version' "$fixture/output"
assert_native 1 host cross good:2.0
export PKG_CONFIG_cross=fixture_pkg_config PKG_CONFIG_PATH_cross=/synthetic-target
assert_native 0 host cross good:2.0
grep -q 'path=/synthetic-target' "$fixture/native-calls"
export PKG_CONFIG_cross=missing-target-tool
assert_native 1 host cross good:2.0
assert_native 0 host cross # no desktop requirements means no tool requirement
unset PKG_CONFIG PKG_CONFIG_cross PKG_CONFIG_PATH_cross
# Native host precedence must beat an unqualified missing executable.
export PKG_CONFIG=missing-native-tool HOST_PKG_CONFIG=fixture_pkg_config
assert_native 0 host host good:2.0
unset HOST_PKG_CONFIG
assert_native 1 host host good:2.0
grep -q 'missing pkg-config executable' "$fixture/output"
export PKG_CONFIG=fixture_pkg_config TARGET_PKG_CONFIG_SYSROOT_DIR=/synthetic-sysroot
assert_native 0 host cross good:2.0
unset TARGET_PKG_CONFIG_SYSROOT_DIR
export PKG_CONFIG_x86_64_unknown_linux_gnu=fixture_pkg_config
assert_native 0 host x86_64-unknown-linux-gnu good:2.0
# env supports the literal target-triple names that Bash export cannot set.
export -f fixture_pkg_config
export fixture
# shellcheck disable=SC2016 # The child Bash expands its positional parameter.
env PKG_CONFIG_x86_64-unknown-linux-gnu=fixture_pkg_config \
    PKG_CONFIG_x86_64_unknown_linux_gnu=missing-normalized-tool \
    bash -c 'source "$1"; bp_native_packages_check host x86_64-unknown-linux-gnu good:2.0' \
    bash "$SCRIPT_DIR/build_prerequisites.sh" > "$fixture/output" 2>&1
unset PKG_CONFIG_x86_64_unknown_linux_gnu
export PKG_CONFIG=fixture_pkg_config
assert_native 1 host host malformed-requirement
assert_native 1 host host absent:2.0
echo 'build_prerequisites_test: PASS (13 synthetic native package cases)'

# SDK probes never invoke a compiler or inspect the host's developer directory.
mkdir -p "$fixture/sdk" "$fixture/tools"
printf '#!/bin/sh\ntrue\n' > "$fixture/tools/clang"
chmod +x "$fixture/tools/clang"
xcrun() {
    case "$*" in
        '--sdk macosx --show-sdk-path')
            [ "${fixture_sdk_failure:-0}" = 0 ] || return 1
            printf '%s\n' "${fixture_sdk_path:-$fixture/sdk}" ;;
        '--sdk macosx --find clang') printf '%s\n' "${fixture_clang:-$fixture/tools/clang}" ;;
        *) return 1 ;;
    esac
}
assert_sdk() {
    local expected=$1 rc=0
    bp_macos_sdk_check > "$fixture/output" 2>&1 || rc=$?
    if [ "$rc" -ne "$expected" ]; then
        cat "$fixture/output"
        echo "expected SDK status $expected, got $rc" >&2
        return 1
    fi
}
unset SDKROOT
assert_sdk 0
fixture_sdk_failure=1
assert_sdk 1
grep -q 'cannot discover macOS SDK' "$fixture/output"
export SDKROOT="$fixture/sdk"
assert_sdk 0 # an explicit accessible SDK does not need SDK discovery
SDKROOT="$fixture/missing-sdk"
fixture_clang="$fixture/missing-clang"
assert_sdk 1
grep -q 'inaccessible macOS SDK' "$fixture/output"
grep -q 'inaccessible macOS compiler' "$fixture/output"
unset SDKROOT fixture_clang
fixture_sdk_failure=0 fixture_sdk_path=''
# A successful command with an empty response still fails closed.
xcrun() { return 0; }
assert_sdk 1
grep -q 'inaccessible macOS SDK' "$fixture/output"
grep -q 'inaccessible macOS compiler' "$fixture/output"
echo 'build_prerequisites_test: PASS (5 synthetic macOS SDK cases)'
(
    unset -f xcrun
    export PATH="$fixture/empty-path"
    assert_sdk 1
)
grep -q 'missing xcrun' "$fixture/output"
echo 'build_prerequisites_test: PASS (missing developer tools)'

# Orchestration aggregates memory and native failures, without invoking probes.
# This subshell preserves the real helper definitions for other fixture groups.
(
    bp_linux_memory_check() { echo memory >> "$fixture/order"; return "${fixture_memory_rc:-0}"; }
    bp_macos_memory_check() { echo mac-memory >> "$fixture/order"; return 0; }
    bp_macos_sdk_check() { echo sdk >> "$fixture/order"; return 0; }
    bp_native_packages_check() { printf '%s\n' "$@" > "$fixture/packages"; return "${fixture_native_rc:-0}"; }
    assert_build() {
        local expected=$1 rc=0
        shift
        : > "$fixture/order"
        rm -f "$fixture/packages"
        bp_build_check "$@" > "$fixture/output" 2>&1 || rc=$?
        [ "$rc" -eq "$expected" ] || { cat "$fixture/output"; return 1; }
    }
    assert_build 0 Linux x86_64-unknown-linux-gnu x86_64-unknown-linux-gnu headless all 8
    [ ! -e "$fixture/packages" ]
    assert_build 0 Linux x86_64-unknown-linux-gnu x86_64-unknown-linux-gnu desktop all 8
    grep -qx 'glib-2.0:2.70' "$fixture/packages"
    grep -qx 'javascriptcoregtk-4.1:2.38' "$fixture/packages"
    grep -qx 'gdk-pixbuf-2.0:2.36.8' "$fixture/packages"
    grep -qx 'cairo-gobject:1.14' "$fixture/packages"
    fixture_memory_rc=72 fixture_native_rc=1
    assert_build 1 Linux x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu desktop all 8
    grep -qx memory "$fixture/order"
    grep -qx aarch64-unknown-linux-gnu "$fixture/packages"
    fixture_native_rc=0
    assert_build 72 Linux x86_64-unknown-linux-gnu x86_64-unknown-linux-gnu desktop all 8
    assert_build 72 Linux x86_64-unknown-linux-gnu x86_64-unknown-linux-gnu desktop memory 8
    [ ! -e "$fixture/packages" ]
    assert_build 0 Darwin aarch64-apple-darwin aarch64-apple-darwin headless all 8
    grep -qx sdk "$fixture/order"
    [ ! -e "$fixture/packages" ]
    assert_build 2 Linux x86_64-unknown-linux-gnu x86_64-unknown-linux-gnu typo all 8
    [ ! -s "$fixture/order" ]
    assert_build 2 Linux x86_64-unknown-linux-gnu x86_64-unknown-linux-gnu desktop typo 8
    [ ! -s "$fixture/order" ]
)
echo 'build_prerequisites_test: PASS (8 orchestration cases)'

# Exercise the real CLI parser with fixture libraries, never live OS probes.
mkdir -p "$fixture/command/lib"
cp "$SCRIPT_DIR/../build-prerequisites.sh" "$fixture/command/"
cat > "$fixture/command/lib/build_prerequisites.sh" <<'SH'
bp_cargo_selection() {
    printf '%s\n' "$@" > "$BP_FIXTURE_CARGO_ARGS"
    [ "${BP_FIXTURE_CARGO_FAIL:-0}" = 0 ] || return 1
    echo desktop
}
bp_build_check() { printf '<%s>\n' "$@"; return 72; }
SH
cat > "$fixture/command/lib/host_memory_guard.sh" <<'SH'
host_memory_build_line_gb() { echo 5; }
SH
assert_command() {
    local expected=$1 rc=0
    shift
    bash "$fixture/command/build-prerequisites.sh" "$@" > "$fixture/output" 2>&1 || rc=$?
    [ "$rc" -eq "$expected" ] || { cat "$fixture/output"; return 1; }
}
assert_command 2
assert_command 2 --host
assert_command 2 --unknown
assert_command 72 --host fixture-host --target aarch64-fixture-linux-gnu --selection headless --memory
grep -qx '<fixture-host>' "$fixture/output"
grep -qx '<aarch64-fixture-linux-gnu>' "$fixture/output"
grep -qx '<headless>' "$fixture/output"
grep -qx '<memory>' "$fixture/output"
grep -qx '<5>' "$fixture/output"
export BP_FIXTURE_CARGO_ARGS="$fixture/cli-cargo-args"
assert_command 72 --host fixture-host --target aarch64-fixture-linux-gnu -- --workspace --all-features
grep -qx '<desktop>' "$fixture/output"
grep -qx -- '--all-features' "$fixture/cli-cargo-args"
assert_command 2 --host fixture-host --target aarch64-fixture-linux-gnu --selection headless -- --workspace
assert_command 2 --host fixture-host --target aarch64-fixture-linux-gnu --memory -- --workspace
assert_command 72 --host fixture-host --target aarch64-fixture-linux-gnu --
grep -qx '<desktop>' "$fixture/output"
assert_command 72 --host fixture-host -- --target aarch64-fixture-linux-gnu -p lucidos-app
grep -qx 'aarch64-fixture-linux-gnu' "$fixture/cli-cargo-args"
grep -qx -- '-p' "$fixture/cli-cargo-args"
export BP_FIXTURE_CARGO_FAIL=1
assert_command 1 --host fixture-host --target aarch64-fixture-linux-gnu -- --workspace
unset BP_FIXTURE_CARGO_FAIL BP_FIXTURE_CARGO_ARGS
echo 'build_prerequisites_test: PASS (10 CLI parser cases)'

# Selection reads Cargo's target-filtered dependency graph, never installed
# native packages or a textual scan of optional manifest dependencies.
(
    cargo() {
        printf '%s\n' "$@" > "$fixture/cargo-args"
        printf '%s\n' "$fixture_graph"
        return "$fixture_cargo_rc"
    }
    fixture_cargo_rc=0
    fixture_graph='lucidos-engine v1.0.0
libc v0.2.0'
    [ "$(bp_cargo_selection aarch64-fixture-linux-gnu -p lucidos-engine --all-features)" = headless ]
    grep -qx -- '--offline' "$fixture/cargo-args"
    grep -qx -- '--locked' "$fixture/cargo-args"
    grep -qx -- 'aarch64-fixture-linux-gnu' "$fixture/cargo-args"
    grep -qx -- '--all-features' "$fixture/cargo-args"
    fixture_graph='lucidos-app v1.0.0
gtk-sys v0.18.0 (*)'
    [ "$(bp_cargo_selection aarch64-fixture-linux-gnu --workspace --all-features)" = desktop ]
    fixture_graph='webkit2gtk-sys v2.0.0'
    [ "$(bp_cargo_selection aarch64-fixture-linux-gnu -p lucidos-app)" = desktop ]
    fixture_cargo_rc=1
    rc=0
    bp_cargo_selection aarch64-fixture-linux-gnu --workspace > "$fixture/output" 2>&1 || rc=$?
    [ "$rc" -eq 1 ]
    grep -q 'cannot resolve' "$fixture/output"
    fixture_cargo_rc=0 fixture_graph=''
    rc=0
    bp_cargo_selection aarch64-fixture-linux-gnu --workspace > "$fixture/output" 2>&1 || rc=$?
    [ "$rc" -eq 1 ]
)
echo 'build_prerequisites_test: PASS (5 synthetic Cargo selection cases)'

# Hybrid hierarchies bind memory to v1; unified membership is not a second
# memory controller. No live proc or cgroup paths are read here.
printf '2:memory:/child\n0::/child\n' > "$fixture/proc/self/cgroup"
printf '1 0 0:1 /child %s rw - cgroup cgroup rw,memory\n2 0 0:2 / %s rw - cgroup2 cgroup rw\n' \
    "$fixture/cgroup/child" "$fixture/unified" > "$fixture/proc/self/mountinfo"
bp_linux_memory_check 8 "$fixture/proc" > "$fixture/output"
export PKG_CONFIG=fixture_pkg_config PKG_CONFIG_ALLOW_CROSS=1 TARGET_PKG_CONFIG_LIBDIR=/synthetic-target
assert_native 0 host cross good:2.0
unset TARGET_PKG_CONFIG_LIBDIR
assert_native 1 host cross good:2.0
unset PKG_CONFIG_ALLOW_CROSS PKG_CONFIG
echo 'build_prerequisites_test: PASS (hybrid memory and qualified cross metadata)'

mkdir -p "$fixture/disabled/child"
printf '0::/child\n' > "$fixture/proc/self/cgroup"
printf '1 0 0:1 / %s rw - cgroup2 cgroup rw\n' "$fixture/disabled" > "$fixture/proc/self/mountinfo"
printf 'cpu io\n' > "$fixture/disabled/child/cgroup.controllers"
printf '4294967296\n' > "$fixture/disabled/memory.max"
printf '0\n' > "$fixture/disabled/memory.current"
rc=0
bp_linux_memory_check 8 "$fixture/proc" > "$fixture/output" 2>&1 || rc=$?
[ "$rc" -eq 72 ]
echo 'build_prerequisites_test: PASS (disabled v2 leaf preserves ancestor limit)'
