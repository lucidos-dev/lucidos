#!/usr/bin/env bash
# pg_ready.sh: wait until a Postgres container serves real clients.
#
# On a first run the image's entrypoint starts a temporary server that listens
# on the unix socket only, then stops it and starts the real one. A socket probe
# answers yes during that window, and the next psql hits "the database system is
# shutting down". Probing over TCP tells the two apart, since the temporary
# server never listens there.

# wait_pg_ready <container> <user> [tries]: probe once a second, over TCP.
# Prints dots as it waits. Returns 1 when the server never answers.
wait_pg_ready() {
    local container="$1" user="$2" tries="${3:-30}" _
    for _ in $(seq 1 "$tries"); do
        if docker exec "$container" pg_isready -U "$user" -d postgres -h 127.0.0.1 -p 5432 >/dev/null 2>&1; then
            return 0
        fi
        echo -n "."
        sleep 1
    done
    return 1
}
