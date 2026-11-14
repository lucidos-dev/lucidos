#!/bin/bash
# Tests for scripts/lib/pg_ready.sh. `docker` and `sleep` are shadowed, so no
# host call happens.
#
# Run: ./scripts/lib/pg_ready_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/pg_ready.sh
source "$SCRIPT_DIR/pg_ready.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

sleep() { :; }
TCP_FAILS_LEFT=0
TCP_CALLS=0
# The temporary init server answers on the socket and never on TCP. The real
# server answers on TCP once TCP_FAILS_LEFT probes have failed.
docker() {
    case " $* " in
        *" -h 127.0.0.1 "*)
            TCP_CALLS=$((TCP_CALLS + 1))
            if [ "$TCP_FAILS_LEFT" -gt 0 ]; then
                TCP_FAILS_LEFT=$((TCP_FAILS_LEFT - 1))
                return 1
            fi
            return 0
            ;;
        *) return 0 ;;
    esac
}

TCP_FAILS_LEFT=3
TCP_CALLS=0
if wait_pg_ready c u 10 >/dev/null; then pass "returns once the real server answers"; else fail "returns once the real server answers"; fi
if [ "$TCP_CALLS" -eq 4 ]; then pass "ignores a socket-only server (4 TCP probes)"; else fail "ignores a socket-only server (saw $TCP_CALLS TCP probes)"; fi

TCP_FAILS_LEFT=99
if wait_pg_ready c u 3 >/dev/null; then fail "gives up when TCP never answers"; else pass "gives up when TCP never answers"; fi

echo "pg_ready_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
