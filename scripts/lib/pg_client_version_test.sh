#!/bin/bash
# Tests for scripts/lib/pg_client_version.sh. Pure string logic, no host calls.
#
# Run: ./scripts/lib/pg_client_version_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/pg_client_version.sh
source "$SCRIPT_DIR/pg_client_version.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

assert_eq() { # <expected> <actual> <label>
    if [ "$1" = "$2" ]; then pass "$3"; else fail "$3 (expected '$1', got '$2')"; fi
}
assert_covers() { # <client> <server> <label>
    if pg_client_covers_server "$1" "$2"; then pass "$3"; else fail "$3"; fi
}
assert_not_covers() { # <client> <server> <label>
    if pg_client_covers_server "$1" "$2"; then fail "$3"; else pass "$3"; fi
}

assert_eq 18 "$(pg_major 'pg_dump (PostgreSQL) 18.4')" "reads a pg_dump --version line"
assert_eq 16 "$(pg_major 'pg_dump (PostgreSQL) 16.13 (Ubuntu 16.13-0ubuntu0.24.04.1)')" "reads a distro build"
assert_eq 18 "$(pg_major '18.4 (Debian 18.4-1.pgdg13+1)')" "reads a server_version"
assert_eq 18 "$(pg_major '18')" "reads a bare major"
assert_eq "" "$(pg_major 'nothing')" "reads no version from text without one"

assert_covers 'pg_dump (PostgreSQL) 18.4' '18.4 (Debian)' "equal majors cover"
assert_covers 'pg_dump (PostgreSQL) 19.0' '18.4' "a newer client covers"
assert_not_covers 'pg_dump (PostgreSQL) 16.13 (Ubuntu)' '18.4 (Debian)' "an older client does not cover"
assert_not_covers '' '18.4' "an unreadable client does not cover"

echo "pass: $PASS fail: $FAIL"
[ "$FAIL" -eq 0 ]
