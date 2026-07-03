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
        { printf '#!/bin/sh\n'
          printf '[ "$1" = "-s" ] && { echo %s; exit 0; }\n' "$os"
          printf 'echo %s\n' "$os"; } > "$dir/uname"
        chmod +x "$dir/uname"
    fi
    printf '#!/bin/sh\ncase "$1" in print) exit %s;; *) exit 0;; esac\n' "$rc" > "$dir/launchctl"
    printf '#!/bin/sh\nexit 1\n' > "$dir/systemctl"
    chmod +x "$dir/launchctl" "$dir/systemctl"
    printf '%s' "$dir"
}

# ── PURE: identity (slug-suffixed) ────────────────────────────────────────────
echo "test: service identity is slug-suffixed (coexists; never collides with the .app's com.lucidos.engine)"
[ "$(service_launchd_label default)" = "com.lucidos.gateway.default" ] && pass "launchd label (default)" || fail "label: $(service_launchd_label default)"
[ "$(service_launchd_label test)" = "com.lucidos.gateway.test" ] && pass "launchd label (test)" || fail "label: $(service_launchd_label test)"
[ "$(service_systemd_unit_name test)" = "lucidos-gateway-test.service" ] && pass "systemd unit name" || fail "unit: $(service_systemd_unit_name test)"

# ── PURE: validation ──────────────────────────────────────────────────────────
echo ""
echo "test: service_is_instance_name accepts slugs, rejects junk + reserved names"
for ok_slug in default test ab-cd x1; do
    service_is_instance_name "$ok_slug" && pass "accepts '$ok_slug'" || fail "rejected valid '$ok_slug'"
done
for bad in "" "UP" "a b" "-x" "x-" "a_b" gateway runtime current logs; do
    service_is_instance_name "$bad" && fail "accepted invalid '$bad'" || pass "rejects '$bad'"
done
echo ""
echo "test: service_is_port_number bounds"
for ok_p in 1 5252 65535; do service_is_port_number "$ok_p" && pass "accepts $ok_p" || fail "rejected $ok_p"; done
for bad_p in 0 65536 "" abc 52x; do service_is_port_number "$bad_p" && fail "accepted $bad_p" || pass "rejects '$bad_p'"; done

# ── PURE: paths ───────────────────────────────────────────────────────────────
echo ""
echo "test: path resolution (data dir, port file, plist, unit, logs, runtime root, health URL)"
[ "$(service_instance_data_dir /opt/x test)" = "/opt/x/test" ] && pass "data dir = <prefix>/<slug>" || fail "data dir: $(service_instance_data_dir /opt/x test)"
[ "$(service_instance_port_file /opt/x/test)" = "/opt/x/test/port" ] && pass "port file = <data>/port" || fail "port file: $(service_instance_port_file /opt/x/test)"
got="$(service_launchd_plist_path /Users/me test)"
[ "$got" = "/Users/me/Library/LaunchAgents/com.lucidos.gateway.test.plist" ] && pass "plist path = $got" || fail "plist path: $got"
got="$(service_systemd_unit_path /home/u test)"
[ "$got" = "/home/u/.config/systemd/user/lucidos-gateway-test.service" ] && pass "unit path (default XDG) = $got" || fail "unit path: $got"
got="$(service_systemd_unit_path /home/u test /home/u/.xdg)"
[ "$got" = "/home/u/.xdg/systemd/user/lucidos-gateway-test.service" ] && pass "unit path honors XDG_CONFIG_HOME" || fail "unit path XDG: $got"
[ "$(service_log_dir /opt/x/test)" = "/opt/x/test/logs" ] && pass "log dir = <data>/logs" || fail "log dir: $(service_log_dir /opt/x/test)"
[ "$(service_runtime_root /opt/x)" = "/opt/x/runtime/current" ] && pass "runtime root = shared current symlink" || fail "runtime root: $(service_runtime_root /opt/x)"
[ "$(service_health_url 5252)" = "http://localhost:5252/~/api/v1/health" ] && pass "health url (sigil namespace)" || fail "health url: $(service_health_url 5252)"
[ "$(service_health_url 5252 https)" = "https://localhost:5252/~/api/v1/health" ] && pass "health url honors the https scheme (TLS instance)" || fail "health url https: $(service_health_url 5252 https)"

# ── PURE: bind-value validation + network.toml render/parse round-trip ────────
echo ""
echo "test: service_is_bind_value accepts all/loopback/IPs, rejects junk"
for ok_b in all loopback 0.0.0.0 192.168.1.7 100.64.0.1 :: ::1 fd7a:115c:a1e0::1; do
    service_is_bind_value "$ok_b" && pass "accepts '$ok_b'" || fail "rejected valid '$ok_b'"
done
for bad_b in "" lan yes 256.1.1.1 1.2.3 "192.168.1.7 evil" "all;rm"; do
    service_is_bind_value "$bad_b" && fail "accepted invalid '$bad_b'" || pass "rejects '$bad_b'"
done

echo ""
echo "test: network.toml render mirrors the gateway writer; bind/inherit parse back out"
NT="$(service_render_network_toml all true)"
has "$NT" '[gateway]' && has "$NT" 'bind = "all"' && pass "render carries [gateway] bind" || fail "render: $NT"
has "$NT" '[engine]' && has "$NT" 'inherit = true' && pass "render carries [engine] inherit" || fail "render inherit: $NT"
[ "$(service_network_toml_bind "$NT")" = "all" ] && pass "bind parses back out" || fail "bind parse: $(service_network_toml_bind "$NT")"
[ "$(service_network_toml_engine_inherit "$NT")" = "true" ] && pass "inherit parses back out" || fail "inherit parse"
NT2="$(service_render_network_toml 100.64.0.1 false)"
[ "$(service_network_toml_bind "$NT2")" = "100.64.0.1" ] && pass "IP bind round-trips" || fail "IP bind: $(service_network_toml_bind "$NT2")"
[ "$(service_network_toml_engine_inherit "$NT2")" = "false" ] && pass "inherit=false round-trips" || fail "inherit=false parse"
[ "$(service_network_toml_engine_inherit "")" = "true" ] && pass "absent file defaults inherit=true" || fail "empty-contents inherit default"

# ── OFFLINE FS: service_write_network_toml writes + preserves inherit ─────────
echo ""
echo "test: service_write_network_toml writes fresh + preserves an existing inherit=false"
NTH="$(mktemp -d)"
service_write_network_toml "$NTH" all || fail "fresh network.toml write failed"
[ "$(service_network_toml_bind "$(cat "$NTH/.lucidos/network.toml")")" = "all" ] && pass "fresh write records bind=all" || fail "fresh write: $(cat "$NTH/.lucidos/network.toml" 2>/dev/null)"
service_render_network_toml loopback false > "$NTH/.lucidos/network.toml"
service_write_network_toml "$NTH" 192.168.1.7 || fail "rewrite failed"
NTC="$(cat "$NTH/.lucidos/network.toml")"
[ "$(service_network_toml_bind "$NTC")" = "192.168.1.7" ] && pass "rewrite updates bind" || fail "rewrite bind: $NTC"
[ "$(service_network_toml_engine_inherit "$NTC")" = "false" ] && pass "rewrite preserves inherit=false" || fail "rewrite lost inherit: $NTC"
rm -rf "$NTH"

# ── PURE: env contract (matches spawn_gateway; shared runtime + per-instance data) ─
echo ""
echo "test: service_runtime_env_pairs matches the spawn_gateway env contract"
env_block="$(service_runtime_env_pairs /opt/x/runtime/current /opt/x/test 5252)"
check_env() { has "$env_block" "$1" && pass "env has $1" || fail "env missing $1: $env_block"; }
check_env "LUCIDOS_API_PORT=5252"
check_env "LUCIDOS_GATEWAY_DATA=/opt/x/test"
check_env "LUCIDOS_GATEWAY_PG_BACKEND=embedded"
check_env "LUCIDOS_PG_BIN_DIR=/opt/x/runtime/current/postgres/bin"
check_env "LUCIDOS_PG_LIB_DIR=/opt/x/runtime/current/postgres/lib"
check_env "LUCIDOS_ENGINE_BIN=/opt/x/runtime/current/lucidos-engine"
check_env "LUCIDOS_CLI_BIN=/opt/x/runtime/current/lucidos"
check_env "LUCIDOS_STATIC_DIR=/opt/x/runtime/current/frontend"
check_env "LUCIDOS_SDK_DIR=/opt/x/runtime/current/sdk"
check_env "FASTEMBED_CACHE_DIR=/opt/x/test/fastembed"
check_env "LUCIDOS_BOOT_WITHOUT_PROVIDER=1"
check_env "LUCIDOS_PACKAGED=1"
# TLS is OPT-IN: the base contract must NOT carry TLS vars (packaged posture is
# plain http; install.sh appends service_tls_env_pairs only when both flags are
# supplied).
has "$env_block" "LUCIDOS_TLS_CERT" && fail "base env must not carry LUCIDOS_TLS_CERT" || pass "base env has no TLS vars (opt-in only)"
echo ""
echo "test: service_tls_env_pairs renders the opt-in TLS pairs"
tls_block="$(service_tls_env_pairs /p/certs/host.crt /p/certs/host.key)"
has "$tls_block" "LUCIDOS_TLS_CERT=/p/certs/host.crt" && pass "TLS cert pair" || fail "TLS cert pair: $tls_block"
has "$tls_block" "LUCIDOS_TLS_KEY=/p/certs/host.key" && pass "TLS key pair" || fail "TLS key pair: $tls_block"

# ── PURE: manager DECISION + compose decision ─────────────────────────────────
echo ""
echo "test: service_decide_manager across faked (os, launchctl, systemd-user)"
[ "$(service_decide_manager Darwin 1 0)" = "launchd" ]      && pass "macOS + launchctl → launchd" || fail "Darwin 1 0"
[ "$(service_decide_manager Darwin 0 0)" = "none" ]         && pass "macOS w/o launchctl → none" || fail "Darwin 0 0"
[ "$(service_decide_manager Linux 0 1)" = "systemd-user" ]  && pass "Linux + systemd --user → systemd-user" || fail "Linux 0 1"
[ "$(service_decide_manager Linux 0 0)" = "none" ]          && pass "Linux w/o systemd --user → none" || fail "Linux 0 0"
[ "$(service_decide_manager PlanNine 1 1)" = "none" ]       && pass "unknown OS → none" || fail "PlanNine 1 1"
echo ""
echo "test: service_compose_decision (default service; --no-service + no-manager degrade)"
[ "$(service_compose_decision "" launchd)" = "service" ]       && pass "default + launchd → service" || fail "'' launchd"
[ "$(service_compose_decision 1 launchd)" = "foreground" ]     && pass "--no-service → foreground" || fail "1 launchd"
[ "$(service_compose_decision "" none)" = "foreground" ]       && pass "no manager → foreground (degrade)" || fail "'' none"

# ── PURE: command-arg builders, xml escape, port candidates ──────────────────
echo ""
echo "test: launchctl domain/target builders + xml escape + port candidates"
[ "$(service_launchd_domain 501)" = "gui/501" ] && pass "domain = gui/<uid>" || fail "domain"
[ "$(service_launchd_target 501 com.lucidos.gateway.test)" = "gui/501/com.lucidos.gateway.test" ] && pass "target = gui/<uid>/<label>" || fail "target"
[ "$(service_xml_escape '/a&b/<x>')" = "/a&amp;b/&lt;x&gt;" ] && pass "xml escape" || fail "xml escape"
cand="$(service_port_candidates 5252 3)"
[ "$cand" = "$(printf '5252\n5253\n5254')" ] && pass "port candidates ascend from base" || fail "candidates: $cand"

# ── PURE: launchd plist content ──────────────────────────────────────────────
echo ""
echo "test: launchd plist embeds the gateway-only ProgramArguments, env, KeepAlive, logs"
PL="$(service_launchd_plist com.lucidos.gateway.test /p/runtime/current/lucidos-gateway /p/test /p/test/logs/gateway.out.log /p/test/logs/gateway.err.log "$env_block")"
has "$PL" "<string>com.lucidos.gateway.test</string>" && pass "plist has the slug-suffixed label" || fail "plist label missing"
has "$PL" "<string>/p/runtime/current/lucidos-gateway</string>" && pass "ProgramArguments = the shared gateway binary" || fail "plist gateway path missing"
n="$(printf '%s\n' "$PL" | awk '/<key>ProgramArguments<\/key>/{f=1;next} f&&/<\/array>/{f=0} f&&/<string>/{c++} END{print c+0}')"
[ "$n" = "1" ] && pass "ProgramArguments has exactly one entry (gateway only, not per-engine)" || fail "ProgramArguments entries: $n"
has "$PL" "<key>RunAtLoad</key>" && pass "RunAtLoad present" || fail "RunAtLoad missing"
has "$PL" "<key>KeepAlive</key>" && pass "KeepAlive present" || fail "KeepAlive missing"
has "$PL" "<key>LUCIDOS_API_PORT</key>" && has "$PL" "<string>5252</string>" && pass "env var rendered as key/string" || fail "env var not rendered"
has "$PL" "<string>/p/test/logs/gateway.out.log</string>" && pass "StandardOutPath in the instance log dir" || fail "out log missing"
has "$PL" "<string>/p/test</string>" && pass "WorkingDirectory = the instance data dir" || fail "workdir missing"

# ── PURE: systemd unit content ───────────────────────────────────────────────
echo ""
echo "test: systemd unit embeds ExecStart (gateway only), Environment, SIGUSR1, WantedBy"
UNIT="$(service_systemd_unit /p/runtime/current/lucidos-gateway /p/test "$env_block")"
has "$UNIT" 'ExecStart="/p/runtime/current/lucidos-gateway"' && pass "ExecStart = the shared gateway (quoted word)" || fail "ExecStart wrong"
n="$(printf '%s\n' "$UNIT" | grep -c '^ExecStart=')"
[ "$n" = "1" ] && pass "exactly one ExecStart (gateway only)" || fail "ExecStart count: $n"
has "$UNIT" 'Environment="LUCIDOS_API_PORT=5252"' && pass "Environment= rendered" || fail "Environment missing"
has "$UNIT" "WorkingDirectory=/p/test" && pass "WorkingDirectory = the instance data dir" || fail "WorkingDirectory missing"
has "$UNIT" "Restart=always" && pass "Restart=always (KeepAlive parity)" || fail "Restart missing"
has "$UNIT" "KillSignal=SIGUSR1" && pass "KillSignal=SIGUSR1 (gateway ignores SIGTERM)" || fail "KillSignal missing"
has "$UNIT" "KillMode=process" && pass "KillMode=process (leave engines+PG for re-adoption)" || fail "KillMode missing"
has "$UNIT" "WantedBy=default.target" && pass "WantedBy=default.target (starts at login)" || fail "WantedBy missing"

# ── PURE: systemd escaping — env values round-trip through the unit parser ────
echo ""
echo "test: service_systemd_escape_env doubles % and backslash-escapes quotes/backslashes"
[ "$(service_systemd_escape_env 'KEY=ab%cd')" = 'KEY=ab%%cd' ] && pass "% doubled (specifier expansion defused)" || fail "%: $(service_systemd_escape_env 'KEY=ab%cd')"
[ "$(service_systemd_escape_env 'KEY=a"b')" = 'KEY=a\"b' ] && pass "quote escaped" || fail "quote: $(service_systemd_escape_env 'KEY=a"b')"
[ "$(service_systemd_escape_env 'KEY=a\b')" = 'KEY=a\\b' ] && pass "backslash escaped" || fail "backslash: $(service_systemd_escape_env 'KEY=a\b')"
[ "$(service_systemd_escape_env 'KEY=plain')" = 'KEY=plain' ] && pass "plain value untouched" || fail "plain: $(service_systemd_escape_env 'KEY=plain')"
echo ""
echo "test: systemd unit escapes hostile env values, ExecStart, and WorkingDirectory"
HOSTILE_ENV='OPENAI_API_KEY=sk-10%off"then\some'
UNIT2="$(service_systemd_unit '/p/50% full/lucidos-gateway' '/p/50% full/test' "$HOSTILE_ENV")"
has "$UNIT2" 'Environment="OPENAI_API_KEY=sk-10%%off\"then\\some"' && pass "hostile env value escaped in Environment=" || fail "hostile env: $UNIT2"
has "$UNIT2" 'ExecStart="/p/50%% full/lucidos-gateway"' && pass "ExecStart %-escaped + quoted" || fail "ExecStart escape: $UNIT2"
has "$UNIT2" 'WorkingDirectory=/p/50%% full/test' && pass "WorkingDirectory %-escaped" || fail "workdir escape: $UNIT2"

# ── PURE: uninstall purge target = the instance data dir ─────────────────────
echo ""
echo "test: service_uninstall_purge_targets = the instance data dir"
[ "$(service_uninstall_purge_targets /home/u/.lucidos/test)" = "/home/u/.lucidos/test" ] && pass "purge target = <data>" || fail "purge target: $(service_uninstall_purge_targets /home/u/.lucidos/test)"

# ── OFFLINE FS: service_list_instance_names lists slugs (port marker), skips runtime ─
echo ""
echo "test: service_list_instance_names lists instances (port marker), skips the shared runtime"
LP="$(mktemp -d)"
mkdir -p "$LP/default" "$LP/test" "$LP/runtime" "$LP/nomarker"
: > "$LP/default/port"; : > "$LP/test/port"   # only these are registered instances
listing="$(service_list_instance_names "$LP" | sort | tr '\n' ' ')"
[ "$listing" = "default test " ] && pass "lists default + test, skips runtime + markerless dirs" || fail "listing: '$listing'"
rm -rf "$LP"

# ── PURE: effectful wrappers exist (side effects isolated in thin wrappers) ───
echo ""
echo "test: effectful wrappers are defined"
for fn in service_detect_manager service_port_in_use service_list_instance_names service_write_file \
          service_launchd_load service_launchd_unload service_systemd_load service_systemd_unload \
          service_health_wait service_stop_embedded_runtime; do
    type "$fn" >/dev/null 2>&1 && pass "$fn defined" || fail "$fn missing"
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
[ -z "$(ls "$FAKEHOME/Library/LaunchAgents/" 2>/dev/null)" ] && pass "no launchd plist written under --no-service" || fail "a plist was written under --no-service"
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
[ -z "$(ls "$FAKEHOME/Library/LaunchAgents/" 2>/dev/null)" ] && pass "no plist written on the degrade path" || fail "a plist was written on the degrade path"
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
[ -f "$PLIST" ] && pass "wrote the slug-keyed LaunchAgent plist" || fail "no plist at $PLIST: $out"
if [ -f "$PLIST" ]; then
    has "$(cat "$PLIST")" "<string>$PREFIX/runtime/current/lucidos-gateway</string>" \
        && pass "plist ProgramArguments points at the SHARED runtime" || fail "plist not pointing at shared runtime"
    has "$(cat "$PLIST")" "<string>59231</string>" && pass "plist carries the chosen port 59231" || fail "plist missing port"
fi
[ -f "$DATA/port" ] && [ "$(cat "$DATA/port")" = "59231" ] && pass "recorded the instance port marker (59231)" || fail "port marker missing/wrong"
has "$out" "did not answer" && pass "health check failed loud when no gateway came up" || fail "expected a loud health failure: $out"
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
    has "$(cat "$PLIST")" "<key>LUCIDOS_TLS_CERT</key>" && has "$(cat "$PLIST")" "<string>$CERTS/host.crt</string>" \
        && pass "plist carries LUCIDOS_TLS_CERT" || fail "plist missing TLS cert: $(cat "$PLIST")"
    has "$(cat "$PLIST")" "<key>LUCIDOS_TLS_KEY</key>" && pass "plist carries LUCIDOS_TLS_KEY" || fail "plist missing TLS key"
else
    fail "no plist at $PLIST: $out"
fi
has "$out" "https://localhost:59233" && pass "health failure message names the https URL" || fail "expected an https health URL in: $out"
rm -rf "$FB" "$PREFIX" "$FAKEHOME" "$REL" "$CERTS"

# ── INTEGRATION: TLS flags are both-or-neither + files must exist ─────────────
echo ""
echo "test: TLS validation fails closed (one flag alone; missing files)"
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"; FAKEHOME="$(mktemp -d)"
out="$(HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch \
        --tls-cert /tmp/only-cert.pem 2>&1)"; rc=$?
[ $rc -ne 0 ] && has "$out" "supplied together" && pass "refused --tls-cert without --tls-key" \
    || fail "expected a both-or-neither refusal (rc=$rc): $out"
out="$(HOME="$FAKEHOME" LUCIDOS_GATEWAY_DATA='' \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch \
        --tls-cert /nonexistent/host.crt --tls-key /nonexistent/host.key 2>&1)"; rc=$?
[ $rc -ne 0 ] && has "$out" "file not found" && pass "refused a missing cert file" \
    || fail "expected a missing-file refusal (rc=$rc): $out"
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
[ -z "$(ls "$FAKEHOME/Library/LaunchAgents/" 2>/dev/null)" ] && pass "nothing registered for a rejected slug" || fail "a plist was written for a rejected slug"
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
[ -f "$PA" ] && [ -f "$PB" ] && pass "distinct slug-suffixed plists (alpha + beta)" \
    || fail "missing a per-instance plist (alpha=$([ -f "$PA" ] && echo y || echo n) beta=$([ -f "$PB" ] && echo y || echo n))"
[ -d "$PREFIX/alpha" ] && [ -d "$PREFIX/beta" ] && pass "distinct per-instance data dirs" \
    || fail "missing a per-instance data dir"
pa="$(tr -d '[:space:]' < "$PREFIX/alpha/port" 2>/dev/null || true)"
pb="$(tr -d '[:space:]' < "$PREFIX/beta/port" 2>/dev/null || true)"
[ "$pa" = "59241" ] && [ "$pb" = "59242" ] && [ "$pa" != "$pb" ] && pass "distinct port markers (alpha=59241, beta=59242)" \
    || fail "port markers wrong (alpha='$pa' beta='$pb')"
runtime_trees="$(ls -d "$PREFIX"/runtime/lucidos-* 2>/dev/null | wc -l | tr -d ' ')"
[ -L "$PREFIX/runtime/current" ] && [ "$runtime_trees" = "1" ] \
    && pass "ONE shared runtime extracted (current symlink + single runtime tree)" || fail "expected a single shared runtime (trees=$runtime_trees)"
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
[ -e "$FAKEHOME/.lucidos/network.toml" ] && fail "network.toml written despite refusal/--no-launch" || pass "no network.toml side effect on refusal"

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
has "$out" "https://localhost:59251" && pass "health/banner URLs use https for a TLS install" || fail "expected https URLs in output: $out"
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
has "$out" "runtime" && fail "--list should not show the shared runtime as an instance" || pass "--list skips the shared runtime"
rm -rf "$FB" "$PREFIX" "$FAKEHOME"

# ── INTEGRATION: uninstall --help documents --purge + data-safety ────────────
echo ""
echo "test: uninstall.sh --help documents --name/--all/--purge and the data-safe default"
out="$(bash "$UNINSTALL" --help 2>&1)"
for tok in --name --all --list --purge "KEEP data"; do
    has "$out" "$tok" && pass "uninstall --help documents '$tok'" || fail "uninstall --help missing '$tok'"
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
has "$out" "session unreachable" && pass "warned that a running service can't be stopped from here" || fail "missing the unreachable-bus warning: $out"
[ -d "$PREFIX/test" ] && pass "instance data kept (no --purge)" || fail "data deleted without --purge"
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
