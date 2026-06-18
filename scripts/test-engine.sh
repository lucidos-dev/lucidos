#!/usr/bin/env bash
#
# Run the lucidos-engine test suite against a DEDICATED, disposable Postgres
# (pgvector) container, so `make test` works whether or not any workspace
# engine happens to be running.
#
# The engine's integration tests (`setup_test_db` in src/test_support.rs) read
# TEST_DATABASE_URL and CREATE/DROP throwaway `lucidos_test_*` databases on it.
# This script provisions that database and exports the URL. Without it the tests
# fall back to a hardcoded localhost:5432 that only exists if a workspace PG
# happens to be up — the fragile setup that made ~470 tests "fail" the moment
# every workspace container exited.
#
# Why a DEDICATED container (`lucidos-pg-test`), not a workspace's PG:
#   - tests CREATE/DROP databases; pointing them at the personal/dev workspace
#     PG would mutate that instance and couple test runs to a running workspace.
#   - isolation is by name + port + image; nothing else touches this container.
#
# This script NEVER broad-kills. (The previous test-engine.sh was deleted for
# `pkill -f cognos-engine`, which killed unrelated processes — see CLAUDE.md
# "Never kill broadly".) It only ever touches its own `lucidos-pg-test`
# container by exact name.
#
# Usage:
#   ./scripts/test-engine.sh                  # cargo test -p lucidos-engine --lib
#   ./scripts/test-engine.sh --full           # whole crate (lib + integration + doctests)
#   ./scripts/test-engine.sh --fresh          # recreate the test DB container first
#   ./scripts/test-engine.sh -- <cargo args>  # everything after -- is appended to
#                                             # the `cargo test` invocation verbatim.
#                                             # To filter by test name (one or many)
#                                             # or pass harness flags, include a second
#                                             # -- so they reach the test binary:
#                                             #   -- -- migration_tests
#                                             #   -- -- mymod another_mod --test-threads=4
#
# Env:
#   LUCIDOS_TEST_PG_PORT   host port for the test container (default 5510)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

PG_IMAGE="pgvector/pgvector:pg18"      # same image the workspaces use (has pgvector)
PG_CONTAINER="lucidos-pg-test"
PG_PORT="${LUCIDOS_TEST_PG_PORT:-5510}"  # off the 5432+ workspace range on purpose

FULL=0
FRESH=0
PASSTHRU=()
while [ $# -gt 0 ]; do
    case "$1" in
        --full)  FULL=1; shift ;;
        --fresh) FRESH=1; shift ;;
        --)      shift; PASSTHRU=("$@"); break ;;
        *)       PASSTHRU+=("$1"); shift ;;
    esac
done

if ! docker info >/dev/null 2>&1; then
    echo "ERROR: Docker is not running. Start Docker Desktop and retry." >&2
    exit 1
fi

ensure_test_pg() {
    if [ "$FRESH" = "1" ]; then
        echo "[test-db] --fresh: removing existing $PG_CONTAINER"
        docker rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
    fi

    if docker inspect "$PG_CONTAINER" >/dev/null 2>&1; then
        local status
        status="$(docker inspect --format '{{.State.Status}}' "$PG_CONTAINER" 2>/dev/null || echo "")"
        if [ "$status" != "running" ]; then
            echo "[test-db] starting existing $PG_CONTAINER"
            docker start "$PG_CONTAINER" >/dev/null
        else
            echo "[test-db] reusing running $PG_CONTAINER"
        fi
        # The container may have been created with a different host port; read
        # back the real one so TEST_DATABASE_URL always matches reality.
        PG_PORT="$(docker inspect \
            --format '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' \
            "$PG_CONTAINER" 2>/dev/null || echo "$PG_PORT")"
    else
        # Best-effort preflight: only meaningful where lsof exists. Guard the
        # `command -v` so a missing lsof (minimal CI images) doesn't make the
        # bare `lsof` exit-127 read as "port free" — in that case we skip the
        # check and let `docker run -p` surface its own bind error.
        if command -v lsof >/dev/null 2>&1 && lsof -ti :"$PG_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
            echo "ERROR: test PG port $PG_PORT is in use by another process." >&2
            echo "       Set LUCIDOS_TEST_PG_PORT to a free port and retry." >&2
            exit 1
        fi
        echo "[test-db] creating $PG_CONTAINER on port $PG_PORT ($PG_IMAGE)"
        # max_connections raised so full-parallel test runs (one ~5-conn pool per
        # test thread) never exhaust the server.
        docker run -d \
            --name "$PG_CONTAINER" \
            -e POSTGRES_USER=lucidos \
            -e POSTGRES_PASSWORD=lucidos \
            -e POSTGRES_DB=lucidos \
            -p "$PG_PORT:5432" \
            "$PG_IMAGE" \
            postgres -c max_connections=500 >/dev/null
    fi

    echo -n "[test-db] waiting for Postgres"
    for _ in $(seq 1 30); do
        if docker exec "$PG_CONTAINER" pg_isready -U lucidos >/dev/null 2>&1; then
            echo " ready"
            return 0
        fi
        echo -n "."
        sleep 1
    done
    echo ""
    echo "ERROR: test Postgres did not become ready in 30s" >&2
    docker logs --tail 30 "$PG_CONTAINER" >&2 || true
    exit 1
}

ensure_test_pg

export TEST_DATABASE_URL="postgres://lucidos:lucidos@localhost:$PG_PORT/postgres"
echo "[test-db] TEST_DATABASE_URL=postgres://lucidos:lucidos@localhost:$PG_PORT/postgres"

cd "$PROJECT_DIR"

# Safe array expansion: under `set -u`, "${arr[@]}" on an empty array is an
# "unbound variable" error in bash 3.2 (macOS default). The +-form expands to
# nothing when empty and to the quoted elements otherwise.
if [ "$FULL" = "1" ]; then
    echo "[test] cargo test -p lucidos-engine (full crate)"
    cargo test -p lucidos-engine ${PASSTHRU[@]+"${PASSTHRU[@]}"}
else
    echo "[test] cargo test -p lucidos-engine --lib"
    cargo test -p lucidos-engine --lib ${PASSTHRU[@]+"${PASSTHRU[@]}"}
fi
