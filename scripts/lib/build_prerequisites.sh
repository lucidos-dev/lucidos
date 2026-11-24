#!/usr/bin/env bash
# Pure prerequisite readers. Callers supply paths from the process's memory
# hierarchy, including every ancestor limit: a child may say max while its
# parent is constrained. No swap is included in usable compile capacity.
# These functions do not install packages, acquire slots or run compilers.

bp_linux_memory_verdict() { # <floor-GiB> <meminfo> [<limit-file> <usage-file> ...]
    local floor=$1 meminfo=$2 limit usage value current readings="" available
    shift 2
    if ! awk -v floor="$floor" 'BEGIN { exit !(floor ~ /^[0-9]+([.][0-9]+)?$/) }'; then
        echo 'ERROR: invalid build memory floor' >&2
        return 1
    fi
    available="$(awk '$1 == "MemAvailable:" && $2 ~ /^[0-9]+$/ && $3 == "kB" { printf "%.0f\n", $2 * 1024; found++ }
        END { if (found != 1) exit 1 }' "$meminfo" 2>/dev/null)" || {
        echo 'ERROR: cannot read Linux MemAvailable for build prerequisites' >&2
        return 1
    }
    while [ $# -gt 0 ]; do
        if [ $# -lt 2 ]; then
            echo 'ERROR: cgroup limit needs a matching usage file' >&2
            return 1
        fi
        limit=$1 usage=$2
        shift 2
        if ! value="$(cat "$limit" 2>/dev/null)" || ! current="$(cat "$usage" 2>/dev/null)"; then
            echo 'ERROR: cannot read cgroup memory limit/usage' >&2
            return 1
        fi
        case "$value" in
            max) ;;
            '' | *[!0-9]*) echo 'ERROR: invalid cgroup memory limit' >&2; return 1 ;;
        esac
        case "$current" in
            '' | *[!0-9]*) echo 'ERROR: invalid cgroup memory usage' >&2; return 1 ;;
        esac
        readings="$readings$value $current
"
    done
    printf '%s' "$readings" | awk -v available="$available" -v floor="$floor" '
        BEGIN { usable = available; limiting = "host" }
        NF == 2 && $1 != "max" {
            headroom = $1 - $2
            if (headroom < 0) headroom = 0
            if (headroom < usable) { usable = headroom; limiting = $1 }
        }
        END {
            printf "Build memory: usable=%.2f GiB, floor=%.2f GiB, limiting=%s bytes (host means MemAvailable)\n", usable / 1073741824, floor, limiting
            if (usable < floor * 1073741824) exit 72
        }'
}

# Resolve the current process's memory controller, including ancestor limits.
# A proc-directory argument is a fixture seam, never a fallback to live probes.
bp_linux_memory_check() { # <floor-GiB> [proc-directory]
    local floor=$1 proc=${2:-/proc} pairs path
    local files=()
    pairs="$(python3 - "$proc" <<'PY'
import pathlib
import re
import sys


def decode(value):
    return re.sub(r"\\([0-7]{3})", lambda m: chr(int(m[1], 8)), value)


try:
    proc = pathlib.Path(sys.argv[1])
    controllers = []
    for line in (proc / 'self/cgroup').read_text().splitlines():
        _, names, group = line.split(':', 2)
        if not names or 'memory' in names.split(','):
            controllers.append(('cgroup2' if not names else 'cgroup', group))
    if any(kind == "cgroup" for kind, _ in controllers):
        controllers = [(kind, group) for kind, group in controllers if kind == "cgroup"]
    mounts = []
    for line in (proc / 'self/mountinfo').read_text().splitlines():
        before, after = line.split(' - ', 1)
        fields, options = before.split(), after.split()
        if options[0] == 'cgroup2' or (
                options[0] == 'cgroup' and 'memory' in options[2].split(',')):
            mounts.append((options[0], decode(fields[3]), decode(fields[4])))
    for kind, group in controllers:
        matched = False
        for fs, root, mount in mounts:
            if kind != fs:
                continue
            # Membership is absolute within the hierarchy. A namespaced mount
            # may expose only a subtree; never walk outside its visible root.
            try:
                relative = pathlib.PurePosixPath(group).relative_to(root)
            except ValueError:
                continue
            if '..' in relative.parts:
                raise ValueError('invalid cgroup membership')
            base = pathlib.Path(mount)
            current = base / relative
            limit, usage = (('memory.max', 'memory.current') if fs == 'cgroup2'
                            else ('memory.limit_in_bytes', 'memory.usage_in_bytes'))
            while True:
                if (fs == 'cgroup2' and not (current / limit).exists()
                        and (current / 'cgroup.controllers').is_file()
                        and 'memory' not in (current / 'cgroup.controllers').read_text().split()):
                    if current == base:
                        break
                    current = current.parent
                    continue
                # The global v2 root has no memory.max interface.
                # A constrained namespace root does expose one; inspect it.
                if (current == base and root == '/' and fs == 'cgroup2'
                        and not (current / limit).exists()):
                    break
                for name in (limit, usage):
                    path = str(current / name)
                    if '\n' in path:
                        raise ValueError('newline in cgroup path')
                    print(path)
                if current == base:
                    break
                current = current.parent
            matched = True
            break
        if not matched:
            raise ValueError('cannot resolve memory controller mount')
except (OSError, ValueError, IndexError) as error:
    print(f'ERROR: cannot discover cgroup memory prerequisites: {error}', file=sys.stderr)
    sys.exit(1)
PY
)" || return 1
    while IFS= read -r path; do
        [ -z "$path" ] || files+=("$path")
    done <<< "$pairs"
    if [ -n "$pairs" ]; then
        bp_linux_memory_verdict "$floor" "$proc/meminfo" "${files[@]}"
    else
        bp_linux_memory_verdict "$floor" "$proc/meminfo"
    fi
}

# Read a complete vm_stat snapshot as "available-GiB compressor-GiB".
# Unlike the browser guard's permissive readers, hardening must distinguish
# a valid zero from a missing counter. Page size is reported by the OS, not
# inferred from the architecture (Intel and Apple Silicon differ).
bp_macos_vm_snapshot() { # <vm_stat-output-file>
    awk '
        /page size of/ {
            headers++
            for (i = 1; i < NF; i++) if ($i == "of") {
                raw = $(i + 1)
                if (raw !~ /^[0-9]+$/ || raw + 0 <= 0) bad = 1
                page = raw + 0
            }
        }
        /^Pages free:/ { key = "free"; count = $NF; record() }
        /^Pages speculative:/ { key = "spec"; count = $NF; record() }
        /^Pages purgeable:/ { key = "purge"; count = $NF; record() }
        /^File-backed pages:/ { key = "file"; count = $NF; record() }
        /^Pages occupied by compressor:/ { key = "compressor"; count = $NF; record() }
        function record() {
            sub(/[.]$/, "", count)
            if (count !~ /^[0-9]+$/ || seen[key]++) bad = 1
            counters[key] = count + 0
        }
        END {
            if (bad || headers != 1 || page <= 0 || !seen["free"] ||
                !seen["spec"] || !seen["purge"] || !seen["file"] || !seen["compressor"]) {
                print "ERROR: incomplete or malformed macOS vm_stat snapshot" > "/dev/stderr"
                exit 1
            }
            available = counters["free"] + counters["spec"] + counters["purge"] + counters["file"]
            printf "%.2f %.2f\n", available * page / 1073741824, counters["compressor"] * page / 1073741824
        }
    ' "$1"
}

# Caller sources host_memory_guard.sh before using this adapter. Keep the
# browser policy in one place; only hardening requires every probe to succeed.
# The subshell pins the verified initial readings without leaking overrides.
bp_macos_memory_check() (
    local floor=$1 snapshot available compressor level swap physical reasons
    snapshot="$(vm_stat 2>/dev/null)" || {
        echo 'ERROR: cannot read macOS vm_stat for build prerequisites' >&2
        return 1
    }
    snapshot="$(bp_macos_vm_snapshot /dev/stdin <<< "$snapshot")" || return 1
    read -r available compressor <<< "$snapshot"
    level="$(_host_mem_read_pressure_level)"
    swap="$(_host_mem_read_swap_used_gb)"
    physical="$(_host_mem_read_physical_gb)"
    case "$level" in
        1|2|4) ;;
        *) echo 'ERROR: cannot read macOS memory pressure for build prerequisites' >&2; return 1 ;;
    esac
    if ! _host_mem_is_number "$floor" || ! _host_mem_is_number "$swap" ||
        ! _host_mem_is_number "$physical" || ! _host_mem_over "$physical" 0; then
        echo 'ERROR: invalid build floor or unreadable macOS swap/physical memory' >&2
        return 1
    fi
    export HOST_AVAIL_GB_OVERRIDE=$available
    export HOST_COMPRESSOR_GB_OVERRIDE=$compressor
    export HOST_SWAP_USED_GB_OVERRIDE=$swap
    export HOST_PHYSMEM_GB_OVERRIDE=$physical
    # Pin only the initial pressure; shared confirmation still re-reads the host.
    echo "Build memory: available=$available GiB, floor=$floor GiB, physical=$physical GiB, compressor=$compressor GiB, pressure=$level"
    if reasons="$(host_memory_gate_verdict "$floor" "$level")"; then
        return 0
    fi
    printf 'ERROR: build refused on host memory: %s\n' "$reasons" >&2
    return 72
)

# Probe only the selected target's pkg-config environment. Requirements are
# package:minimum-version pairs derived by the caller from compile selection.
# Match pkg-config-rs precedence; never treat host metadata as cross evidence.
bp_native_packages_check() { # <host-triple> <target-triple> [package:minimum ...]
    local host=$1 target=$2 normalized tool='' name value item package minimum version status=0 cross=0 kind=HOST
    shift 2
    [ $# -gt 0 ] || return 0
    normalized=${target//-/_}
    [ "$host" = "$target" ] || { cross=1; kind=TARGET; }
    for name in "PKG_CONFIG_$target" "PKG_CONFIG_$normalized" "${kind}_PKG_CONFIG" PKG_CONFIG; do
        value="$(printenv "$name" 2>/dev/null)" || value=''
        if [ -n "$value" ]; then tool=$value; break; fi
    done
    if [ "$cross" -eq 1 ]; then
        local configured=0
        case "$name" in "PKG_CONFIG_$target"|"PKG_CONFIG_$normalized"|TARGET_PKG_CONFIG) configured=1 ;; esac
        for name in "PKG_CONFIG_SYSROOT_DIR_$target" "PKG_CONFIG_SYSROOT_DIR_$normalized" TARGET_PKG_CONFIG_SYSROOT_DIR PKG_CONFIG_SYSROOT_DIR; do
            value="$(printenv "$name" 2>/dev/null)" || value=''
            [ -z "$value" ] || configured=1
        done
        if [ "${PKG_CONFIG_ALLOW_CROSS:-0}" = 1 ]; then
            for name in "PKG_CONFIG_PATH_$target" "PKG_CONFIG_PATH_$normalized" TARGET_PKG_CONFIG_PATH \
                "PKG_CONFIG_LIBDIR_$target" "PKG_CONFIG_LIBDIR_$normalized" TARGET_PKG_CONFIG_LIBDIR; do
                value="$(printenv "$name" 2>/dev/null)" || value=''
                [ -z "$value" ] || configured=1
            done
        fi
        if [ "$configured" -eq 0 ]; then
            echo "ERROR: configure a target pkg-config executable or sysroot for $target; host metadata cannot certify cross compilation" >&2
            return 1
        fi
    fi
    tool=${tool:-pkg-config}
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "ERROR: missing pkg-config executable: $tool (target $target)" >&2
        return 1
    fi
    # pkg-config-rs translates target-qualified path variables for its child.
    # Do the same in a subshell so a target probe cannot alter subsequent ones.
    (
        for item in PKG_CONFIG_PATH PKG_CONFIG_LIBDIR PKG_CONFIG_SYSROOT_DIR; do
            for name in "${item}_$target" "${item}_$normalized" "${kind}_$item" "$item"; do
                if value="$(printenv "$name" 2>/dev/null)"; then
                    export "$item=$value"
                    break
                fi
            done
        done
        for item in "$@"; do
            package=${item%%:*} minimum=${item#*:}
            if [ "$package" = "$item" ] || [ -z "$package" ] || [ -z "$minimum" ]; then
                echo "ERROR: malformed native requirement: $item" >&2
                status=1
            elif ! version="$("$tool" --modversion "$package" 2>/dev/null)"; then
                echo "ERROR: missing or inaccessible $package metadata for $target" >&2
                status=1
            elif ! awk -v v="$version" 'BEGIN { exit !(v ~ /^[0-9]+([.][0-9]+)*$/) }'; then
                echo "ERROR: malformed $package version from pkg-config: $version" >&2
                status=1
            elif ! "$tool" --atleast-version="$minimum" "$package"; then
                echo "ERROR: $package $version is below required $minimum for $target" >&2
                status=1
            else
                echo "Native prerequisite: $package $version (minimum $minimum, target $target)"
            fi
        done
        return "$status"
    )
}

# Resolve the selected macOS SDK without compiling. SDKROOT is also honored by
# native build tooling; use it when supplied rather than certifying another SDK.
bp_macos_sdk_check() {
    local sdk=${SDKROOT:-} compiler status=0
    if ! command -v xcrun >/dev/null 2>&1; then
        echo 'ERROR: missing xcrun; install or select Apple developer tools' >&2
        return 1
    fi
    if [ -z "$sdk" ]; then
        if ! sdk="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null)"; then
            echo 'ERROR: cannot discover macOS SDK; select an installed developer directory' >&2
            status=1
        fi
    fi
    if [ -z "$sdk" ] || [ ! -d "$sdk" ] || [ ! -r "$sdk" ] || [ ! -x "$sdk" ]; then
        echo "ERROR: inaccessible macOS SDK: ${sdk:-<empty>}; check SDKROOT and developer tools" >&2
        status=1
    else
        echo "Native prerequisite: macOS SDK $sdk"
    fi
    compiler="$(xcrun --sdk macosx --find clang 2>/dev/null)" || compiler=''
    if [ -z "$compiler" ] || [ ! -f "$compiler" ] || [ ! -x "$compiler" ]; then
        echo "ERROR: inaccessible macOS compiler: ${compiler:-<empty>}; select installed Apple developer tools" >&2
        status=1
    else
        echo "Native prerequisite: macOS compiler $compiler"
    fi
    return "$status"
}

# The caller derives desktop/headless from its selected Cargo targets, not the
# machine's installed packages. Memory always describes the executing host;
# native packages describe the compile target. No slot is acquired here.
bp_build_check() { # <host-OS> <host-triple> <target-triple> <desktop|headless> <all|memory> <floor-GiB>
    local os=$1 host=$2 target=$3 selection=$4 mode=$5 floor=$6 memory_rc=0 native_rc=0
    case "$selection" in desktop|headless) ;; *) echo 'ERROR: invalid build prerequisite selection' >&2; return 2 ;; esac
    case "$mode" in all|memory) ;; *) echo 'ERROR: invalid build prerequisite mode' >&2; return 2 ;; esac
    case "$os" in
        Linux) bp_linux_memory_check "$floor" || memory_rc=$? ;;
        Darwin) bp_macos_memory_check "$floor" || memory_rc=$? ;;
        *) echo "ERROR: unsupported build memory host: $os" >&2; memory_rc=1 ;;
    esac
    if [ "$mode" = all ]; then
        case "$target" in
            *-apple-darwin)
                if [ "$os" = Darwin ]; then
                    bp_macos_sdk_check || native_rc=$?
                else
                    echo 'ERROR: macOS SDK preflight requires a macOS host' >&2
                    native_rc=1
                fi ;;
            *-linux-*)
                if [ "$selection" = desktop ]; then
                    # Minima from the selected Tauri/GTK/WebKit dependency
                    # metadata. AppIndicator is dynamically loaded, not linked.
                    bp_native_packages_check "$host" "$target" \
                        glib-2.0:2.70 gobject-2.0:2.70 gio-2.0:2.70 \
                        gtk+-3.0:3.24 gdk-3.0:3.22 webkit2gtk-4.1:2.40 \
                        javascriptcoregtk-4.1:2.38 libsoup-3.0:3.0 \
                        atk:2.28 cairo:1.14 cairo-gobject:1.14 \
                        pango:1.40 gdk-pixbuf-2.0:2.36.8 || native_rc=$?
                fi ;;
            *) echo "ERROR: unsupported build target: $target" >&2; native_rc=1 ;;
        esac
    fi
    # A missing prerequisite takes precedence over retryable memory pressure,
    # but both diagnostics have already been collected.
    [ "$native_rc" -eq 0 ] || return 1
    return "$memory_rc"
}

# Ask Cargo for the same package/feature selection as the upcoming compile.
# The caller supplies the effective target and selection arguments (including
# --all-features where used). --offline forbids fetching missing dependencies;
# an incomplete graph is a refusal, never evidence for headless selection.
# tree reads manifests/lockfiles only; it never invokes a compiler or build.rs.
bp_cargo_selection() { # <effective-target-triple> [Cargo package/feature flags]
    local target=$1 graph
    shift
    if ! graph="$(cargo tree --locked --offline --color never --prefix none \
        --edges normal,build,dev --format '{p}' --target "$target" "$@")"; then
        echo 'ERROR: cannot resolve selected Cargo dependencies for native prerequisites' >&2
        return 1
    fi
    if [ -z "$graph" ]; then
        echo 'ERROR: empty selected Cargo dependency graph' >&2
        return 1
    fi
    printf '%s\n' "$graph" | awk '
        $1 == "gtk-sys" || $1 == "gdk-sys" || $1 == "webkit2gtk-sys" ||
        $1 == "tauri-runtime-wry" { desktop = 1 }
        END { print desktop ? "desktop" : "headless" }'
}
