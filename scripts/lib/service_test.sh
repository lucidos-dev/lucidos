#!/usr/bin/env bash
# Tests for scripts/lib/service.sh + install.sh/uninstall.sh service wiring (step 4
# of docs/plans/2026-06-30-installer-step4-service-mode.md). SLUG-KEYED instances.
# Two halves, same offline style as install_test.sh / stage_runtime_test.sh:
#   • PURE helpers — sourced + asserted directly (identity, validation, paths, env
#     pairs, plist/unit templating, manager DECISION, compose decision, command-arg
#     builders, port candidates, uninstall paths) + an offline FS test of
#     service_list_instance_names. No launchctl/systemd/network.
#   • INTEGRATION — install.sh / uninstall.sh invoked as subprocesses that NEVER
#     touch real launchd/systemd: foreground paths (--no-service, faked no-manager
#     degrade), a register-wiring test with a FAKE launchctl + a 1s health timeout,
#     and uninstall (--list / --name / --all / --purge) with fake managers. All
#     network-free, and all data pinned into temp dirs (the suite runs inside a live
#     workspace whose ambient LUCIDOS_GATEWAY_DATA points at the REAL gateway dir).
# Run: ./scripts/lib/service_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
INSTALL="$PROJECT_DIR/install.sh"
UNINSTALL="$PROJECT_DIR/uninstall.sh"

# Sandbox every remote-access env-as-flag input: install.sh reads LUCIDOS_BIND
# as the --bind default, so an ambient value would flip the no-bind tests. The
# TLS twins (LUCIDOS_TLS_CERT/KEY) are pinned to EMPTY-exported just below.
unset LUCIDOS_BIND

# shellcheck source=scripts/lib/service.sh
source "$SCRIPT_DIR/service.sh"
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"
# shellcheck source=scripts/lib/headless_tarball.sh
source "$SCRIPT_DIR/headless_tarball.sh"
# shellcheck source=scripts/lib/stage_runtime.sh
source "$SCRIPT_DIR/stage_runtime.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }
has()  { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }

# Pin the ambient TLS env to empty for every install.sh subprocess below: the
# suite runs inside a live dev workspace whose engine carries LUCIDOS_TLS_CERT/
# LUCIDOS_TLS_KEY (dev serves https), and install.sh reads those env vars as the
# --tls-cert/--tls-key defaults — ambient leakage would silently flip the no-TLS
# tests to the TLS path. Explicit flags in the TLS tests override these.
export LUCIDOS_TLS_CERT='' LUCIDOS_TLS_KEY=''

VERSION="0.14.0"
TRIPLE="$(stage_runtime_host_triple)"
STEM="lucidos-$VERSION-$TRIPLE"
NAMES=(lucidos-engine lucidos-gateway lucidos frontend postgres sdk)

# A fake runtime tree; the "gateway" just prints + exits 0 so a FOREGROUND
# `exec gateway` returns cleanly (no real server, no hang).
new_resources() {
    local dir; dir="$(mktemp -d)"
    printf 'engine\n'  > "$dir/lucidos-engine"
    printf '#!/bin/sh\necho gateway\n' > "$dir/lucidos-gateway"
    printf 'cli\n'     > "$dir/lucidos"
    chmod +x "$dir/lucidos-engine" "$dir/lucidos-gateway" "$dir/lucidos"
    mkdir -p "$dir/frontend" && printf '<html>\n' > "$dir/frontend/index.html"
    mkdir -p "$dir/sdk"      && printf 'sdk\n'     > "$dir/sdk/sdk.js"
    mkdir -p "$dir/postgres/bin" "$dir/postgres/lib"
    printf 'postgres\n' > "$dir/postgres/bin/postgres"; chmod +x "$dir/postgres/bin/postgres"
    printf 'libpq\n'    > "$dir/postgres/lib/libpq.5"
    printf '%s' "$dir"
}
new_release_dir() {
    local res out
    res="$(new_resources)"; out="$(mktemp -d)"
    headless_tarball_emit "$res" "$out" "$VERSION" "$TRIPLE" "${NAMES[@]}" >/dev/null \
        || { echo "ERROR: could not build fake tarball" >&2; return 1; }
    rm -rf "$res"
    printf '%s' "$out"
}

# make_fakebin [uname-os] [launchctl-print-rc] — a PATH dir with fake launchctl
# (print → given rc; everything else succeeds), fake systemctl (always fails, so
# the systemd --user branch is skipped on any host), and (optional) a uname shim.
make_fakebin() {
    local dir os rc
    dir="$(mktemp -d)"; os="${1:-}"; rc="${2:-1}"
    if [ -n "$os" ]; then
        # shellcheck disable=SC2016 # $1 belongs to the GENERATED stub, so it must not expand here
        { printf '#!/bin/sh\n'
          printf '[ "$1" = "-s" ] && { echo %s; exit 0; }\n' "$os"
          printf 'echo %s\n' "$os"; } > "$dir/uname"
        chmod +x "$dir/uname"
    fi
    # shellcheck disable=SC2016 # $1 belongs to the GENERATED stub, so it must not expand here
    printf '#!/bin/sh\ncase "$1" in print) exit %s;; *) exit 0;; esac\n' "$rc" > "$dir/launchctl"
    printf '#!/bin/sh\nexit 1\n' > "$dir/systemctl"
    chmod +x "$dir/launchctl" "$dir/systemctl"
    printf '%s' "$dir"
}

# ── PURE: identity (slug-suffixed) ────────────────────────────────────────────
echo "test: service identity is slug-suffixed (coexists; never collides with the .app's com.lucidos.engine)"
if [ "$(service_launchd_label default)" = "com.lucidos.gateway.default" ]; then pass "launchd label (default)"; else fail "label: $(service_launchd_label default)"; fi
if [ "$(service_launchd_label test)" = "com.lucidos.gateway.test" ]; then pass "launchd label (test)"; else fail "label: $(service_launchd_label test)"; fi
if [ "$(service_systemd_unit_name test)" = "lucidos-gateway-test.service" ]; then pass "systemd unit name"; else fail "unit: $(service_systemd_unit_name test)"; fi

# ── PURE: validation ──────────────────────────────────────────────────────────
echo ""
echo "test: service_is_instance_name accepts slugs, rejects junk + reserved names"
for ok_slug in default test ab-cd x1; do
    if service_is_instance_name "$ok_slug"; then pass "accepts '$ok_slug'"; else fail "rejected valid '$ok_slug'"; fi
done
for bad in "" "UP" "a b" "-x" "x-" "a_b" gateway runtime current logs; do
    if service_is_instance_name "$bad"; then fail "accepted invalid '$bad'"; else pass "rejects '$bad'"; fi
done
echo ""
echo "test: service_is_port_number bounds"
for ok_p in 1 5252 65535; do if service_is_port_number "$ok_p"; then pass "accepts $ok_p"; else fail "rejected $ok_p"; fi; done
for bad_p in 0 65536 "" abc 52x; do if service_is_port_number "$bad_p"; then fail "accepted $bad_p"; else pass "rejects '$bad_p'"; fi; done

# ── PURE: paths ───────────────────────────────────────────────────────────────
echo ""
echo "test: path resolution (data dir, port file, plist, unit, logs, runtime root, health URL)"
if [ "$(service_instance_data_dir /opt/x test)" = "/opt/x/test" ]; then pass "data dir = <prefix>/<slug>"; else fail "data dir: $(service_instance_data_dir /opt/x test)"; fi
if [ "$(service_instance_port_file /opt/x/test)" = "/opt/x/test/port" ]; then pass "port file = <data>/port"; else fail "port file: $(service_instance_port_file /opt/x/test)"; fi
got="$(service_launchd_plist_path /Users/me test)"
if [ "$got" = "/Users/me/Library/LaunchAgents/com.lucidos.gateway.test.plist" ]; then pass "plist path = $got"; else fail "plist path: $got"; fi
got="$(service_systemd_unit_path /home/u test)"
if [ "$got" = "/home/u/.config/systemd/user/lucidos-gateway-test.service" ]; then pass "unit path (default XDG) = $got"; else fail "unit path: $got"; fi
got="$(service_systemd_unit_path /home/u test /home/u/.xdg)"
if [ "$got" = "/home/u/.xdg/systemd/user/lucidos-gateway-test.service" ]; then pass "unit path honors XDG_CONFIG_HOME"; else fail "unit path XDG: $got"; fi
if [ "$(service_log_dir /opt/x/test)" = "/opt/x/test/logs" ]; then pass "log dir = <data>/logs"; else fail "log dir: $(service_log_dir /opt/x/test)"; fi
if [ "$(service_runtime_root /opt/x)" = "/opt/x/runtime/current" ]; then pass "runtime root = shared current symlink"; else fail "runtime root: $(service_runtime_root /opt/x)"; fi
if [ "$(service_health_url 5252)" = "http://localhost:5252/~/api/v1/health" ]; then pass "health url (sigil namespace)"; else fail "health url: $(service_health_url 5252)"; fi
if [ "$(service_health_url 5252 https)" = "https://localhost:5252/~/api/v1/health" ]; then pass "health url honors the https scheme (TLS instance)"; else fail "health url https: $(service_health_url 5252 https)"; fi

# ── PURE: bind-value validation + network.toml render/parse round-trip ────────
echo ""
echo "test: service_is_bind_value accepts all/loopback/IPs, rejects junk"
for ok_b in all loopback 0.0.0.0 192.168.1.7 100.64.0.1 :: ::1 fd7a:115c:a1e0::1; do
    if service_is_bind_value "$ok_b"; then pass "accepts '$ok_b'"; else fail "rejected valid '$ok_b'"; fi
done
for bad_b in "" lan yes 256.1.1.1 1.2.3 "192.168.1.7 evil" "all;rm"; do
    if service_is_bind_value "$bad_b"; then fail "accepted invalid '$bad_b'"; else pass "rejects '$bad_b'"; fi
done

echo ""
echo "test: network.toml render mirrors the gateway writer; bind/inherit parse back out"
NT="$(service_render_network_toml all true)"
if has "$NT" '[gateway]' && has "$NT" 'bind = "all"'; then pass "render carries [gateway] bind"; else fail "render: $NT"; fi
if has "$NT" '[engine]' && has "$NT" 'inherit = true'; then pass "render carries [engine] inherit"; else fail "render inherit: $NT"; fi
if [ "$(service_network_toml_bind "$NT")" = "all" ]; then pass "bind parses back out"; else fail "bind parse: $(service_network_toml_bind "$NT")"; fi
if [ "$(service_network_toml_engine_inherit "$NT")" = "true" ]; then pass "inherit parses back out"; else fail "inherit parse"; fi
NT2="$(service_render_network_toml 100.64.0.1 false)"
if [ "$(service_network_toml_bind "$NT2")" = "100.64.0.1" ]; then pass "IP bind round-trips"; else fail "IP bind: $(service_network_toml_bind "$NT2")"; fi
if [ "$(service_network_toml_engine_inherit "$NT2")" = "false" ]; then pass "inherit=false round-trips"; else fail "inherit=false parse"; fi
if [ "$(service_network_toml_engine_inherit "")" = "true" ]; then pass "absent file defaults inherit=true"; else fail "empty-contents inherit default"; fi

# ── OFFLINE FS: service_write_network_toml writes + preserves inherit ─────────
echo ""
echo "test: service_write_network_toml writes fresh + preserves an existing inherit=false"
NTH="$(mktemp -d)"
service_write_network_toml "$NTH" all || fail "fresh network.toml write failed"
if [ "$(service_network_toml_bind "$(cat "$NTH/.lucidos/network.toml")")" = "all" ]; then pass "fresh write records bind=all"; else fail "fresh write: $(cat "$NTH/.lucidos/network.toml" 2>/dev/null)"; fi
service_render_network_toml loopback false > "$NTH/.lucidos/network.toml"
service_write_network_toml "$NTH" 192.168.1.7 || fail "rewrite failed"
NTC="$(cat "$NTH/.lucidos/network.toml")"
if [ "$(service_network_toml_bind "$NTC")" = "192.168.1.7" ]; then pass "rewrite updates bind"; else fail "rewrite bind: $NTC"; fi
if [ "$(service_network_toml_engine_inherit "$NTC")" = "false" ]; then pass "rewrite preserves inherit=false"; else fail "rewrite lost inherit: $NTC"; fi
rm -rf "$NTH"

# ── PURE: env contract (matches spawn_gateway; shared runtime + per-instance data) ─
echo ""
echo "test: service_runtime_env_pairs matches the spawn_gateway env contract"
env_block="$(service_runtime_env_pairs /opt/x/runtime/current /opt/x/test 5252)"
check_env() { if has "$env_block" "$1"; then pass "env has $1"; else fail "env missing $1: $env_block"; fi; }
check_env "LUCIDOS_API_PORT=5252"
check_env "LUCIDOS_GATEWAY_DATA=/opt/x/test"
check_env "LUCIDOS_GATEWAY_PG_BACKEND=embedded"
check_env "LUCIDOS_PG_BIN_DIR=/opt/x/runtime/current/postgres/bin"
check_env "LUCIDOS_PG_LIB_DIR=/opt/x/runtime/current/postgres/lib"
check_env "LUCIDOS_ENGINE_BIN=/opt/x/runtime/current/lucidos-engine"
check_env "LUCIDOS_CLI_BIN=/opt/x/runtime/current/lucidos"
check_env "LUCIDOS_STATIC_DIR=/opt/x/runtime/current/frontend"
check_env "LUCIDOS_SDK_DIR=/opt/x/runtime/current/sdk"
check_env "LUCIDOS_SYSTEM_KNOWHOW_DIR=/opt/x/runtime/current/system-knowhow"
check_env "FASTEMBED_CACHE_DIR=/opt/x/test/fastembed"
check_env "LUCIDOS_BOOT_WITHOUT_PROVIDER=1"
check_env "LUCIDOS_PACKAGED=1"
# TLS is OPT-IN: the base contract must NOT carry TLS vars (packaged posture is
# plain http; install.sh appends service_tls_env_pairs only when both flags are
# supplied).
if has "$env_block" "LUCIDOS_TLS_CERT"; then fail "base env must not carry LUCIDOS_TLS_CERT"; else pass "base env has no TLS vars (opt-in only)"; fi
echo ""
echo "test: service_tls_env_pairs renders the opt-in TLS pairs"
tls_block="$(service_tls_env_pairs /p/certs/host.crt /p/certs/host.key)"
if has "$tls_block" "LUCIDOS_TLS_CERT=/p/certs/host.crt"; then pass "TLS cert pair"; else fail "TLS cert pair: $tls_block"; fi
if has "$tls_block" "LUCIDOS_TLS_KEY=/p/certs/host.key"; then pass "TLS key pair"; else fail "TLS key pair: $tls_block"; fi

# ── PURE: manager DECISION + compose decision ─────────────────────────────────
echo ""
echo "test: service_decide_manager across faked (os, launchctl, systemd-user)"
if [ "$(service_decide_manager Darwin 1 0)" = "launchd" ]; then pass "macOS + launchctl → launchd"; else fail "Darwin 1 0"; fi
if [ "$(service_decide_manager Darwin 0 0)" = "none" ]; then pass "macOS w/o launchctl → none"; else fail "Darwin 0 0"; fi
if [ "$(service_decide_manager Linux 0 1)" = "systemd-user" ]; then pass "Linux + systemd --user → systemd-user"; else fail "Linux 0 1"; fi
if [ "$(service_decide_manager Linux 0 0)" = "none" ]; then pass "Linux w/o systemd --user → none"; else fail "Linux 0 0"; fi
if [ "$(service_decide_manager PlanNine 1 1)" = "none" ]; then pass "unknown OS → none"; else fail "PlanNine 1 1"; fi
echo ""
echo "test: service_compose_decision (default service; --no-service + no-manager degrade)"
if [ "$(service_compose_decision "" launchd)" = "service" ]; then pass "default + launchd → service"; else fail "'' launchd"; fi
if [ "$(service_compose_decision 1 launchd)" = "foreground" ]; then pass "--no-service → foreground"; else fail "1 launchd"; fi
if [ "$(service_compose_decision "" none)" = "foreground" ]; then pass "no manager → foreground (degrade)"; else fail "'' none"; fi

# ── PURE: command-arg builders, xml escape, port candidates ──────────────────
echo ""
echo "test: launchctl domain/target builders + xml escape + port candidates"
if [ "$(service_launchd_domain 501)" = "gui/501" ]; then pass "domain = gui/<uid>"; else fail "domain"; fi
if [ "$(service_launchd_target 501 com.lucidos.gateway.test)" = "gui/501/com.lucidos.gateway.test" ]; then pass "target = gui/<uid>/<label>"; else fail "target"; fi
if [ "$(service_xml_escape '/a&b/<x>')" = "/a&amp;b/&lt;x&gt;" ]; then pass "xml escape"; else fail "xml escape"; fi
cand="$(service_port_candidates 5252 3)"
if [ "$cand" = "$(printf '5252\n5253\n5254')" ]; then pass "port candidates ascend from base"; else fail "candidates: $cand"; fi

# ── PURE: launchd plist content ──────────────────────────────────────────────
echo ""
echo "test: launchd plist embeds the gateway-only ProgramArguments, env, KeepAlive, logs"
PL="$(service_launchd_plist com.lucidos.gateway.test /p/runtime/current/lucidos-gateway /p/test /p/test/logs/gateway.out.log /p/test/logs/gateway.err.log "$env_block")"
if has "$PL" "<string>com.lucidos.gateway.test</string>"; then pass "plist has the slug-suffixed label"; else fail "plist label missing"; fi
if has "$PL" "<string>/p/runtime/current/lucidos-gateway</string>"; then pass "ProgramArguments = the shared gateway binary"; else fail "plist gateway path missing"; fi
n="$(printf '%s\n' "$PL" | awk '/<key>ProgramArguments<\/key>/{f=1;next} f&&/<\/array>/{f=0} f&&/<string>/{c++} END{print c+0}')"
if [ "$n" = "1" ]; then pass "ProgramArguments has exactly one entry (gateway only, not per-engine)"; else fail "ProgramArguments entries: $n"; fi
if has "$PL" "<key>RunAtLoad</key>"; then pass "RunAtLoad present"; else fail "RunAtLoad missing"; fi
if has "$PL" "<key>KeepAlive</key>"; then pass "KeepAlive present"; else fail "KeepAlive missing"; fi
if has "$PL" "<key>LUCIDOS_API_PORT</key>" && has "$PL" "<string>5252</string>"; then pass "env var rendered as key/string"; else fail "env var not rendered"; fi
if has "$PL" "<string>/p/test/logs/gateway.out.log</string>"; then pass "StandardOutPath in the instance log dir"; else fail "out log missing"; fi
if has "$PL" "<string>/p/test</string>"; then pass "WorkingDirectory = the instance data dir"; else fail "workdir missing"; fi

# ── PURE: systemd unit content ───────────────────────────────────────────────
echo ""
echo "test: systemd unit embeds ExecStart (gateway only), Environment, SIGUSR1, WantedBy"
UNIT="$(service_systemd_unit /p/runtime/current/lucidos-gateway /p/test "$env_block")"
if has "$UNIT" 'ExecStart="/p/runtime/current/lucidos-gateway"'; then pass "ExecStart = the shared gateway (quoted word)"; else fail "ExecStart wrong"; fi
n="$(printf '%s\n' "$UNIT" | grep -c '^ExecStart=')"
if [ "$n" = "1" ]; then pass "exactly one ExecStart (gateway only)"; else fail "ExecStart count: $n"; fi
if has "$UNIT" 'Environment="LUCIDOS_API_PORT=5252"'; then pass "Environment= rendered"; else fail "Environment missing"; fi
if has "$UNIT" "WorkingDirectory=/p/test"; then pass "WorkingDirectory = the instance data dir"; else fail "WorkingDirectory missing"; fi
if has "$UNIT" "Restart=always"; then pass "Restart=always (KeepAlive parity)"; else fail "Restart missing"; fi
if has "$UNIT" "KillSignal=SIGUSR1"; then pass "KillSignal=SIGUSR1 (gateway ignores SIGTERM)"; else fail "KillSignal missing"; fi
if has "$UNIT" "KillMode=process"; then pass "KillMode=process (leave engines+PG for re-adoption)"; else fail "KillMode missing"; fi
if has "$UNIT" "WantedBy=default.target"; then pass "WantedBy=default.target (starts at login)"; else fail "WantedBy missing"; fi

# ── PURE: systemd escaping — env values round-trip through the unit parser ────
echo ""
echo "test: service_systemd_escape_env doubles % and backslash-escapes quotes/backslashes"
if [ "$(service_systemd_escape_env 'KEY=ab%cd')" = 'KEY=ab%%cd' ]; then pass "% doubled (specifier expansion defused)"; else fail "%: $(service_systemd_escape_env 'KEY=ab%cd')"; fi
if [ "$(service_systemd_escape_env 'KEY=a"b')" = 'KEY=a\"b' ]; then
    pass "quote escaped"
else
    fail "quote: $(service_systemd_escape_env 'KEY=a"b')"
fi
if [ "$(service_systemd_escape_env 'KEY=a\b')" = 'KEY=a\\b' ]; then pass "backslash escaped"; else fail "backslash: $(service_systemd_escape_env 'KEY=a\b')"; fi
if [ "$(service_systemd_escape_env 'KEY=plain')" = 'KEY=plain' ]; then pass "plain value untouched"; else fail "plain: $(service_systemd_escape_env 'KEY=plain')"; fi
echo ""
echo "test: systemd unit escapes hostile env values, ExecStart, and WorkingDirectory"
HOSTILE_ENV='OPENAI_API_KEY=sk-10%off"then\some'
UNIT2="$(service_systemd_unit '/p/50% full/lucidos-gateway' '/p/50% full/test' "$HOSTILE_ENV")"
if has "$UNIT2" 'Environment="OPENAI_API_KEY=sk-10%%off\"then\\some"'; then pass "hostile env value escaped in Environment="; else fail "hostile env: $UNIT2"; fi
if has "$UNIT2" 'ExecStart="/p/50%% full/lucidos-gateway"'; then pass "ExecStart %-escaped + quoted"; else fail "ExecStart escape: $UNIT2"; fi
if has "$UNIT2" 'WorkingDirectory=/p/50%% full/test'; then pass "WorkingDirectory %-escaped"; else fail "workdir escape: $UNIT2"; fi

# ── PURE: uninstall purge target = the instance data dir ─────────────────────
echo ""
echo "test: service_uninstall_purge_targets = the instance data dir"
if [ "$(service_uninstall_purge_targets /home/u/.lucidos/test)" = "/home/u/.lucidos/test" ]; then pass "purge target = <data>"; else fail "purge target: $(service_uninstall_purge_targets /home/u/.lucidos/test)"; fi

# ── OFFLINE FS: service_list_instance_names lists slugs (port marker), skips runtime ─
echo ""
echo "test: service_list_instance_names lists instances (port marker), skips the shared runtime"
LP="$(mktemp -d)"
mkdir -p "$LP/default" "$LP/test" "$LP/runtime" "$LP/nomarker"
: > "$LP/default/port"; : > "$LP/test/port"   # only these are registered instances
listing="$(service_list_instance_names "$LP" | sort | tr '\n' ' ')"
if [ "$listing" = "default test " ]; then pass "lists default + test, skips runtime + markerless dirs"; else fail "listing: '$listing'"; fi
rm -rf "$LP"

# ── PURE: effectful wrappers exist (side effects isolated in thin wrappers) ───
echo ""
echo "test: effectful wrappers are defined"
for fn in service_detect_manager service_port_in_use service_list_instance_names service_write_file \
          service_launchd_load service_launchd_unload service_systemd_load service_systemd_unload \
          service_health_wait service_stop_embedded_runtime; do
    if type "$fn" >/dev/null 2>&1; then pass "$fn defined"; else fail "$fn missing"; fi
done

# ── INTEGRATION: --no-service → FOREGROUND launch (no registration) ──────────
echo ""
echo "test: --no-service runs in the foreground (no service registered)"
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
# Pin LUCIDOS_GATEWAY_DATA into the temp prefix — the suite runs inside a live
# workspace whose ambient LUCIDOS_GATEWAY_DATA points at the REAL gateway dir.
out="$(HOME="$FAKEHOME" LUCIDOS_NO_SERVICE=1 LUCIDOS_GATEWAY_DATA="$PREFIX/default" \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" 2>&1)"; rc=$?
if [ $rc -eq 0 ] && has "$out" "FOREGROUND" && has "$out" "--no-service"; then
    pass "--no-service took the foreground branch (exec gateway returned 0)"
else
    fail "expected a foreground launch (rc=$rc): $out"
fi
if [ -z "$(ls "$FAKEHOME/Library/LaunchAgents/" 2>/dev/null)" ]; then pass "no launchd plist written under --no-service"; else fail "a plist was written under --no-service"; fi
# The FOREGROUND shape has to record the port marker too. That marker is the
# whole of instance discovery (service_list_instance_names keys on it), so
# without it a --no-service or degraded install is invisible to
# `uninstall.sh --list` and unremovable by `--all --purge`, which returns early
# with no targets and leaves the data dir AND the shared runtime on disk.
assert_foreground_marker() {   # <prefix> <label>
    local pfx="$1" label="$2" marker="$1/default/port" listed
    if [ -f "$marker" ] && grep -qE '^[0-9]+$' "$marker"; then
        pass "$label recorded the instance port marker ($(tr -d '\n' < "$marker"))"
    else
        fail "$label left no usable port marker at $marker"
    fi
    listed="$(service_list_instance_names "$pfx")"
    if [ "$listed" = "default" ]; then
        pass "$label install is discoverable (service_list_instance_names finds it)"
    else
        fail "$label: service_list_instance_names '$pfx' = '$listed' (expected 'default')"
    fi
}
assert_foreground_marker "$PREFIX" "--no-service"
rm -rf "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: no service manager → graceful degrade to FOREGROUND ──────────
echo ""
echo "test: no supported service manager degrades to a foreground launch"
FB="$(make_fakebin PlanNine)"   # uname → unknown OS ⇒ decide_manager = none on ANY host
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA="$PREFIX/default" \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" 2>&1)"; rc=$?
if [ $rc -eq 0 ] && has "$out" "No supported user service manager" && has "$out" "FOREGROUND"; then
    pass "no-manager host degraded to foreground (no registration attempted)"
else
    fail "expected a no-manager degrade to foreground (rc=$rc): $out"
fi
if [ -z "$(ls "$FAKEHOME/Library/LaunchAgents/" 2>/dev/null)" ]; then pass "no plist written on the degrade path"; else fail "a plist was written on the degrade path"; fi
# The degrade path is the shape a container takes (install-smoke.yml's front-door
# job runs in a bare ubuntu:22.04 with no launchd and no systemd), and it is the
# one that used to leave nothing behind for the uninstaller to find.
assert_foreground_marker "$PREFIX" "the no-manager degrade"
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: register wiring (fake launchctl + fast health timeout) ───────
echo ""
echo "test: default install registers a slug-keyed launchd service (plist + port marker written)"
# uname→Darwin + a fake launchctl (reports loaded) forces the launchd register
# path on ANY host without touching real launchd. A 1s health timeout makes the
# (no real gateway) health wait fail fast; the plist + port marker are written
# before the health check, so we assert them despite the expected non-zero exit.
FB="$(make_fakebin Darwin 0)"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"; DATA="$PREFIX/reg"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA="$DATA" LUCIDOS_HEALTH_TIMEOUT=1 \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --name reg --port 59231 2>&1)"; rc=$?
PLIST="$FAKEHOME/Library/LaunchAgents/com.lucidos.gateway.reg.plist"
if [ -f "$PLIST" ]; then pass "wrote the slug-keyed LaunchAgent plist"; else fail "no plist at $PLIST: $out"; fi
if [ -f "$PLIST" ]; then
    if has "$(cat "$PLIST")" "<string>$PREFIX/runtime/current/lucidos-gateway</string>"; then
        pass "plist ProgramArguments points at the SHARED runtime"
    else
        fail "plist not pointing at shared runtime"
    fi
    if has "$(cat "$PLIST")" "<string>59231</string>"; then pass "plist carries the chosen port 59231"; else fail "plist missing port"; fi
fi
if [ -f "$DATA/port" ] && [ "$(cat "$DATA/port")" = "59231" ]; then pass "recorded the instance port marker (59231)"; else fail "port marker missing/wrong"; fi
if has "$out" "did not answer"; then pass "health check failed loud when no gateway came up"; else fail "expected a loud health failure: $out"; fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME" "$REL"

# ── INTEGRATION: --tls-cert/--tls-key bake TLS into the service env (https) ───
echo ""
echo "test: --tls-cert/--tls-key are baked into the plist and flip the health probe to https"
FB="$(make_fakebin Darwin 0)"
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"; DATA="$PREFIX/tlsreg"
CERTS="$(mktemp -d)"; : > "$CERTS/host.crt"; : > "$CERTS/host.key"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA="$DATA" LUCIDOS_HEALTH_TIMEOUT=1 \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --name tlsreg --port 59233 \
        --tls-cert "$CERTS/host.crt" --tls-key "$CERTS/host.key" 2>&1)"; rc=$?
PLIST="$FAKEHOME/Library/LaunchAgents/com.lucidos.gateway.tlsreg.plist"
if [ -f "$PLIST" ]; then
    if has "$(cat "$PLIST")" "<key>LUCIDOS_TLS_CERT</key>" && has "$(cat "$PLIST")" "<string>$CERTS/host.crt</string>"; then
        pass "plist carries LUCIDOS_TLS_CERT"
    else
        fail "plist missing TLS cert: $(cat "$PLIST")"
    fi
    if has "$(cat "$PLIST")" "<key>LUCIDOS_TLS_KEY</key>"; then pass "plist carries LUCIDOS_TLS_KEY"; else fail "plist missing TLS key"; fi
else
    fail "no plist at $PLIST: $out"
fi
if has "$out" "https://localhost:59233"; then pass "health failure message names the https URL"; else fail "expected an https health URL in: $out"; fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME" "$REL" "$CERTS"

# ── INTEGRATION: TLS flags are both-or-neither + files must exist ─────────────
echo ""
echo "test: TLS validation fails closed (one flag alone; missing files)"
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
out="$(HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch \
        --tls-cert /tmp/only-cert.pem 2>&1)"; rc=$?
if [ $rc -ne 0 ] && has "$out" "supplied together"; then
    pass "refused --tls-cert without --tls-key"
else
    fail "expected a both-or-neither refusal (rc=$rc): $out"
fi
out="$(HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch \
        --tls-cert /nonexistent/host.crt --tls-key /nonexistent/host.key 2>&1)"; rc=$?
if [ $rc -ne 0 ] && has "$out" "file not found"; then
    pass "refused a missing cert file"
else
    fail "expected a missing-file refusal (rc=$rc): $out"
fi
rm -rf "$REL" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: install.sh rejects a reserved --name (slug validation wired in) ─
echo ""
echo "test: install.sh refuses a reserved --name (finish_install gates on service_is_instance_name)"
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
# --no-launch isolates the check: finish_install validates the slug BEFORE the
# --no-launch short-circuit, so no service/foreground side effect is reached.
out="$(HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --name gateway --no-launch 2>&1)"; rc=$?
if [ $rc -ne 0 ] && has "$out" "reserved name"; then
    pass "install.sh refused the reserved slug 'gateway'"
else
    fail "expected a reserved-slug refusal (rc=$rc): $out"
fi
if [ -z "$(ls "$FAKEHOME/Library/LaunchAgents/" 2>/dev/null)" ]; then pass "nothing registered for a rejected slug"; else fail "a plist was written for a rejected slug"; fi
rm -rf "$REL" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: two instances installed via install.sh coexist ───────────────
echo ""
echo "test: two install.sh instances coexist (distinct data dirs + service ids + ports; ONE shared runtime)"
# uname→Darwin + a fake launchctl (reports loaded) forces the launchd register
# path on ANY host. A 1s health timeout makes each (no real gateway) health wait
# fail fast; the plist + port marker are written before the health check, so the
# coexistence artifacts are asserted despite the expected non-zero install exit.
FB="$(make_fakebin Darwin 0)"
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' LUCIDOS_HEALTH_TIMEOUT=1 \
    bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --name alpha --port 59241 >/dev/null 2>&1 || true
# The second instance reuses the already-extracted runtime (idempotent extract).
PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' LUCIDOS_HEALTH_TIMEOUT=1 \
    bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --name beta --port 59242 >/dev/null 2>&1 || true
PA="$FAKEHOME/Library/LaunchAgents/com.lucidos.gateway.alpha.plist"
PB="$FAKEHOME/Library/LaunchAgents/com.lucidos.gateway.beta.plist"
if [ -f "$PA" ] && [ -f "$PB" ]; then
    pass "distinct slug-suffixed plists (alpha + beta)"
else
    fail "missing a per-instance plist (alpha=$([ -f "$PA" ] && echo y || echo n) beta=$([ -f "$PB" ] && echo y || echo n))"
fi
if [ -d "$PREFIX/alpha" ] && [ -d "$PREFIX/beta" ]; then
    pass "distinct per-instance data dirs"
else
    fail "missing a per-instance data dir"
fi
pa="$(tr -d '[:space:]' < "$PREFIX/alpha/port" 2>/dev/null || true)"
pb="$(tr -d '[:space:]' < "$PREFIX/beta/port" 2>/dev/null || true)"
if [ "$pa" = "59241" ] && [ "$pb" = "59242" ] && [ "$pa" != "$pb" ]; then
    pass "distinct port markers (alpha=59241, beta=59242)"
else
    fail "port markers wrong (alpha='$pa' beta='$pb')"
fi
runtime_trees="$(find "$PREFIX/runtime" -maxdepth 1 -name 'lucidos-*' 2>/dev/null | wc -l | tr -d ' ')"
if [ -L "$PREFIX/runtime/current" ] && [ "$runtime_trees" = "1" ]; then
    pass "ONE shared runtime extracted (current symlink + single runtime tree)"
else
    fail "expected a single shared runtime (trees=$runtime_trees)"
fi
if has "$(cat "$PA")" "<string>$PREFIX/runtime/current/lucidos-gateway</string>" \
    && has "$(cat "$PB")" "<string>$PREFIX/runtime/current/lucidos-gateway</string>"; then
    pass "both instances' services point at the shared runtime gateway"
else
    fail "an instance plist does not point at the shared runtime"
fi
rm -rf "$FB" "$REL" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: remote-access flags (--bind; TLS × banner interplay) ─────────
# The TLS both-or-neither / missing-file refusals are covered above in "TLS
# validation fails closed" — these tests add --bind and the TLS install banner.
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
CERT="$(mktemp)"; KEY="$(mktemp)"; printf 'cert\n' > "$CERT"; printf 'key\n' > "$KEY"

echo ""
echo "test: an invalid --bind value is refused (gateway would silently fall back to loopback)"
out="$(HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --bind lan --no-launch 2>&1)"; rc=$?
if [ $rc -ne 0 ] && has "$out" "--bind must be"; then
    pass "invalid --bind refused"
else
    fail "expected a --bind refusal (rc=$rc): $out"
fi
if [ -e "$FAKEHOME/.lucidos/network.toml" ]; then fail "network.toml written despite refusal/--no-launch"; else pass "no network.toml side effect on refusal"; fi

echo ""
echo "test: a TLS install bakes the pair into the service env and probes https"
FB="$(make_fakebin Darwin 0)"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' LUCIDOS_HEALTH_TIMEOUT=1 \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --name tls --port 59251 \
        --tls-cert "$CERT" --tls-key "$KEY" 2>&1)"; rc=$?
PLIST="$FAKEHOME/Library/LaunchAgents/com.lucidos.gateway.tls.plist"
if [ -f "$PLIST" ] && has "$(cat "$PLIST")" "<key>LUCIDOS_TLS_CERT</key>" && has "$(cat "$PLIST")" "<key>LUCIDOS_TLS_KEY</key>"; then
    pass "TLS pair baked into the service env"
else
    fail "TLS env missing from the plist: $(cat "$PLIST" 2>/dev/null)"
fi
if has "$out" "https://localhost:59251"; then pass "health/banner URLs use https for a TLS install"; else fail "expected https URLs in output: $out"; fi
rm -rf "$FB"

echo ""
echo "test: --bind all writes the machine-global network.toml (the picker's knob, not unit env)"
FB="$(make_fakebin Darwin 0)"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' LUCIDOS_HEALTH_TIMEOUT=1 \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --name bnd --port 59252 --bind all 2>&1)"; rc=$?
NTF="$FAKEHOME/.lucidos/network.toml"
if [ -f "$NTF" ] && [ "$(service_network_toml_bind "$(cat "$NTF")")" = "all" ]; then
    pass "--bind all landed in ~/.lucidos/network.toml"
else
    fail "expected bind=all in $NTF: $(cat "$NTF" 2>/dev/null)"
fi
PLB="$FAKEHOME/Library/LaunchAgents/com.lucidos.gateway.bnd.plist"
if [ -f "$PLB" ] && ! has "$(cat "$PLB")" "LUCIDOS_GATEWAY_BIND"; then
    pass "no bind env baked into the unit (network.toml stays authoritative)"
else
    fail "bind env leaked into the service unit: $(cat "$PLB" 2>/dev/null)"
fi
rm -rf "$FB" "$REL" "$PREFIX" "$FAKEHOME" "$CERT" "$KEY"

# ── INTEGRATION: uninstall --list ────────────────────────────────────────────
echo ""
echo "test: uninstall.sh --list shows installed instances + ports (fake managers, no real launchd)"
FB="$(make_fakebin "" 1)"   # launchctl print → not loaded; systemctl → unavailable
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
mkdir -p "$PREFIX/default" "$PREFIX/test" "$PREFIX/runtime"
printf '5252\n' > "$PREFIX/default/port"; printf '5300\n' > "$PREFIX/test/port"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' bash "$UNINSTALL" --prefix "$PREFIX" --list 2>&1)"; rc=$?
if [ $rc -eq 0 ] && has "$out" "default" && has "$out" "5252" && has "$out" "test" && has "$out" "5300"; then
    pass "--list shows both instances + ports"
else
    fail "expected a listing of default+test (rc=$rc): $out"
fi
if has "$out" "runtime"; then fail "--list should not show the shared runtime as an instance"; else pass "--list skips the shared runtime"; fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: uninstall --help documents --purge + data-safety ────────────
echo ""
echo "test: uninstall.sh --help documents --name/--all/--purge and the data-safe default"
out="$(bash "$UNINSTALL" --help 2>&1)"
for tok in --name --all --list --purge "KEEP data"; do
    if has "$out" "$tok"; then pass "uninstall --help documents '$tok'"; else fail "uninstall --help missing '$tok'"; fi
done

# ── INTEGRATION: uninstall --name (no --purge) keeps data ────────────────────
echo ""
echo "test: uninstall.sh --name (no --purge) keeps the instance data, reports it"
FB="$(make_fakebin "" 1)"; PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
mkdir -p "$PREFIX/test"; printf '5300\n' > "$PREFIX/test/port"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' bash "$UNINSTALL" --prefix "$PREFIX" --name test 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ -d "$PREFIX/test" ] && has "$out" "Left your data in place"; then
    pass "no-purge uninstall kept the instance data + reported it"
else
    fail "expected a data-safe no-purge uninstall (rc=$rc, kept=$([ -d "$PREFIX/test" ] && echo y || echo n)): $out"
fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: uninstall --name --purge removes that instance's data ───────
echo ""
echo "test: uninstall.sh --name --purge removes the instance (plist + data) but not siblings"
FB="$(make_fakebin "" 0)"   # launchctl print → loaded, so the bootout + plist-removal path runs
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
mkdir -p "$PREFIX/test" "$PREFIX/default" "$FAKEHOME/Library/LaunchAgents"
printf '5300\n' > "$PREFIX/test/port"; printf '5252\n' > "$PREFIX/default/port"
PLIST="$FAKEHOME/Library/LaunchAgents/com.lucidos.gateway.test.plist"; printf 'x\n' > "$PLIST"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' bash "$UNINSTALL" --prefix "$PREFIX" --name test --purge 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ ! -e "$PREFIX/test" ] && [ ! -e "$PLIST" ] && [ -d "$PREFIX/default" ]; then
    pass "--name --purge removed test (data + plist), left default + runtime intact"
else
    fail "expected only 'test' removed (rc=$rc, test=$([ -e "$PREFIX/test" ] && echo y||echo n), default=$([ -d "$PREFIX/default" ] && echo y||echo n)): $out"
fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: uninstall --all --purge removes every instance + the runtime ─
echo ""
echo "test: uninstall.sh --all --purge removes every instance AND the shared runtime"
FB="$(make_fakebin "" 1)"; PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
mkdir -p "$PREFIX/default" "$PREFIX/test" "$PREFIX/runtime/$STEM"
printf '5252\n' > "$PREFIX/default/port"; printf '5300\n' > "$PREFIX/test/port"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' bash "$UNINSTALL" --prefix "$PREFIX" --all --purge 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ ! -e "$PREFIX/default" ] && [ ! -e "$PREFIX/test" ] && [ ! -e "$PREFIX/runtime" ]; then
    pass "--all --purge removed every instance + the shared runtime"
else
    fail "expected a full purge (rc=$rc): $out"
fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: uninstall --all with ZERO instances (bash 3.2 empty-array) ──
echo ""
echo "test: uninstall.sh --all with no instances is a clean no-op (no empty-array crash)"
FB="$(make_fakebin "" 1)"; PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
mkdir -p "$PREFIX/runtime"   # only the shared runtime; NO instance dirs
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' /bin/bash "$UNINSTALL" --prefix "$PREFIX" --all 2>&1)"; rc=$?
if [ $rc -eq 0 ] && has "$out" "No Lucidos instances"; then
    pass "--all with zero instances is a clean no-op under stock /bin/bash"
else
    fail "expected a clean no-op (rc=$rc): $out"
fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: uninstall removes the systemd unit FILE without a user bus ───
echo ""
echo "test: uninstall.sh removes the systemd unit file even when the user bus is unreachable"
# The fake systemctl always exits 1 → `systemctl --user show-environment` fails,
# emulating a bare ssh session with no XDG_RUNTIME_DIR/bus. The unit FILE must
# still be removed (else the \"uninstalled\" service resurrects at next boot);
# the running stack is left alone (a bus-less shell can't stop the gateway, and
# killing its engines would only make it respawn them).
FB="$(make_fakebin "" 1)"; PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
mkdir -p "$PREFIX/test"; printf '5300\n' > "$PREFIX/test/port"
UNITF="$FAKEHOME/.config/systemd/user/lucidos-gateway-test.service"
mkdir -p "$(dirname "$UNITF")"; printf '[Unit]\n' > "$UNITF"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" XDG_CONFIG_HOME='' LUCIDOS_GATEWAY_DATA='' \
        bash "$UNINSTALL" --prefix "$PREFIX" --name test 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ ! -e "$UNITF" ]; then
    pass "unit file removed despite the unreachable user bus"
else
    fail "expected the unit file gone (rc=$rc, present=$([ -e "$UNITF" ] && echo y || echo n)): $out"
fi
if has "$out" "session unreachable"; then pass "warned that a running service can't be stopped from here"; else fail "missing the unreachable-bus warning: $out"; fi
if [ -d "$PREFIX/test" ]; then pass "instance data kept (no --purge)"; else fail "data deleted without --purge"; fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: install.sh --uninstall delegates to uninstall.sh ─────────────
echo ""
echo "test: install.sh --uninstall --all --purge delegates to uninstall.sh"
FB="$(make_fakebin "" 1)"; PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
mkdir -p "$PREFIX/default"; printf '5252\n' > "$PREFIX/default/port"
out="$(PATH="$FB:$PATH" HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' bash "$INSTALL" --uninstall --all --purge --prefix "$PREFIX" 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ ! -e "$PREFIX/default" ] && has "$out" "uninstalled + purged"; then
    pass "install.sh --uninstall routed through uninstall.sh and purged"
else
    fail "expected install.sh --uninstall to delegate + purge (rc=$rc): $out"
fi
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

echo ""
echo "service: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
