#!/usr/bin/env bash
# service.sh — register/unregister the bundled Lucidos GATEWAY as a USER-level
# service so a terminal install survives terminal-close + reboot and restarts on
# failure. Step 4 of docs/plans/2026-06-30-installer-step4-service-mode.md — the
# final step of the bundled-installer rework.
#
# SLUG-KEYED INSTANCES. Each gateway is an "instance" identified by a stable
# slug (`--name`, default `default`). The PORT is a MUTABLE PROPERTY of an
# instance, not its identity — so an instance's port can be changed on a re-run
# without orphaning its data dir. Multiple gateways coexist — each instance owns
# `<prefix>/<slug>/` (registry + embedded PG + fastembed + logs + a `port`
# marker), while the RUNTIME (binaries + PG + frontend + sdk) is downloaded once
# and SHARED at `<prefix>/runtime/current`. This is how a terminal install
# coexists with a dev gateway (`~/.lucidos/gateway`) and the packaged `.app`
# (port 5252, data under Application Support) without colliding on a port, a data
# dir, or a service id. The slugs `gateway`/`runtime`/`current` are reserved so a
# `--name` can never alias the dev data dir or the shared runtime.
#
# The service supervises the GATEWAY ONLY (never per-engine): the gateway is the
# long-lived supervisor (ADR 0014) that provisions the embedded Postgres and
# spawns/supervises one engine per workspace. This mirrors the packaged macOS
# .app model in crates/lucidos-app/src/desktop.rs, except the headless tarball has
# no Tauri `--service` wrapper, so the service runs `lucidos-gateway` DIRECTLY with
# the same env contract spawn_gateway sets.
#
# Two managers: launchd LaunchAgent (macOS) and systemd --user (Linux). The
# gateway ignores SIGTERM and stops gracefully on SIGUSR1 (crates/lucidos-gateway/
# src/server.rs install_shutdown), so the systemd unit sets KillSignal=SIGUSR1 +
# KillMode=process (stop the gateway gracefully; leave engines + PG for a relaunch
# to re-adopt — engine statelessness). launchd has no kill-signal knob, so a
# `bootout` SIGTERMs (ignored) then SIGKILLs after the exit timeout; Postgres is
# crash-safe, so that is acceptable.
#
# Split, exactly like stage_runtime.sh / install_common.sh:
#   • PURE helpers (identity, paths, templating, manager DECISION, compose
#     decision, env pairs, command-arg builders, slug/port validation, port
#     candidates, uninstall paths) — no side effects, unit-tested offline by
#     scripts/lib/service_test.sh.
#   • EFFECTFUL wrappers (the actual launchctl/systemctl/curl/kill/pg_ctl calls,
#     port probing, instance listing): thin, clearly sectioned, and never run
#     against a real service manager by the tests. The launchd pair is the one
#     exception the tests DO drive, through a stateful fake launchctl on PATH,
#     because its contract is a TIMING one that no pure helper can express (see
#     the async-bootout rule further down).
#
# Pure shell. No dependence on the caller's `set -e`; helpers return explicit
# status. Sourced by install.sh + uninstall.sh (locally from a checkout, else
# fetched from the same ref as the installer).

# ════════════════════════════════════════════════════════════════════════════
# PURE helpers — offline-testable; no side effects.
# ════════════════════════════════════════════════════════════════════════════

# ── identity (slug-suffixed labels / unit names) ─────────────────────────────

# service_launchd_label <slug> — the macOS LaunchAgent label for an instance.
# Slug-suffixed so instances coexist and never collide with the DMG's
# `com.lucidos.engine`.
service_launchd_label() { printf 'com.lucidos.gateway.%s' "$1"; }

# service_systemd_unit_name <slug> — the systemd --user unit name for an instance.
service_systemd_unit_name() { printf 'lucidos-gateway-%s.service' "$1"; }

# ── validation ───────────────────────────────────────────────────────────────

# service_is_instance_name <s> — true iff <s> is a valid instance slug: lowercase
# alphanumerics + internal dashes, and NOT a reserved name (`runtime`/`current`
# are the shared-runtime paths; `gateway` is the dev gateway's data dir; `logs`).
# LC_ALL=C forces byte-wise glob ranges — under a UTF-8 locale `[a-z]` collates to
# include uppercase, so without it "UP" would slip through as valid.
service_is_instance_name() {
    local LC_ALL=C
    case "$1" in
        ''|*[!a-z0-9-]*) return 1 ;;
        -*|*-) return 1 ;;
        runtime|current|gateway|logs) return 1 ;;
        *) return 0 ;;
    esac
}

# service_is_port_number <s> — true iff <s> is a valid 1..65535 port.
service_is_port_number() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
        *) [ "$1" -ge 1 ] 2>/dev/null && [ "$1" -le 65535 ] 2>/dev/null ;;
    esac
}

# ── paths ────────────────────────────────────────────────────────────────────

# service_instance_data_dir <prefix> <slug> — the per-instance data dir
# `<prefix>/<slug>` (registry + embedded PG + fastembed + logs + `port` marker).
# Keyed by slug so two gateways never share a data dir, and (since `gateway` is a
# reserved slug) it is never `~/.lucidos/gateway` (the dev gateway's dir).
service_instance_data_dir() { printf '%s/%s' "${1%/}" "$2"; }

# service_instance_port_file <data> — the file recording an instance's current
# port. Its presence marks a registered instance (so listing can find it) and a
# bare re-run reuses the port it holds.
service_instance_port_file() { printf '%s/port' "${1%/}"; }

# service_launchd_plist_path <home> <slug> — ~/Library/LaunchAgents/<label>.plist.
service_launchd_plist_path() {
    printf '%s/Library/LaunchAgents/%s.plist' "${1%/}" "$(service_launchd_label "$2")"
}

# service_systemd_unit_path <home> <slug> [xdg-config-home] — the user unit path,
# honoring $XDG_CONFIG_HOME when set, else <home>/.config.
service_systemd_unit_path() {
    local home="${1%/}" slug="$2" xdg="${3:-}" base
    if [ -n "$xdg" ]; then base="${xdg%/}"; else base="$home/.config"; fi
    printf '%s/systemd/user/%s' "$base" "$(service_systemd_unit_name "$slug")"
}

# service_log_dir <data> — where the service writes stdout/stderr (launchd):
# inside the instance data dir, so everything for an instance lives under
# `<prefix>/<slug>/`.
service_log_dir() { printf '%s/logs' "${1%/}"; }

# service_runtime_root <prefix> — the SHARED, upgrade-stable runtime path every
# instance points at: the `current` symlink install.sh maintains over the active
# runtime. Downloaded once; not per-instance.
service_runtime_root() { printf '%s/runtime/current' "${1%/}"; }

# service_health_url <port> [scheme] — the gateway health endpoint (behind the
# sigil namespace, ADR 0014 — a bare /api/v1/health resolves as a workspace
# slug). Scheme defaults to http (the packaged posture); a TLS-enabled instance
# (LUCIDOS_TLS_CERT/KEY baked into its service env — the gateway serves https
# iff both are set) probes https.
service_health_url() { printf '%s://localhost:%s/~/api/v1/health' "${2:-http}" "$1"; }

# ── port candidates (pure) ───────────────────────────────────────────────────

# service_port_candidates <base> [count] — print <count> ascending ports from
# <base> (default 64), one per line. The effectful auto-pick walks these.
service_port_candidates() {
    local base="$1" count="${2:-64}" i=0
    while [ "$i" -lt "$count" ]; do
        printf '%s\n' "$((base + i))"
        i=$((i + 1))
    done
}

# ── network.toml (machine-global bind config, ADR 0014) ─────────────────────

# service_is_bind_value <s> — true iff <s> is a valid --bind value: `all`,
# `loopback`, or a literal IP (IPv4 dotted quad / IPv6). Mirrors the gateway's
# parse_bind_value contract (crates/lucidos-gateway/src/net_config.rs), which
# falls back to loopback on anything unparseable — the installer refuses those
# up front instead of registering a silently-loopback service the user
# believes is exposed.
service_is_bind_value() {
    local LC_ALL=C s="$1"
    case "$s" in
        all|loopback) return 0 ;;
        '') return 1 ;;
        *:*)
            # IPv6-ish (hex, colons, dots for v4-mapped forms).
            case "$s" in *[!0-9a-fA-F:.]*) return 1 ;; *) return 0 ;; esac
            ;;
        *)
            # IPv4 dotted quad, each octet 0..255.
            local IFS=. o n=0
            for o in $s; do
                case "$o" in ''|*[!0-9]*) return 1 ;; esac
                [ "$o" -le 255 ] || return 1
                n=$((n + 1))
            done
            [ "$n" -eq 4 ]
            ;;
    esac
}

# service_network_toml_path <home> — the machine-global file BOTH the gateway
# and every directly-launched engine read at boot
# (crates/lucidos-gateway/src/net_config.rs::network_toml_path). Deliberately
# keyed on $HOME, NOT the install prefix — it is machine-global by design, and
# it is the SAME knob the picker's Settings → Network access edits, so a later
# UI change keeps working (unlike baking bind env into the unit, which would
# permanently shadow the file: env > file in the resolution order).
service_network_toml_path() { printf '%s/.lucidos/network.toml' "${1%/}"; }

# service_network_toml_bind <contents> — extract the current [gateway] bind
# value ("" when absent). Tolerant single-line parse, used for the
# changed-value warning.
service_network_toml_bind() {
    printf '%s\n' "$1" | sed -n 's/^[[:space:]]*bind[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -1
}

# service_network_toml_engine_inherit <contents> — extract the [engine] inherit
# flag: false only on an explicit `inherit = false` line, else true (matching
# the gateway's default-on-absent/malformed behavior).
service_network_toml_engine_inherit() {
    if printf '%s\n' "$1" | grep -qE '^[[:space:]]*inherit[[:space:]]*=[[:space:]]*false'; then
        printf 'false'
    else
        printf 'true'
    fi
}

# service_render_network_toml <bind> <inherit> — the full network.toml content.
# Byte-mirror of the gateway's net_config::render_network_toml: the picker's
# writer regenerates this whole file from the same template, so keeping the two
# writers identical avoids churn when ownership alternates between them.
service_render_network_toml() {
    local bind="$1" inherit="$2"
    cat <<EOF
# Lucidos machine-level network bind configuration.
# Read at process start by the gateway AND every directly-launched
# engine. Environment variables override this file. Changes take effect
# only after a gateway / engine restart (a live socket cannot re-bind).
#
# [gateway].bind   "loopback" (default, most secure) | "all" (LAN +
#                  tailnet) | a literal IP to bind exactly, e.g. your
#                  Tailscale 100.x address.
# [engine].inherit  true  = every workspace engine binds the gateway's
#                          bind; false = each engine reads its own
#                          per-workspace bind (Settings -> Access).

[gateway]
bind = "$bind"

[engine]
inherit = $inherit
EOF
}

# ── env contract (single source of truth, shared with the foreground launcher) ─

# service_runtime_env_pairs <runtime-root> <data> <port> — the canonical
# NAME=VALUE env the gateway runs with, identical to
# crates/lucidos-app/src/desktop.rs::spawn_gateway but resolved against the SHARED
# <runtime-root> + the per-instance <data> + <port>. One per line.
service_runtime_env_pairs() {
    local root="${1%/}" data="${2%/}" port="$3"
    printf 'LUCIDOS_API_PORT=%s\n' "$port"
    printf 'LUCIDOS_GATEWAY_DATA=%s\n' "$data"
    printf 'LUCIDOS_GATEWAY_PG_BACKEND=embedded\n'
    printf 'LUCIDOS_PG_BIN_DIR=%s/postgres/bin\n' "$root"
    printf 'LUCIDOS_PG_LIB_DIR=%s/postgres/lib\n' "$root"
    printf 'LUCIDOS_ENGINE_BIN=%s/lucidos-engine\n' "$root"
    printf 'LUCIDOS_CLI_BIN=%s/lucidos\n' "$root"
    printf 'LUCIDOS_STATIC_DIR=%s/frontend\n' "$root"
    printf 'LUCIDOS_SDK_DIR=%s/sdk\n' "$root"
    printf 'LUCIDOS_SYSTEM_KNOWHOW_DIR=%s/system-knowhow\n' "$root"
    printf 'FASTEMBED_CACHE_DIR=%s/fastembed\n' "$data"
    printf 'LUCIDOS_BOOT_WITHOUT_PROVIDER=1\n'
    printf 'LUCIDOS_PACKAGED=1\n'
}

# service_tls_env_pairs <cert-path> <key-path> — the OPT-IN TLS env pairs
# (install.sh --tls-cert/--tls-key). The gateway serves TLS iff BOTH
# LUCIDOS_TLS_CERT and LUCIDOS_TLS_KEY point at non-empty files (ADR 0014); it
# already strips them from spawned engines (gateway terminates TLS). https is
# what gives a NON-localhost device a secure context — service worker, PWA
# install, and web push all require one, so a remote phone/laptop needs this
# (plus the Settings → Access → Network access bind) to get notifications.
service_tls_env_pairs() {
    printf 'LUCIDOS_TLS_CERT=%s\n' "$1"
    printf 'LUCIDOS_TLS_KEY=%s\n' "$2"
}

# ── manager detection + compose decision (pure) ──────────────────────────────

# service_decide_manager <uname-s> <has-launchctl 0|1> <has-systemd-user 0|1> —
# the PURE decision the effectful service_detect_manager wraps.
service_decide_manager() {
    local os="$1" has_launchctl="$2" has_systemd_user="$3"
    case "$os" in
        Darwin) [ "$has_launchctl" = "1" ] && { printf 'launchd'; return 0; } ;;
        Linux)  [ "$has_systemd_user" = "1" ] && { printf 'systemd-user'; return 0; } ;;
    esac
    printf 'none'
}

# service_compose_decision <no-service> <manager> — service vs foreground. A
# non-empty <no-service> (--no-service) or a `none` manager (graceful degrade)
# both choose foreground; otherwise register the service.
service_compose_decision() {
    local no_service="$1" manager="$2"
    if [ -n "$no_service" ]; then printf 'foreground'; return 0; fi
    if [ "$manager" = "none" ]; then printf 'foreground'; return 0; fi
    printf 'service'
}

# ── launchctl command-arg builders (pure) ────────────────────────────────────

# service_launchd_domain <uid> — gui/<uid> (the per-user launchd domain).
service_launchd_domain() { printf 'gui/%s' "$1"; }

# service_launchd_target <uid> <label> — gui/<uid>/<label> (the job target).
service_launchd_target() { printf 'gui/%s/%s' "$1" "$2"; }

# service_launchd_timeout: seconds to wait for a booted-out job to actually leave
# the domain (see service_launchd_wait_gone). LUCIDOS_LAUNCHD_TIMEOUT overrides it.
#
# 30s is headroom, not the expected wait. launchd escalates SIGTERM to SIGKILL
# only after the job's ExitTimeOut, whose documented default is 20s, and the
# gateway always takes that path because it ignores SIGTERM. Measured teardown on
# macOS 26 is ~5s, so the normal case returns long before this; the ceiling is
# sized so a slow or loaded machine cannot be misreported as a stuck job.
service_launchd_timeout() { printf '%s' "${LUCIDOS_LAUNCHD_TIMEOUT:-30}"; }

# ── plist / unit templating (pure) ───────────────────────────────────────────

# service_xml_escape <string> — minimal XML escaping for plist values.
service_xml_escape() {
    local s="$1"
    s="${s//&/&amp;}"
    s="${s//</&lt;}"
    s="${s//>/&gt;}"
    printf '%s' "$s"
}

# service_systemd_escape_env <NAME=VALUE line> — escape one env line for a
# quoted systemd `Environment="…"` assignment so the value round-trips
# byte-identical through systemd's unit parser: `%` doubles to `%%` (else it is
# expanded as a unit specifier — an API key containing `%` would reach the
# gateway silently mangled), and `\`/`"` get C-style backslash escapes (else
# they terminate/corrupt the quoted string). The launchd twin is
# service_xml_escape. Env NAMES are plain identifiers, so escaping the whole
# line uniformly is safe.
service_systemd_escape_env() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//%/%%}"
    printf '%s' "$s"
}

# service_launchd_plist <label> <gateway-bin> <workdir> <out-log> <err-log> <env-block>
# Render the LaunchAgent plist: RunAtLoad + KeepAlive (always-on), ProgramArguments
# = ONLY the gateway binary (the service supervises the gateway, which spawns the
# engines), EnvironmentVariables = the env-block (newline NAME=VALUE pairs), and
# stdout/stderr log paths. Pure string output.
service_launchd_plist() {
    local label="$1" gateway_bin="$2" workdir="$3" out_log="$4" err_log="$5" env_block="$6"
    local env_xml
    env_xml="$(
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            printf '        <key>%s</key>\n        <string>%s</string>\n' \
                "${line%%=*}" "$(service_xml_escape "${line#*=}")"
        done <<EOF
$env_block
EOF
    )"
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$label</string>
    <key>ProgramArguments</key>
    <array>
        <string>$(service_xml_escape "$gateway_bin")</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
$env_xml
    </dict>
    <key>WorkingDirectory</key>
    <string>$(service_xml_escape "$workdir")</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>StandardOutPath</key>
    <string>$(service_xml_escape "$out_log")</string>
    <key>StandardErrorPath</key>
    <string>$(service_xml_escape "$err_log")</string>
</dict>
</plist>
EOF
}

# service_systemd_unit <gateway-bin> <workdir> <env-block> — render the systemd
# --user unit. ExecStart = ONLY the gateway binary; Environment= from the env
# block; Restart=always (always-on, KeepAlive parity); KillSignal=SIGUSR1 +
# KillMode=process so `stop`/`restart` stop the gateway gracefully and LEAVE the
# engines + embedded Postgres for the relaunched gateway to re-adopt (engine
# statelessness). WantedBy=default.target so `enable` starts it at login.
service_systemd_unit() {
    local gateway_bin="$1" workdir="$2" env_block="$3" env_lines
    env_lines="$(
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            printf 'Environment="%s"\n' "$(service_systemd_escape_env "$line")"
        done <<EOF
$env_block
EOF
    )"
    # ExecStart is a quoted word (survives spaces) with the same escapes as the
    # env lines; WorkingDirectory is a bare to-end-of-line value, so only `%`
    # (specifier expansion) needs doubling there.
    local exec_quoted workdir_esc
    exec_quoted="\"$(service_systemd_escape_env "$gateway_bin")\""
    workdir_esc="${workdir//%/%%}"
    cat <<EOF
[Unit]
Description=Lucidos workspace gateway (always-on)
After=network.target

[Service]
Type=simple
ExecStart=$exec_quoted
WorkingDirectory=$workdir_esc
$env_lines
Restart=always
RestartSec=2
KillSignal=SIGUSR1
KillMode=process
TimeoutStopSec=20

[Install]
WantedBy=default.target
EOF
}

# ── uninstall (pure) ─────────────────────────────────────────────────────────

# service_uninstall_purge_targets <data> — the path an uninstall --purge deletes
# for ONE instance: its whole data dir (registry + embedded PG + fastembed +
# logs). The SHARED runtime is deleted only by `uninstall --all --purge`, handled
# separately. Without --purge the uninstall deletes nothing (prints "left behind").
service_uninstall_purge_targets() {
    printf '%s\n' "${1%/}"
}

# ════════════════════════════════════════════════════════════════════════════
# EFFECTFUL wrappers — the launchctl/systemctl/curl/kill/pg_ctl side effects +
# port probing + instance listing. The unit tests NEVER call anything below this
# line (they run offline).
# ════════════════════════════════════════════════════════════════════════════

# service_detect_manager — probe the host → launchd | systemd-user | none (via
# the pure service_decide_manager). systemd --user needs a reachable user
# systemd/D-Bus session; `systemctl --user show-environment` is the cheap probe.
service_detect_manager() {
    local os has_launchctl=0 has_systemd_user=0
    os="$(uname -s)"
    command -v launchctl >/dev/null 2>&1 && has_launchctl=1
    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
        has_systemd_user=1
    fi
    service_decide_manager "$os" "$has_launchctl" "$has_systemd_user"
}

# service_port_in_use <port> — true if something is LISTENing on the port. Prefers
# lsof (authoritative for LISTEN on macOS + Linux); falls back to a bash /dev/tcp
# loopback connect (a successful connect ⇒ in use). Best-effort — on neither, it
# reports free.
service_port_in_use() {
    local port="$1"
    if command -v lsof >/dev/null 2>&1; then
        lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
        return $?
    fi
    ( exec 3<>"/dev/tcp/127.0.0.1/$port" ) >/dev/null 2>&1
}

# service_list_instance_names <prefix> — print the slug of every registered
# instance (the subdirs of <prefix> that carry a `port` marker; `runtime` and
# other dirs are skipped), one per line.
service_list_instance_names() {
    local prefix="${1%/}" entry base
    [ -d "$prefix" ] || return 0
    for entry in "$prefix"/*/; do
        [ -d "$entry" ] || continue
        [ -f "${entry}port" ] || continue
        base="$(basename "$entry")"
        printf '%s\n' "$base"
    done
}

# service_write_file <path> <content> [mode] — create the parent dir, write the
# content, and chmod. Mode 600 is used when the content carries provider secrets.
service_write_file() {
    local path="$1" content="$2" mode="${3:-644}" dir
    dir="$(dirname "$path")"
    mkdir -p "$dir" || return 1
    printf '%s\n' "$content" > "$path" || return 1
    chmod "$mode" "$path" 2>/dev/null || true
    return 0
}

# service_launchd_is_loaded <uid> <label> — true if the job is bootstrapped.
service_launchd_is_loaded() {
    launchctl print "$(service_launchd_target "$1" "$2")" >/dev/null 2>&1
}

# ── the async-bootout rule, which the two wrappers below both depend on ──────
#
# `launchctl bootout` IS ASYNCHRONOUS, AND ITS EXIT CODE IS NOT AN ANSWER. It
# returns 0 the moment launchd ACCEPTS the request, not when the job is gone.
# launchd then SIGTERMs the job and, because the gateway deliberately IGNORES
# SIGTERM (it stops on SIGUSR1: crates/lucidos-gateway/src/server.rs
# install_shutdown, and the KillSignal note in this file's header), has to wait
# out ExitTimeOut and SIGKILL it. For that entire window `launchctl print
# gui/<uid>/<label>` keeps succeeding. Measured on macOS 26: bootout returns 0
# and the job stays visible for ~5s.
#
# So NEITHER wrapper may act on launchctl's exit code. Both decide by OBSERVING
# the domain, via service_launchd_wait_gone. Do not "simplify" that away: each
# call site had a real, shipped bug caused by trusting the exit code, described
# where it happened.
#
# The `launchctl load` family is a second trap in the same area: it prints
# "Load failed: 5: Input/output error" and exits 0, so its status is no evidence
# either. Only the observed state is.
#
# All of this is Darwin-only by construction (nothing else has launchctl), which
# is why the fractional `sleep` below is safe. systemd needs no equivalent:
# `systemctl --user disable --now` blocks until the unit has actually stopped.

# service_launchd_wait_gone <uid> <label> [timeout-secs]: block until the job has
# genuinely left the domain. 0 = gone, non-zero = still there at the deadline.
service_launchd_wait_gone() {
    local uid="$1" label="$2" timeout="${3:-$(service_launchd_timeout)}" deadline
    deadline=$(( $(date +%s) + timeout ))
    while service_launchd_is_loaded "$uid" "$label"; do
        [ "$(date +%s)" -ge "$deadline" ] && return 1
        sleep 0.2
    done
    return 0
}

# service_launchd_load <uid> <plist> <label> — (re)load + (re)start the agent,
# idempotently: fully unload an existing job first so a rewritten plist takes
# effect, then bootstrap (legacy `load -w` fallback) and kickstart. Returns
# non-zero if the job is not loaded afterwards, or if an old job would not go.
#
# The unload has to COMPLETE before the bootstrap. Bootstrapping into a domain
# that still holds the dying job is refused with "Bootstrap failed: 5: Input/
# output error"; `load -w` then also fails while exiting 0, `kickstart` returns
# 37, and is_loaded still sees the OLD job. This function therefore used to
# report success on a re-run over a running instance and, seconds later, leave no
# job in the domain at all: an upgrade or a --port change silently unregistered
# the service until the next login. Reproduced deterministically on macOS 26.
service_launchd_load() {
    local uid="$1" plist="$2" label="$3" domain target
    domain="$(service_launchd_domain "$uid")"
    target="$(service_launchd_target "$uid" "$label")"
    service_launchd_unload "$uid" "$label" || return 1
    launchctl bootstrap "$domain" "$plist" >/dev/null 2>&1 \
        || launchctl load -w "$plist" >/dev/null 2>&1 || true
    launchctl kickstart -k "$target" >/dev/null 2>&1 || true
    service_launchd_is_loaded "$uid" "$label"
}

# service_launchd_unload <uid> <label> [timeout-secs]: bootout the agent (legacy
# `remove` fallback) and WAIT until it has actually left the domain. A not-loaded
# job is success. Returns non-zero, with a diagnosis in SERVICE_LAUNCHD_ERR, if
# the job is STILL bootstrapped at the deadline.
#
# Returning 0 straight after the bootout is what made `uninstall.sh` report
# "Stopped launchd agent" over a job that was still loaded, which install-smoke's
# front-door rung 7 catches. It also hid the case that actually hurts: when a
# bootout is REFUSED, the agent carries KeepAlive, so the user who just
# uninstalled keeps a gateway respawning until they log out. Both readings looked
# identical from the exit code; only the wait can tell them apart.
service_launchd_unload() {
    local uid="$1" label="$2" timeout="${3:-$(service_launchd_timeout)}" target out=""
    target="$(service_launchd_target "$uid" "$label")"
    SERVICE_LAUNCHD_ERR=""
    service_launchd_is_loaded "$uid" "$label" || return 0
    # Keep launchctl's stderr. It is the only thing that distinguishes "refused"
    # from "accepted but slow", and discarding it is what made this invisible.
    out="$(launchctl bootout "$target" 2>&1)" \
        || out="$out${out:+; }launchctl remove: $(launchctl remove "$label" 2>&1)"
    service_launchd_wait_gone "$uid" "$label" "$timeout" && return 0
    # shellcheck disable=SC2034 # out-param: read by uninstall.sh remove_instance + install.sh register_launchd
    SERVICE_LAUNCHD_ERR="still bootstrapped at $target after ${timeout}s${out:+ (launchctl said: $out)}"
    return 1
}

# service_systemd_is_active <unit> — true if the unit is running.
service_systemd_is_active() {
    systemctl --user is-active --quiet "$1"
}

# service_systemd_load <unit-path> <unit-name> — reload, enable (start at login),
# and restart (start-if-stopped, reload-if-running → idempotent). Returns non-zero
# if the (re)start fails.
service_systemd_load() {
    local unit_name="$2"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
    systemctl --user enable "$unit_name" >/dev/null 2>&1 || true
    systemctl --user restart "$unit_name" >/dev/null 2>&1 || return 1
    return 0
}

# service_systemd_unload <unit-name> — disable + stop the unit, then reload.
service_systemd_unload() {
    systemctl --user disable --now "$1" >/dev/null 2>&1 || true
    systemctl --user daemon-reload >/dev/null 2>&1 || true
    return 0
}

# service_health_wait <port> <timeout-secs> [interval-secs] [scheme] — poll the
# gateway health URL until it answers 200 or the deadline passes. Needs curl.
# `-k` accepts the self-signed/private-CA certs a TLS-flagged install typically
# carries (same posture as the dev scripts' `curl -sk`); it is a no-op on http.
service_health_wait() {
    local port="$1" timeout="${2:-120}" interval="${3:-1}" scheme="${4:-http}" url deadline now
    url="$(service_health_url "$port" "$scheme")"
    deadline=$(( $(date +%s) + timeout ))
    while :; do
        if curl -fsSk -o /dev/null --max-time 3 "$url" 2>/dev/null; then
            return 0
        fi
        now="$(date +%s)"
        [ "$now" -ge "$deadline" ] && return 1
        sleep "$interval"
    done
}

# service_write_network_toml <home> <bind> — regenerate the machine-global
# network.toml with <bind>, preserving the existing [engine] inherit flag.
# Whole-file regenerate + atomic tmp/rename, exactly like the gateway's own
# writer (net_config::write_network_toml).
service_write_network_toml() {
    local home="$1" bind="$2" path existing="" inherit
    path="$(service_network_toml_path "$home")"
    [ -f "$path" ] && existing="$(cat "$path" 2>/dev/null || true)"
    inherit="$(service_network_toml_engine_inherit "$existing")"
    mkdir -p "$(dirname "$path")" || return 1
    service_render_network_toml "$bind" "$inherit" > "$path.tmp" || return 1
    mv "$path.tmp" "$path"
}

# service_stop_embedded_runtime <data> <runtime-root> — best-effort graceful stop
# of what the gateway spawned, so an uninstall leaves no orphans (mirrors
# crates/lucidos-app/src/desktop.rs shutdown order): SIGUSR1 each workspace engine
# (engines ignore SIGTERM), then `pg_ctl -m fast stop` the embedded cluster at
# <data>/pgdata using the bundled binaries + libpath. No-op for anything missing.
# Call this AFTER the gateway is unregistered, so it can't respawn the engines.
service_stop_embedded_runtime() {
    local data="${1%/}" root="${2%/}"
    local workspaces="$data/workspaces" pgdata="$data/pgdata"
    local pidfile pid pg_bin pg_lib

    if [ -d "$workspaces" ]; then
        for pidfile in "$workspaces"/*/.lucidos/engine.pid; do
            [ -f "$pidfile" ] || continue
            pid="$(tr -d '[:space:]' < "$pidfile" 2>/dev/null || true)"
            if [ -n "$pid" ]; then
                kill -USR1 "$pid" >/dev/null 2>&1 || true
            fi
            rm -f "$pidfile" 2>/dev/null || true
        done
    fi

    pg_bin="$root/postgres/bin"
    pg_lib="$root/postgres/lib"
    if [ -f "$pgdata/PG_VERSION" ] && [ -x "$pg_bin/pg_ctl" ]; then
        DYLD_LIBRARY_PATH="$pg_lib" LD_LIBRARY_PATH="$pg_lib" \
            "$pg_bin/pg_ctl" -D "$pgdata" -m fast -w stop >/dev/null 2>&1 || true
    fi
    return 0
}
