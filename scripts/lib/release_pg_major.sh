#!/usr/bin/env bash
# release_pg_major.sh: the one pin on the bundled Postgres major.
#
# Every installed embedded cluster's pgdata is on this major. The gateway
# refuses a data dir from any other major and leaves it untouched, because no
# cross-major path (pg_upgrade or dump and restore) exists. A bundle on another
# major would therefore lock every user out of every workspace. So both build
# paths call release_pg_major_guard before they build, and
# release_pg_major_test.sh fails if the pin moves.
#
# Raise BUNDLED_PG_MAJOR only in the change that ships a cross-major path
# (ADR 0626).

BUNDLED_PG_MAJOR=18

# release_pg_major_guard <pg-version>: succeed only when the version's major is
# BUNDLED_PG_MAJOR. Prints the refusal on stderr.
release_pg_major_guard() {
    local version="$1" major="${1%%.*}"
    [ "$major" = "$BUNDLED_PG_MAJOR" ] && return 0
    echo "ERROR: PG_VERSION '$version' is not Postgres $BUNDLED_PG_MAJOR, the major every installed pgdata is on." >&2
    echo "       Lucidos has no cross-major path, so the gateway would refuse every existing workspace." >&2
    echo "       Ship a cross-major path first, then raise BUNDLED_PG_MAJOR in scripts/lib/release_pg_major.sh." >&2
    return 1
}
