#!/bin/bash
# Pre-Bash hook: block kill patterns that would take down the host engine
# or its sibling frontend.
#
# Belt-and-suspenders layer to the production-side fix in scripts/lib/ports.sh
# (is_protected_host_pid + kill_unprotected_pids). The ports.sh guard catches
# indirect kills routed through allocate_ports; this hook catches direct
# `kill <pid>` / `pkill` / `killall` / `lsof | xargs kill` patterns the model
# types verbatim.
#
# Env vars come from the spawning subprocess (set by the engine via
# api::actor::host_protection_env_vars):
#   LUCIDOS_HOST_PID       — engine process id (always set)
#   LUCIDOS_FRONTEND_PID   — sibling Vite pid (web-dev only; may be unset)
#   LUCIDOS_API_PORT       — engine port (re-exported from the engine's env)

# jq reads from stdin directly — no need for an intermediate `cat`.
COMMAND=$(jq -r '.tool_input.command // empty')
[ -z "$COMMAND" ] && exit 0

block() {
    cat >&2 <<EOF
BLOCKED: $1

This command would kill the Lucidos host engine (or its sibling frontend),
taking down the workspace that owns this Claude Code session.

If you actually need to stop a workspace, use:
  ./scripts/stop.sh -w <workspace>
EOF
    exit 2
}

# `CMD_START` matches a position where bash parses the next word as a command
# name: start-of-string, after `;`, `&`, `&&`, `|`, `||`, `(`, `` ` ``, `$(`,
# or a NEWLINE — with optional whitespace. The newline must be a real `\n`
# byte (interpolated via $'…'); the escape sequence `\n` inside a bash
# `[[ =~ ]]` regex character class is literal backslash+n, not a newline,
# which would let multi-line scripts slip past the anchor.
#
# `PATH_PREFIX` matches an optional path prefix ending in `/`, so absolute
# paths like `/bin/kill` or `/usr/bin/pkill` are caught alongside the bare
# commands. The `[^[:space:];&|(]*` prefix stops at any shell separator, so
# the prefix can't accidentally span across commands.
_NL=$'\n'
CMD_START="(^|[;&|\`(${_NL}])[[:space:]]*"
PATH_PREFIX='([^[:space:];&|(]*/)?'

# 1) Direct `kill <pid>` against the host or frontend. The trailing
# `[^[:alnum:]]|$` enforces a numeric word boundary so `kill 123456` doesn't
# false-positive on host pid 12345.
for var in LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID; do
    pid="${!var:-}"
    [ -z "$pid" ] && continue
    if [[ "$COMMAND" =~ ${CMD_START}${PATH_PREFIX}kill([[:space:]]+-[[:alnum:]]+)?[[:space:]]+([[:alnum:]_,[:space:]-]+[[:space:]]+)?${pid}([^[:alnum:]]|$) ]]; then
        block "kill targets $var ($pid)"
    fi
done

# 2) Broad pkill/killall against the engine binary name. CLAUDE.md already
# warns these exclude ancestors on macOS, so the caller survives while every
# sibling workspace's engine dies.
for cmd_name in pkill killall; do
    if [[ "$COMMAND" =~ ${CMD_START}${PATH_PREFIX}${cmd_name}([[:space:]]+-[[:alnum:]]+)*[[:space:]]+[^[:space:]]*lucidos-engine ]]; then
        block "$cmd_name lucidos-engine kills every workspace's engine"
    fi
done

# 3) `lsof -ti :<host_port> | xargs kill` — the canonical "free up that port"
# recipe. Bare `lsof -ti :<port>` for inspection is fine; the `| xargs kill`
# (or `xargs -n1 kill`, `xargs -r kill`, etc.) is what makes it lethal, so
# both halves must be present. The `(-|[[:space:]]|$)` after `kill` anchors
# the word so `xargs killall` doesn't surface a confusing "blocked: lsof |
# xargs kill" message for a different (also blocked) pattern.
if [ -n "${LUCIDOS_API_PORT:-}" ]; then
    if [[ "$COMMAND" =~ ${CMD_START}${PATH_PREFIX}lsof[[:space:]]+(-[[:alnum:]]+[[:space:]]+)*:${LUCIDOS_API_PORT}([^[:digit:]]|$) ]] \
       && [[ "$COMMAND" =~ xargs([[:space:]]+-[[:alnum:]]+)*[[:space:]]+${PATH_PREFIX}kill(-|[[:space:]]|$) ]]; then
        block "lsof :$LUCIDOS_API_PORT | xargs kill targets the engine port"
    fi
fi

exit 0
