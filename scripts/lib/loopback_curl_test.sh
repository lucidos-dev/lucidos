#!/bin/bash
# Tests for scripts/lib/loopback_curl.sh, the shell loopback client.
#
# Run: ./scripts/lib/loopback_curl_test.sh   (no harness; direct)
#
# Every case runs under a fake HOME and a stubbed curl, so nothing here reads
# this machine's CA or opens a connection.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=loopback_curl.sh
source "$SCRIPT_DIR/loopback_curl.sh"
unset LUCIDOS_PRIVATE_DIR

PASS=0
FAIL=0
assert_eq() {
    if [ "$2" = "$3" ]; then
        echo "  ok:   $1"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $1 (want '$3', got '$2')"
        FAIL=$((FAIL + 1))
    fi
}

echo "loopback_curl: the CA copy follows the private subtree"
assert_eq "the Lucidos home by default" \
    "$(HOME=/home/u loopback_ca_cert)" "/home/u/.lucidos/loopback-ca.pem"
assert_eq "beside a relocated private subtree" \
    "$(HOME=/home/u LUCIDOS_PRIVATE_DIR=/tmp/e2e/private loopback_ca_cert)" "/tmp/e2e/loopback-ca.pem"
assert_eq "a trailing slash names the same parent" \
    "$(HOME=/home/u LUCIDOS_PRIVATE_DIR=/tmp/e2e/private/ loopback_ca_cert)" "/tmp/e2e/loopback-ca.pem"
assert_eq "an empty override is no override" \
    "$(HOME=/home/u LUCIDOS_PRIVATE_DIR='' loopback_ca_cert)" "/home/u/.lucidos/loopback-ca.pem"

echo "loopback_curl: the origin is https on 127.0.0.1"
assert_eq "origin" "$(loopback_origin 5251)" "https://127.0.0.1:5251"

echo "loopback_curl: curl trusts only the CA and skips every proxy"
# shellcheck disable=SC2317 # called by loopback_curl
curl() { printf '%s\n' "$*"; }
assert_eq "argv" "$(HOME=/home/u loopback_curl -fs https://127.0.0.1:5251/~/api/v1/health)" \
    "--noproxy * --cacert /home/u/.lucidos/loopback-ca.pem -fs https://127.0.0.1:5251/~/api/v1/health"
unset -f curl

echo ""
echo "loopback_curl: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
