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
LUCIDOS_PRIVATE_DIR_NAME="private"

# lucidos_home_dir: $HOME/.lucidos, without touching the disk.
lucidos_home_dir() {
    printf '%s/%s\n' "$HOME" "$LUCIDOS_HOME_DIR_NAME"
}

# lucidos_private_dir: the private subtree, ~/.lucidos/private (ADR 0645), or
# $LUCIDOS_PRIVATE_DIR when set (lucidos_home::private::relocated).
lucidos_private_dir() {
    if [ -n "${LUCIDOS_PRIVATE_DIR:-}" ]; then
        printf '%s\n' "$LUCIDOS_PRIVATE_DIR"
        return 0
    fi
    printf '%s/%s\n' "$(lucidos_home_dir)" "$LUCIDOS_PRIVATE_DIR_NAME"
}

# lucidos_private_entry_in <lucidos-home> <name>: where the private entry
# <name> lives, the twin of lucidos_home::private::entry_in. A home an older
# release used keeps its entries in place, so an older binary and this one
# meet one copy (docs/temporary-measures.md). Reads the disk, writes nothing.
# scripts/lib/service.sh carries a byte-identical copy, pinned by
# crates/lucidos-home.
lucidos_private_entry_in() {
    local home="$1" name="$2" legacy
    local names="network.toml network.toml.lock paired-devices.json webhook-token run secret-keys snapshots gateway local-token hook-socket-token pg-shared-password .local-token.lock .hook-socket-token.lock .pg-shared-password.lock"
    case " $names " in
        *" $name "*) ;;
        *) printf '%s/private/%s\n' "$home" "$name"; return 0 ;;
    esac
    if [ -e "$home/$name" ] || [ -L "$home/$name" ]; then
        printf '%s/%s\n' "$home" "$name"; return 0
    fi
    if [ -e "$home/private/$name" ] || [ -L "$home/private/$name" ]; then
        printf '%s/private/%s\n' "$home" "$name"; return 0
    fi
    for legacy in $names; do
        if [ -e "$home/$legacy" ] || [ -L "$home/$legacy" ]; then
            printf '%s/%s\n' "$home" "$name"; return 0
        fi
    done
    printf '%s/private/%s\n' "$home" "$name"
}

# lucidos_private_entry <name>: inside $LUCIDOS_PRIVATE_DIR when set, with no
# older path considered, else lucidos_private_entry_in for this HOME.
lucidos_private_entry() {
    if [ -n "${LUCIDOS_PRIVATE_DIR:-}" ]; then
        printf '%s/%s\n' "$LUCIDOS_PRIVATE_DIR" "$1"
        return 0
    fi
    lucidos_private_entry_in "$(lucidos_home_dir)" "$1"
}

# gateway_data_dir: $LUCIDOS_GATEWAY_DATA, else the private entry gateway.
gateway_data_dir() {
    printf '%s\n' "${LUCIDOS_GATEWAY_DATA:-$(lucidos_private_entry gateway)}"
}

# ensure_storage_root <dir>: create <dir> and any missing parents owner-only,
# then set <dir> to 0700 unconditionally, so an older root converges.
ensure_storage_root() {
    (umask 077 && mkdir -p "$1") && chmod 700 "$1"
}
