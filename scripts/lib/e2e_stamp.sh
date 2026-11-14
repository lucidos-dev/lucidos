#!/bin/bash
# e2e_stamp.sh: the e2e harness signs its own device stamps.
#
# An engine trusts a device id only with a gateway stamp, signed by a key its
# starter wrote to its stdin. The e2e harness starts its engine directly, so it
# is the starter: it mints a key per engine and pipes it in, as the gateway
# does (crates/lucidos-gateway/src/stack.rs).
#
# The test processes start later and sign with the same key, so it also sits
# in a 0600 file in the disposable e2e workspace's .lucidos/. A production key
# never touches disk. This one signs only for a throwaway test workspace.
#
# Readers: harness_stamp_key in crates/lucidos-e2e/tests/api_support/mod.rs,
# stamped_device_headers_in in crates/lucidos-gateway/src/chain_tests.rs and
# crates/lucidos-app/e2e/harnessStamp.ts. The file holds two lines: the key in
# hex, then the workspace id the stamps name.
#
# Sourced by e2e.sh. Tested by e2e_stamp_test.sh.

# Where the harness keeps the key of the engine it started in <workspace>.
e2e_stamp_key_path() {
    printf '%s/.lucidos/e2e-stamp-key\n' "$1"
}

# The workspace id an engine binds its key to: the gateway's slug when one
# lists it, else the workspace directory's name. Mirrors
# `stamped_workspace_id` in crates/lucidos-engine/src/api/gateway_stamp.rs.
# Pure. $1 is LUCIDOS_WORKSPACE_ID (maybe empty), $2 the workspace path.
e2e_stamp_workspace_id() {
    local slug="$1" workspace="${2%/}"
    if [ -n "$slug" ]; then
        printf '%s\n' "$slug"
    else
        printf '%s\n' "${workspace##*/}"
    fi
}

# Write a fresh key for <workspace> to <path>, readable by its owner only.
# Replaces any earlier key atomically, so a reader never sees half a file.
mint_e2e_stamp_key() {
    local path="$1" workspace_id="$2" key tmp
    key="$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')"
    if [ "${#key}" -ne 64 ]; then
        echo "ERROR: could not read 32 random bytes for the e2e stamp key" >&2
        return 1
    fi
    mkdir -p "$(dirname "$path")" || return 1
    tmp="$path.tmp.$$"
    ( umask 077 && printf '%s\n%s\n' "$key" "$workspace_id" > "$tmp" ) || return 1
    mv -f "$tmp" "$path"
}
