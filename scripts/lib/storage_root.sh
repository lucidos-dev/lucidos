#!/usr/bin/env bash
# storage_root.sh: the storage-root rule for the dev scripts, the shell twin of
# the lucidos-home crate (ADR 0456).
#
# A storage root is a directory Lucidos stores data in, kept at mode 0700 so
# another local user cannot traverse into it (docs/glossary.md § Storage root).
#
# crates/lucidos-home pins the names below and the body of ensure_storage_root,
# here and in install.sh's copy.

LUCIDOS_HOME_DIR_NAME=".lucidos"

# lucidos_home_dir: $HOME/.lucidos, without touching the disk.
lucidos_home_dir() {
    printf '%s/%s\n' "$HOME" "$LUCIDOS_HOME_DIR_NAME"
}

# gateway_data_dir: $LUCIDOS_GATEWAY_DATA, else ~/.lucidos/gateway.
gateway_data_dir() {
    printf '%s\n' "${LUCIDOS_GATEWAY_DATA:-$(lucidos_home_dir)/gateway}"
}

# ensure_storage_root <dir>: create <dir> and any missing parents owner-only,
# then set <dir> to 0700 unconditionally, so an older root converges.
ensure_storage_root() {
    (umask 077 && mkdir -p "$1") && chmod 700 "$1"
}
