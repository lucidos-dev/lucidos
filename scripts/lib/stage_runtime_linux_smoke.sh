#!/usr/bin/env bash
# Run inside a clean Linux container as a non-root user, without host PG libraries.
# Usage: bash scripts/lib/stage_runtime_linux_smoke.sh <pg-prefix> <scratch-dir>
set -euo pipefail

prefix="$1"
scratch="$2"
[ "$(id -u)" -ne 0 ] || { echo "ERROR: PostgreSQL smoke must run as a non-root user" >&2; exit 1; }
unset LD_LIBRARY_PATH LD_PRELOAD
mkdir -p "$scratch/relocated postgres"
pg="$scratch/relocated postgres"
tar -czf "$scratch/postgres.tar.gz" -C "$prefix" .
tar -xzf "$scratch/postgres.tar.gz" -C "$pg"
pg="$(cd "$pg" && pwd)"
data="$pg/smoke-data"

"$pg/bin/postgres" --version
ldd "$pg/bin/postgres" | tee "$scratch/ldd.log"
if grep -q 'not found' "$scratch/ldd.log"; then
    echo "ERROR: PostgreSQL cannot resolve its bundled libraries" >&2
    exit 1
fi
"$pg/bin/initdb" -D "$data" --locale=C --encoding=UTF8 --auth=trust > "$scratch/initdb.log"
trap '"$pg/bin/pg_ctl" -D "$data" -m immediate -w stop > /dev/null 2>&1 || true' EXIT
"$pg/bin/pg_ctl" -D "$data" -l "$scratch/postgres.log" -o "-k \"$data\" -h ''" -w start
"$pg/bin/psql" -h "$data" -U "$(id -un)" -d postgres -v ON_ERROR_STOP=1 <<'SQL'
CREATE EXTENSION vector;
CREATE EXTENSION xml2;
CREATE EXTENSION "uuid-ossp";
DO $$
BEGIN
    IF ('[1,2,3]'::vector <-> '[1,2,4]'::vector) != 1 THEN
        RAISE EXCEPTION 'vector distance failed';
    END IF;
    IF (xpath('/root/value/text()', '<root><value>bundled</value></root>'::xml))[1]::text != 'bundled' THEN
        RAISE EXCEPTION 'XML query failed';
    END IF;
    IF xpath_string('<root><value>bundled</value></root>', '/root/value') != 'bundled' THEN
        RAISE EXCEPTION 'xml2 extension query failed';
    END IF;
    IF uuid_generate_v4() IS NULL THEN
        RAISE EXCEPTION 'uuid extension query failed';
    END IF;
END $$;
SQL
"$pg/bin/pg_ctl" -D "$data" -m fast -w stop
trap - EXIT
printf 'PASS: relocated PostgreSQL, XML, xml2, UUID, and pgvector\n'
