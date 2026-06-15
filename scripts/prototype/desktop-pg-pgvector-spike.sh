#!/usr/bin/env bash
#
# desktop-pg-pgvector-spike.sh — feasibility spike for the "One-Click Install"
# (see CLAUDE.md § One-Click Install).
#
# Proves the single riskiest claim of a no-Docker desktop bundle: that a
# RELOCATABLE PostgreSQL (one we ship inside an .app/.msi/AppImage, not a
# system/Docker install) can run pgvector and serve ANN queries over TCP — the
# exact shape the engine needs (it only wants a DATABASE_URL over TCP).
#
# What it does, end to end:
#   1. Downloads a relocatable PostgreSQL build (theseus-rs/postgresql-binaries —
#      the same tarballs the Rust `postgresql_embedded` crate fetches, so this is
#      the real "embed PG in a Rust app" path).
#   2. Compiles pgvector against that relocated PG via PGXS.
#   3. initdb's a throwaway cluster at an arbitrary path, boots the server on a
#      TCP port, runs L2 / cosine / inner-product queries, and builds an HNSW
#      index — verifying the planner actually uses it.
#   4. Tears the server down.
#
# Prints PASS/FAIL. Re-runnable: downloads + build are cached in the work dir.
#
# ---------------------------------------------------------------------------
# FINDINGS (macOS arm64, 2026-06-14) — read before wiring this into a build:
#
# * The theseus relocatable tarball is a COMPLETE Postgres: server + initdb +
#   pg_ctl + psql + pg_config + server headers + PGXS. ~12 MB download. So
#   extensions can be compiled against it; nothing is stripped.
#
# * GOTCHA: the tarball bakes its BUILD machine's Xcode SDK path into PGXS
#   (`CPPFLAGS/LDFLAGS = -isysroot $(PG_SYSROOT)` → a MacOSX<ver>.sdk under some
#   CI Xcode_*.app that does not exist on your machine). Building an extension
#   against it therefore fails with "'stdio.h' file not found" until you
#   override PG_SYSROOT to a local SDK (`xcrun --show-sdk-path`). This only
#   affects the *build* step — irrelevant to end users.
#
# * SHIPPING MODEL (what this spike implies, NOT what it does): you do NOT
#   compile pgvector on the end user's machine. You compile it ONCE PER PLATFORM
#   in CI and ship the artifacts — `vector.<dylib|so|dll>`, `vector.control`,
#   `vector*.sql` — dropped into the bundled PG's `lib/` + `share/extension/`.
#   The end user's app just runs `CREATE EXTENSION vector`. The PG_SYSROOT dance
#   below is the CI build recipe, per platform.
#
# * CROSS-PLATFORM (not exercised here — this spike is macOS):
#     - Linux:   theseus asset `...-<arch>-unknown-linux-gnu.tar.gz`; build
#                pgvector with system gcc, no sysroot dance. (Docker already
#                does this via the `postgresql-17-pgvector` apt package.)
#     - Windows: theseus asset `...-x86_64-pc-windows-msvc.zip`; pgvector builds
#                with MSVC `nmake /F Makefile.win`, or ship a prebuilt DLL.
#
# * RECOMMENDED NEXT SPIKE: point the engine's migrations at a cluster booted by
#   this script (export TEST_DATABASE_URL=postgres://$USER@127.0.0.1:$PORT/...)
#   and run them, to prove the engine's own pgvector migration applies on the
#   bundled PG.
# ---------------------------------------------------------------------------

set -euo pipefail

PG_VERSION="${PG_VERSION:-17.10.0}"          # match production (Dockerfile ships PG 17)
PGVECTOR_VERSION="${PGVECTOR_VERSION:-0.8.2}"
PORT="${PORT:-54329}"
WORKDIR="${WORKDIR:-/tmp/lucidos-pg-pgvector-spike}"

# --- resolve the relocatable-binary target triple for this host -------------
case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-gnu" ;;
  *) echo "unsupported OS $(uname -s) — this spike covers macOS/Linux"; exit 2 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64)        arch="x86_64" ;;
  *) echo "unsupported arch $(uname -m)"; exit 2 ;;
esac
TRIPLE="${arch}-${os}"

PG_DIRNAME="postgresql-${PG_VERSION}-${TRIPLE}"
PG_TARBALL="${PG_DIRNAME}.tar.gz"
PG_URL="https://github.com/theseus-rs/postgresql-binaries/releases/download/${PG_VERSION}/${PG_TARBALL}"
PGVECTOR_URL="https://github.com/pgvector/pgvector/archive/refs/tags/v${PGVECTOR_VERSION}.tar.gz"

PREFIX="${WORKDIR}/${PG_DIRNAME}"
BIN="${PREFIX}/bin"
PGCONFIG="${BIN}/pg_config"
DATA="${WORKDIR}/data"
SOCK="${WORKDIR}/sock"
LOG="${WORKDIR}/pg.log"
USER_NAME="$(whoami)"

mkdir -p "$WORKDIR"
cd "$WORKDIR"

echo "=== spike: relocatable PG ${PG_VERSION} + pgvector ${PGVECTOR_VERSION} (${TRIPLE}) ==="

# --- 1. relocatable PostgreSQL ---------------------------------------------
if [ ! -x "$PGCONFIG" ]; then
  echo "--- downloading $PG_URL"
  curl -fsSL -m 300 -o "$PG_TARBALL" "$PG_URL"
  tar -xzf "$PG_TARBALL"
fi
[ -x "$PGCONFIG" ] || { echo "FAIL: pg_config missing after extract"; exit 1; }
echo "--- PostgreSQL: $("$PGCONFIG" --version)"

# --- 2. compile pgvector against the relocated PG (PGXS) --------------------
SHAREDIR="$("$PGCONFIG" --sharedir)"
if [ ! -f "${SHAREDIR}/extension/vector.control" ]; then
  echo "--- downloading + building pgvector"
  curl -fsSL -m 180 -o "pgvector-${PGVECTOR_VERSION}.tar.gz" "$PGVECTOR_URL"
  rm -rf pgvector && mkdir -p pgvector
  tar -xzf "pgvector-${PGVECTOR_VERSION}.tar.gz" -C pgvector --strip-components=1

  MAKE_ARGS=(PG_CONFIG="$PGCONFIG")
  if [ "$os" = "apple-darwin" ]; then
    # Override the build machine's baked-in SDK path (see FINDINGS).
    MAKE_ARGS+=(PG_SYSROOT="$(xcrun --show-sdk-path)")
  fi
  ( cd pgvector && make -s clean "${MAKE_ARGS[@]}" >/dev/null 2>&1 || true
    make -s "${MAKE_ARGS[@]}"
    make -s install "${MAKE_ARGS[@]}" )
fi
[ -f "${SHAREDIR}/extension/vector.control" ] || { echo "FAIL: pgvector did not install"; exit 1; }
echo "--- pgvector installed into bundled PG: ${SHAREDIR}/extension/vector.control"

# --- 3. boot the cluster and prove pgvector over TCP -----------------------
export DYLD_LIBRARY_PATH="${PREFIX}/lib"   # macOS: find libpq next to the binaries
export LD_LIBRARY_PATH="${PREFIX}/lib"     # Linux equivalent
rm -rf "$DATA" "$SOCK"; mkdir -p "$SOCK"

echo "--- initdb (fresh cluster, trust auth)"
"$BIN/initdb" -D "$DATA" -U "$USER_NAME" -A trust >/dev/null

echo "--- starting server on 127.0.0.1:${PORT}"
"$BIN/pg_ctl" -D "$DATA" -l "$LOG" \
  -o "-p ${PORT} -k ${SOCK} -c listen_addresses=127.0.0.1" -w start
trap '"$BIN/pg_ctl" -D "$DATA" -m fast stop >/dev/null 2>&1 || true' EXIT

PSQL() { "$BIN/psql" -h 127.0.0.1 -p "$PORT" -U "$USER_NAME" -d postgres -tAX -c "$1"; }

echo "--- $(PSQL 'select version();')"
PSQL "CREATE EXTENSION vector;" >/dev/null
echo "--- extension active: vector $(PSQL "SELECT extversion FROM pg_extension WHERE extname='vector';")"

PSQL "CREATE TABLE items (id int, embedding vector(3));" >/dev/null
PSQL "INSERT INTO items VALUES (1,'[1,1,1]'),(2,'[9,9,9]'),(3,'[1,2,1]'),(4,'[-3,0,2]');" >/dev/null

echo "--- nearest neighbours to [1,1,1] (L2 <->):"
NN="$(PSQL "SELECT id FROM items ORDER BY embedding <-> '[1,1,1]' LIMIT 2;" | paste -sd, -)"
echo "    order = ${NN}"
[ "$NN" = "1,3" ] || { echo "FAIL: unexpected NN order (want 1,3)"; exit 1; }

echo "--- distance operators present (<->, <=>, <#>):"
PSQL "SELECT round((embedding <-> '[1,1,1]')::numeric,3) l2,
             round((embedding <=> '[1,1,1]')::numeric,4) cos,
             round((embedding <#> '[1,1,1]')::numeric,3) ip
      FROM items WHERE id=3;" | sed 's/^/    id=3 -> /'

echo "--- HNSW index (the real ANN path):"
PSQL "CREATE INDEX ON items USING hnsw (embedding vector_l2_ops);" >/dev/null
PLAN="$(PSQL "SET enable_seqscan=off;
              EXPLAIN (COSTS off) SELECT id FROM items ORDER BY embedding <-> '[1,1,1]' LIMIT 1;")"
echo "$PLAN" | sed 's/^/    /'
echo "$PLAN" | grep -q "Index Scan using" || { echo "FAIL: planner did not use the HNSW index"; exit 1; }

echo
echo "=== PASS: relocatable PG ${PG_VERSION} + pgvector ${PGVECTOR_VERSION}, TCP, HNSW — no Docker, no system PG ==="
