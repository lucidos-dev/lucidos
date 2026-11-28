#!/usr/bin/env bash
# shellcheck disable=SC2034 # every name is read by a script that sources this file
# e2e_root.sh: the e2e root, where every local e2e run keeps its workspaces,
# its secrets and its Postgres (docs/glossary.md § e2e root).
#
# No gateway registers it, so an agent's OS sandbox leaves it writable
# (ADR 0645). It sits under the real home, so it stays machine-wide: two
# sessions still share the two e2e slots and their locks. Its private/ is the
# run's LUCIDOS_PRIVATE_DIR, so a run never reads this machine's secrets.
#
# Assignments and functions only, so any script or hermetic test can source it.

# Where the root sits under a home, unless LUCIDOS_E2E_ROOT moves it.
E2E_ROOT_HOME_SUBPATH=".lucidos/e2e-root"
E2E_ROOT="${LUCIDOS_E2E_ROOT:-$HOME/$E2E_ROOT_HOME_SUBPATH}"
E2E_DEFAULT_WORKSPACE="$E2E_ROOT/workspaces/e2e-test"

# The e2e-owned cluster. It runs beside the engine tests' lucidos-pg-test
# (5510) and never touches the live shared cluster.
E2E_PG_CONTAINER="lucidos-pg-e2e"
E2E_PG_VOLUME="lucidos-pg-data-e2e"
E2E_DEFAULT_PG_PORT="5511"

# A Unix socket path holds 103 bytes on macOS and 107 on Linux
# (lucidos-engine-socket's max_path_bytes), so the guard takes the smaller. The
# engine binds `<private>/run/<16 hex digest>`, and the private dir gets the rest.
E2E_SOCKET_PATH_MAX=103
E2E_SOCKET_SUFFIX_BYTES=21

# e2e_private_dir: the run's private subtree.
e2e_private_dir() {
    printf '%s/private\n' "$E2E_ROOT"
}

# e2e_root_env: point this process at the e2e root's secrets and cluster.
e2e_root_env() {
    export LUCIDOS_PRIVATE_DIR
    LUCIDOS_PRIVATE_DIR="$(e2e_private_dir)"
    # A session's own gateway data dir names the live gateway, never this run's.
    unset LUCIDOS_GATEWAY_DATA
    export LUCIDOS_SHARED_PG_CONTAINER="$E2E_PG_CONTAINER"
    export LUCIDOS_SHARED_PG_VOLUME="$E2E_PG_VOLUME"
}

# e2e_root_fits_socket: refuse, naming the way out, when the engine socket
# under the e2e root would break the limit. Counts bytes, as the kernel does.
e2e_root_fits_socket() {
    local bytes
    bytes="$(e2e_private_dir | LC_ALL=C wc -c)"
    # wc counts printf's trailing newline: one byte the path does not hold.
    if [ $(( bytes - 1 + E2E_SOCKET_SUFFIX_BYTES )) -gt "$E2E_SOCKET_PATH_MAX" ]; then
        echo "ERROR: the e2e root $E2E_ROOT is too long for the engine socket path." >&2
        echo "Set LUCIDOS_E2E_ROOT to a shorter directory and re-run." >&2
        return 1
    fi
}

# e2e_root_writable: refuse, naming the way out, when this process cannot
# write the e2e root, as under a sandbox that confines writes to a worktree.
# A real write, since access(2) does not see a sandbox's refusal.
e2e_root_writable() {
    local probe
    if mkdir -p "$E2E_ROOT" 2>/dev/null \
        && probe="$(mktemp "$E2E_ROOT/.write-probe.XXXXXX" 2>/dev/null)"; then
        rm -f "$probe"
        return 0
    fi
    echo "ERROR: this process cannot write the e2e root $E2E_ROOT." >&2
    echo "A sandbox that confines writes to the worktree refuses it. Re-run with --github." >&2
    return 1
}

# e2e_pg_port: the e2e cluster's host port.
e2e_pg_port() {
    printf '%s\n' "${LUCIDOS_E2E_PG_PORT:-$E2E_DEFAULT_PG_PORT}"
}
