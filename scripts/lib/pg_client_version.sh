#!/usr/bin/env bash
# pg_client_version.sh: tell a Postgres client older than its server, which
# pg_dump refuses to dump ("server version mismatch").
#
# The engine's preflight tests dump a database through the host's `pg_dump`, so
# the host client must be at least as new as the test container's server.

# pg_major <text>: the major version in a `pg_dump --version` line or a
# server_version string, such as "pg_dump (PostgreSQL) 18.4" or "18.4 (Debian)".
pg_major() {
    printf '%s\n' "$1" | sed -E -n 's/^[^0-9]*([0-9]+)[.( ].*$|^[^0-9]*([0-9]+)$/\1\2/p' | head -n 1
}

# pg_client_covers_server <client-text> <server-text>: succeeds when the client's
# major version is at least the server's. Fails when either is unreadable.
pg_client_covers_server() {
    local client server
    client="$(pg_major "$1")"
    server="$(pg_major "$2")"
    [ -n "$client" ] && [ -n "$server" ] && [ "$client" -ge "$server" ]
}
