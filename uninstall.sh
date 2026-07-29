#!/usr/bin/env bash
#
# uninstall.sh — stop + unregister Lucidos gateway USER-service instances installed
# by install.sh, and remove their launchd plist / systemd unit.
#
#   ./uninstall.sh                  # remove the sole instance (or list if several)
#   ./uninstall.sh --name test      # remove a named instance
#   ./uninstall.sh --all            # remove every instance
#   ./uninstall.sh --list           # list instances + ports (no changes)
#   ./uninstall.sh --name test --purge   # also delete that instance's data
#   ./uninstall.sh --all --purge         # also delete every instance's data + the shared runtime
#   curl -fsSL https://raw.githubusercontent.com/lucidos-dev/lucidos/main/uninstall.sh | sh
#
# This is step 4 of docs/plans/2026-06-30-installer-step4-service-mode.md. It is
# DATA-SAFE by default: it stops the service, gracefully stops the engines +
# embedded Postgres the gateway spawned, removes the plist/unit, and then prints
# exactly what it left on disk. It deletes data ONLY with --purge.
#
# Instances are SLUG-KEYED (see scripts/lib/service.sh): each is `<prefix>/<slug>/`
# with a slug-suffixed service id; the runtime at `<prefix>/runtime` is SHARED. It
# cleans BOTH launchd and systemd artifacts that exist, so it works regardless of
# which manager registered the service. `install.sh --uninstall` delegates here.
#
# ── bash re-exec guard (POSIX sh → bash) ─────────────────────────────────────
LUCIDOS_UNINSTALL_SELF_URL="${LUCIDOS_UNINSTALL_SELF_URL:-https://raw.githubusercontent.com/lucidos-dev/lucidos/main/uninstall.sh}"
if [ -z "${BASH_VERSION:-}" ]; then
    if command -v bash >/dev/null 2>&1; then
        if [ -f "$0" ] && [ -r "$0" ]; then
            exec bash "$0" "$@"
        else
            _lucidos_payload="$(curl -fsSL "$LUCIDOS_UNINSTALL_SELF_URL")" || _lucidos_payload=""
            if [ -z "$_lucidos_payload" ]; then
                echo "ERROR: could not re-fetch the uninstaller from $LUCIDOS_UNINSTALL_SELF_URL to run it under bash." >&2
                echo "       Re-run explicitly under bash:  curl -fsSL $LUCIDOS_UNINSTALL_SELF_URL | bash" >&2
                exit 1
            fi
            exec bash -c "$_lucidos_payload" bash "$@"
        fi
    else
        echo "ERROR: this uninstaller requires bash, which was not found on PATH." >&2
        exit 1
    fi
fi

set -euo pipefail

# ── configuration (all overridable via environment) ─────────────────────────
LUCIDOS_PREFIX="${LUCIDOS_PREFIX:-$HOME/.lucidos}"            # install prefix used at install time
LUCIDOS_INSTANCE="${LUCIDOS_INSTANCE:-}"                     # target instance slug (--name); empty = the sole instance, or list if several
LUCIDOS_GATEWAY_DATA="${LUCIDOS_GATEWAY_DATA:-}"            # override the (single) instance data dir; empty = <prefix>/<slug>
LUCIDOS_ALL="${LUCIDOS_ALL:-}"                              # set to 1 (or --all) to act on every instance
LUCIDOS_LIST="${LUCIDOS_LIST:-}"                            # set to 1 (or --list) to just list instances
LUCIDOS_PURGE="${LUCIDOS_PURGE:-}"                          # set to 1 (or --purge) to delete data (with --all, also the shared runtime)
LUCIDOS_LIB_BASE_URL="${LUCIDOS_LIB_BASE_URL:-}"           # base URL for service.sh when piped (curl|sh)
LUCIDOS_INSTALL_URL="${LUCIDOS_INSTALL_URL:-https://raw.githubusercontent.com/lucidos-dev/lucidos/main/install.sh}"

# ── output helpers (match install.sh) ────────────────────────────────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_BOLD="$(printf '\033[1m')"; C_DIM="$(printf '\033[2m')"
    C_BLUE="$(printf '\033[34m')"; C_GREEN="$(printf '\033[32m')"
    C_YELLOW="$(printf '\033[33m')"; C_RED="$(printf '\033[31m')"; C_RESET="$(printf '\033[0m')"
else
    C_BOLD=""; C_DIM=""; C_BLUE=""; C_GREEN=""; C_YELLOW=""; C_RED=""; C_RESET=""
fi
step() { printf '%s==>%s %s%s%s\n' "$C_BLUE" "$C_RESET" "$C_BOLD" "$*" "$C_RESET"; }
info() { printf '    %s\n' "$*"; }
ok()   { printf '%s  ✓%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
warn() { printf '%s  !%s %s\n' "$C_YELLOW" "$C_RESET" "$*" >&2; }
die()  { printf '\n%sERROR:%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# ── service lib sourcing (checkout-local, else fetched) ──────────────────────
uninstall_self_dir() {
    local src=""
    if [ -n "${BASH_SOURCE:-}" ] && [ -f "${BASH_SOURCE[0]:-}" ]; then
        src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    fi
    printf '%s' "$src"
}

# Source scripts/lib/service.sh — the SAME pure helpers install.sh uses.
source_service_lib() {
    local self_dir base tmp
    self_dir="$(uninstall_self_dir)"
    if [ -n "$self_dir" ] && [ -f "$self_dir/scripts/lib/service.sh" ]; then
        # shellcheck source=scripts/lib/service.sh
        . "$self_dir/scripts/lib/service.sh" || die "Could not source $self_dir/scripts/lib/service.sh"
        return 0
    fi
    base="${LUCIDOS_LIB_BASE_URL:-${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib}"
    tmp="$(mktemp -d)"
    step "Fetching uninstaller helper library"
    info "$base/service.sh"
    curl -fsSL "$base/service.sh" -o "$tmp/service.sh" || die "Could not fetch service.sh from $base"
    [ -s "$tmp/service.sh" ] || die "Fetched service.sh from $base is empty."
    # The fetched copy IS scripts/lib/service.sh from the same ref, so point
    # ShellCheck at the in-repo original rather than the runtime temp path.
    # shellcheck source=scripts/lib/service.sh
    . "$tmp/service.sh" || die "Could not source the fetched service.sh."
}

# instance_data_dir <slug> — the data dir for <slug>: the override (only honored
# for a single explicit instance) else <prefix>/<slug>.
instance_data_dir() {
    local slug="$1"
    if [ -n "$LUCIDOS_GATEWAY_DATA" ] && [ -n "$LUCIDOS_INSTANCE" ]; then
        printf '%s' "$LUCIDOS_GATEWAY_DATA"
    else
        service_instance_data_dir "$LUCIDOS_PREFIX" "$slug"
    fi
}

# instance_status <slug> — a short "(launchd loaded)" / "(systemd active)" /
# "(stopped)" label for --list.
instance_status() {
    local slug="$1"
    if have launchctl && service_launchd_is_loaded "$(id -u)" "$(service_launchd_label "$slug")"; then
        printf '(launchd loaded)'; return 0
    fi
    if have systemctl && systemctl --user show-environment >/dev/null 2>&1 \
        && service_systemd_is_active "$(service_systemd_unit_name "$slug")"; then
        printf '(systemd active)'; return 0
    fi
    printf '(stopped)'
}

# ── list ─────────────────────────────────────────────────────────────────────
do_list() {
    local slug data port any=0
    step "Installed Lucidos instances ($LUCIDOS_PREFIX)"
    while IFS= read -r slug; do
        [ -n "$slug" ] || continue
        any=1
        data="$(service_instance_data_dir "$LUCIDOS_PREFIX" "$slug")"
        port="$(tr -d '[:space:]' < "$(service_instance_port_file "$data")" 2>/dev/null || true)"
        info "$(printf '%-16s port %-6s %s' "$slug" "${port:-?}" "$(instance_status "$slug")")"
    done <<EOF
$(service_list_instance_names "$LUCIDOS_PREFIX")
EOF
    [ "$any" = "1" ] || info "(none)"
}

# ── remove one instance ──────────────────────────────────────────────────────
remove_instance() {
    local slug="$1" data runtime_root removed=0
    data="$(instance_data_dir "$slug")"
    runtime_root="$(service_runtime_root "$LUCIDOS_PREFIX")"
    step "Removing instance '$slug'"

    # launchd (macOS) — bootout + remove its plist, if present.
    if have launchctl; then
        local label plist uid
        label="$(service_launchd_label "$slug")"
        plist="$(service_launchd_plist_path "$HOME" "$slug")"
        uid="$(id -u)"
        if service_launchd_is_loaded "$uid" "$label"; then
            if service_launchd_unload "$uid" "$label"; then
                ok "Stopped launchd agent $label"
            else
                warn "Could not bootout $label (try: launchctl bootout gui/$uid/$label)"
            fi
            removed=1
        fi
        if [ -f "$plist" ]; then
            if rm -f "$plist"; then ok "Removed $plist"; else warn "Could not remove $plist"; fi
            removed=1
        fi
    fi

    # systemd --user (Linux) — disable + remove its unit, if present. The unit
    # FILE is removed even when the user D-Bus session is unreachable (common
    # over bare ssh: no XDG_RUNTIME_DIR/bus) — otherwise an "uninstalled"
    # service resurrects at the next boot. Only the stop/disable calls need the
    # bus; they stay best-effort behind the probe.
    local bus_ok=0 service_stopped=1
    if have systemctl && systemctl --user show-environment >/dev/null 2>&1; then
        bus_ok=1
    fi
    local unit_name unit_path
    unit_name="$(service_systemd_unit_name "$slug")"
    unit_path="$(service_systemd_unit_path "$HOME" "$slug" "${XDG_CONFIG_HOME:-}")"
    if [ "$bus_ok" = "1" ]; then
        if service_systemd_is_active "$unit_name" || [ -f "$unit_path" ]; then
            service_systemd_unload "$unit_name"
            ok "Stopped + disabled $unit_name"
            removed=1
        fi
    elif [ -f "$unit_path" ]; then
        # Can't reach the user manager, so a running gateway can't be stopped
        # from here — and killing its engines would only make it respawn them.
        warn "systemd --user session unreachable — cannot stop a running $unit_name from this shell."
        info "If it is running, stop it from a login session:  systemctl --user stop $unit_name"
        service_stopped=0
    fi
    if [ -f "$unit_path" ]; then
        if rm -f "$unit_path"; then ok "Removed $unit_path"; else warn "Could not remove $unit_path"; fi
        [ "$bus_ok" = "1" ] && { systemctl --user daemon-reload >/dev/null 2>&1 || true; }
        removed=1
    fi

    if [ "$removed" = "1" ] && [ "$service_stopped" = "1" ]; then
        # Best-effort: gracefully stop the engines + embedded Postgres this instance
        # spawned (the gateway is unregistered above, so it can't respawn them).
        service_stop_embedded_runtime "$data" "$runtime_root"
        ok "Stopped instance '$slug' embedded runtime"
    elif [ "$removed" = "1" ]; then
        # The gateway may still be running (unreachable user bus) and would
        # respawn anything we kill — leave its processes alone; the removed unit
        # file stops it from coming back after the next logout/reboot.
        info "Left the possibly-running gateway + engines alone (no user bus); they stop at next logout/reboot."
    else
        info "Instance '$slug' had no registered service (nothing to stop)."
    fi

    # Data: keep unless --purge.
    if [ -n "$LUCIDOS_PURGE" ]; then
        local t
        while IFS= read -r t; do
            [ -n "$t" ] || continue
            if [ -e "$t" ]; then
                if rm -rf "$t"; then ok "Deleted $t"; else warn "Could not delete $t"; fi
            fi
        done <<EOF
$(service_uninstall_purge_targets "$data")
EOF
    else
        KEPT_DATA="$KEPT_DATA$data
"
    fi
}

# ── resolve which instances to act on ────────────────────────────────────────
# Sets the TARGETS array (slugs). --all → every instance; --name → that one; bare
# → the sole instance, or a refusal listing the choices when several exist.
TARGETS=()
resolve_targets() {
    local -a all=()
    local slug
    while IFS= read -r slug; do
        [ -n "$slug" ] && all+=("$slug")
    done <<EOF
$(service_list_instance_names "$LUCIDOS_PREFIX")
EOF

    if [ -n "$LUCIDOS_ALL" ]; then
        # Guard the empty case: under stock macOS bash 3.2 + `set -u`,
        # `TARGETS=("${all[@]}")` on an EMPTY array errors "all[@]: unbound
        # variable". `${#all[@]}` (count) is safe; only the value expansion is not.
        if [ "${#all[@]}" -gt 0 ]; then TARGETS=("${all[@]}"); fi
        return 0
    fi
    if [ -n "$LUCIDOS_INSTANCE" ]; then
        service_is_instance_name "$LUCIDOS_INSTANCE" \
            || die "--name must be a valid instance slug (got '$LUCIDOS_INSTANCE')."
        TARGETS=("$LUCIDOS_INSTANCE")
        return 0
    fi
    case "${#all[@]}" in
        0) TARGETS=() ;;
        1) TARGETS=("${all[0]}") ;;
        *) die "Several instances are installed: ${all[*]}.
       Pass --name <slug> to remove one, or --all to remove every instance.
       (Run with --list to see them + their ports.)" ;;
    esac
}

# ── the uninstall flow ───────────────────────────────────────────────────────
KEPT_DATA=""
run_uninstall() {
    source_service_lib

    if [ -n "$LUCIDOS_LIST" ]; then
        do_list
        return 0
    fi

    resolve_targets
    if [ "${#TARGETS[@]}" -eq 0 ]; then
        info "No Lucidos instances are installed under $LUCIDOS_PREFIX (nothing to remove)."
        return 0
    fi

    local slug
    for slug in "${TARGETS[@]}"; do
        remove_instance "$slug"
    done

    # --all --purge also removes the SHARED runtime (no instance needs it anymore).
    if [ -n "$LUCIDOS_ALL" ] && [ -n "$LUCIDOS_PURGE" ]; then
        local runtime="$LUCIDOS_PREFIX/runtime"
        if [ -e "$runtime" ]; then
            if rm -rf "$runtime"; then ok "Deleted the shared runtime $runtime"; else warn "Could not delete $runtime"; fi
        fi
    fi

    print_summary
}

print_summary() {
    if [ -z "$LUCIDOS_PURGE" ] && [ -n "$KEPT_DATA" ]; then
        printf '\n'
        info "Left your data in place (re-run with --purge to delete it):"
        printf '%s' "$KEPT_DATA" | while IFS= read -r d; do
            [ -n "$d" ] && info "  $d"
        done
        [ -n "$LUCIDOS_ALL" ] && info "  $LUCIDOS_PREFIX/runtime  (shared runtime)"
    fi
    printf '\n%s========================================%s\n' "$C_GREEN" "$C_RESET"
    if [ -n "$LUCIDOS_PURGE" ]; then
        printf '%s  Lucidos uninstalled + purged ✓%s\n' "$C_BOLD" "$C_RESET"
    else
        printf '%s  Lucidos service removed ✓%s\n' "$C_BOLD" "$C_RESET"
    fi
    printf '%s========================================%s\n\n' "$C_GREEN" "$C_RESET"
}

# ── help ─────────────────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Lucidos uninstaller

Usage:
  ./uninstall.sh                  remove the sole instance (or list if several); KEEP data
  ./uninstall.sh --name SLUG      remove a named instance
  ./uninstall.sh --all            remove every instance
  ./uninstall.sh --list           list instances + ports (no changes)
  ./uninstall.sh --name SLUG --purge   also delete that instance's data
  ./uninstall.sh --all --purge         also delete all data + the shared runtime
  curl -fsSL <url>/uninstall.sh | sh

Instances are slug-keyed: each lives at <prefix>/<slug>/ with a service id
com.lucidos.gateway.<slug> (launchd) / lucidos-gateway-<slug>.service (systemd);
the runtime at <prefix>/runtime is SHARED. This stops + removes the service,
gracefully stops the engines + embedded Postgres, and — unless --purge — leaves
your data on disk and prints where it is.

Flags:
  --name SLUG    target instance (default: the sole instance, else refuse + list)
  --all          act on every instance
  --list         list installed instances + ports, then exit
  --prefix DIR   install prefix (default: \$HOME/.lucidos)
  --purge        also delete the instance data (with --all, also the shared runtime)
  -h, --help     this help

Environment variables:
  LUCIDOS_PREFIX         same as --prefix
  LUCIDOS_INSTANCE       same as --name
  LUCIDOS_GATEWAY_DATA   override the (single) instance data dir
  LUCIDOS_ALL=1          same as --all
  LUCIDOS_PURGE=1        same as --purge
  LUCIDOS_LIB_BASE_URL   base URL for service.sh when piped (curl|sh)
EOF
}

# ── argument parsing ────────────────────────────────────────────────────────
parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --name)     [ $# -ge 2 ] || die "--name requires an argument"; LUCIDOS_INSTANCE="$2"; shift 2 ;;
            --prefix)   [ $# -ge 2 ] || die "--prefix requires an argument"; LUCIDOS_PREFIX="$2"; shift 2 ;;
            --all)      LUCIDOS_ALL=1; shift ;;
            --list)     LUCIDOS_LIST=1; shift ;;
            --purge)    LUCIDOS_PURGE=1; shift ;;
            -h|--help)  usage; exit 0 ;;
            *) die "unknown argument: $1  (run 'uninstall.sh --help' for usage)" ;;
        esac
    done
}

# ── main ────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    printf '%s%sLucidos uninstaller%s\n' "$C_BOLD" "$C_BLUE" "$C_RESET"
    printf '%smode=uninstall prefix=%s name=%s all=%s purge=%s%s\n\n' \
        "$C_DIM" "$LUCIDOS_PREFIX" "${LUCIDOS_INSTANCE:-<auto>}" "${LUCIDOS_ALL:+yes}" "${LUCIDOS_PURGE:+yes}" "$C_RESET"
    run_uninstall
}

main "$@"
