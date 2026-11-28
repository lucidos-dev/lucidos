#!/usr/bin/env bash
# loopback_curl.sh: the shell twin of `lucidos_local_token::client`, the one
# way a script reaches a gateway's loopback listener (decision 12,
# docs/plans/2026-11-19-desktop-window-https.md).
#
# Every gateway loopback listener serves TLS from the loopback CA. So a call
# trusts that CA alone, never `-k`, and has no http fallback: plaintext
# loopback gets the outbox hand-off or a 301, and `curl -f` passes a 301.
# A directly launched engine serves plain http and stays outside this file.
#
# crates/lucidos-local-token pins the file name and the origin below.

# shellcheck source=scripts/lib/storage_root.sh
source "$(dirname "${BASH_SOURCE[0]}")/storage_root.sh"

LUCIDOS_LOOPBACK_CA_FILE="loopback-ca.pem"

# loopback_ca_cert: the loopback CA's public copy. It sits beside a relocated
# private subtree, else in the Lucidos home (loopback_cert::public_cert_path).
loopback_ca_cert() {
    if [ -n "${LUCIDOS_PRIVATE_DIR:-}" ]; then
        printf '%s/%s\n' "$(dirname "$LUCIDOS_PRIVATE_DIR")" "$LUCIDOS_LOOPBACK_CA_FILE"
        return 0
    fi
    printf '%s/%s\n' "$(lucidos_home_dir)" "$LUCIDOS_LOOPBACK_CA_FILE"
}

# loopback_origin <port>: the origin of a gateway loopback listener.
loopback_origin() {
    printf 'https://127.0.0.1:%s\n' "$1"
}

# loopback_curl <curl args...>: curl that trusts only the loopback CA and never
# routes through an exported proxy. Follows no redirect, like the Rust client.
loopback_curl() {
    curl --noproxy '*' --cacert "$(loopback_ca_cert)" "$@"
}
