#!/usr/bin/env bash
# Tests for scripts/lib/stage_runtime.sh — the shared staging helpers (step 2 of
# docs/plans/2026-06-30-installer-step2-linux-tarball.md). Pure shell over fake
# files, no network/cargo/npm/tauri, so the whole matrix runs offline.
# Run: ./scripts/lib/stage_runtime_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/stage_runtime.sh
source "$SCRIPT_DIR/stage_runtime.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

# ── triple / os / arch resolution ─────────────────────────────────────────────
echo "test: stage_runtime_theseus_os maps uname -s"
if [ "$(stage_runtime_theseus_os Darwin)" = "apple-darwin" ]; then pass "Darwin -> apple-darwin"; else fail "Darwin wrong"; fi
if [ "$(stage_runtime_theseus_os Linux)" = "unknown-linux-gnu" ]; then pass "Linux -> unknown-linux-gnu"; else fail "Linux wrong"; fi
if stage_runtime_theseus_os Windows 2>/dev/null; then fail "Windows should be unsupported"; else pass "unsupported OS rejected"; fi

echo ""
echo "test: stage_runtime_arch maps uname -m"
if [ "$(stage_runtime_arch arm64)" = "aarch64" ]; then pass "arm64 -> aarch64"; else fail "arm64 wrong"; fi
if [ "$(stage_runtime_arch aarch64)" = "aarch64" ]; then pass "aarch64 -> aarch64"; else fail "aarch64 wrong"; fi
if [ "$(stage_runtime_arch x86_64)" = "x86_64" ]; then pass "x86_64 -> x86_64"; else fail "x86_64 wrong"; fi
if [ "$(stage_runtime_arch amd64)" = "x86_64" ]; then pass "amd64 -> x86_64"; else fail "amd64 wrong"; fi
if stage_runtime_arch riscv64 2>/dev/null; then fail "riscv64 should be unsupported"; else pass "unsupported arch rejected"; fi

echo ""
echo "test: stage_runtime_triple builds <arch>-<os>"
if [ "$(stage_runtime_triple Linux x86_64)" = "x86_64-unknown-linux-gnu" ]; then pass "Linux/x86_64 -> x86_64-unknown-linux-gnu"; else fail "linux x86_64 triple wrong"; fi
if [ "$(stage_runtime_triple Linux aarch64)" = "aarch64-unknown-linux-gnu" ]; then pass "Linux/aarch64 -> aarch64-unknown-linux-gnu"; else fail "linux aarch64 triple wrong"; fi
if [ "$(stage_runtime_triple Darwin arm64)" = "aarch64-apple-darwin" ]; then pass "Darwin/arm64 -> aarch64-apple-darwin"; else fail "darwin arm64 triple wrong"; fi
if [ "$(stage_runtime_triple Darwin x86_64)" = "x86_64-apple-darwin" ]; then pass "Darwin/x86_64 -> x86_64-apple-darwin"; else fail "darwin x86_64 triple wrong"; fi

echo ""
echo "test: stage_runtime_host_triple resolves on this host"
HT="$(stage_runtime_host_triple)"; rc=$?
if [ $rc -eq 0 ] && [ -n "$HT" ]; then pass "host triple = $HT"; else fail "host triple resolution failed (rc=$rc)"; fi

# ── download URL builders (the cross-platform PG selection) ────────────────────
echo ""
echo "test: stage_runtime_pg_url targets the theseus release asset per triple"
# The must-work Linux case (parent plan binding decision): x86_64-unknown-linux-gnu.
want="https://github.com/theseus-rs/postgresql-binaries/releases/download/18.4.0/postgresql-18.4.0-x86_64-unknown-linux-gnu.tar.gz"
got="$(stage_runtime_pg_url 18.4.0 x86_64-unknown-linux-gnu)"
if [ "$got" = "$want" ]; then pass "linux x86_64 PG url is the theseus asset"; else fail "linux PG url wrong: $got"; fi
want_mac="https://github.com/theseus-rs/postgresql-binaries/releases/download/18.4.0/postgresql-18.4.0-aarch64-apple-darwin.tar.gz"
got_mac="$(stage_runtime_pg_url 18.4.0 aarch64-apple-darwin)"
if [ "$got_mac" = "$want_mac" ]; then pass "macOS arm64 PG url unchanged from the step-1 recipe"; else fail "macOS PG url wrong: $got_mac"; fi

echo ""
echo "test: stage_runtime_pgvector_url targets the version tag tarball"
want_v="https://github.com/pgvector/pgvector/archive/refs/tags/v0.8.2.tar.gz"
got_v="$(stage_runtime_pgvector_url 0.8.2)"
if [ "$got_v" = "$want_v" ]; then pass "pgvector url = $got_v"; else fail "pgvector url wrong: $got_v"; fi

echo ""
echo "test: the macOS PG_SYSROOT dance is keyed on a Darwin host only"
if stage_runtime_needs_macos_sysroot Darwin; then pass "Darwin needs the sysroot override"; else fail "Darwin should need the override"; fi
if stage_runtime_needs_macos_sysroot Linux; then fail "Linux must NOT use the sysroot override"; else pass "Linux skips the override (system gcc)"; fi

# ── assemble: the 7-resource staging tree ─────────────────────────────────────
# A fake set of already-built inputs: three "binaries" (engine, gateway, the
# `lucidos` CLI), three static dirs (frontend, sdk, system-knowhow), and a
# relocatable-PG-like nested tree with a symlink.
new_inputs() {
    local dir; dir="$(mktemp -d)"
    printf 'engine\n'  > "$dir/engine";  chmod +x "$dir/engine"
    printf 'gateway\n' > "$dir/gateway"; chmod +x "$dir/gateway"
    printf 'cli\n'     > "$dir/cli";     chmod +x "$dir/cli"
    mkdir -p "$dir/frontend" && printf '<html>\n' > "$dir/frontend/index.html"
    mkdir -p "$dir/sdk"      && printf 'sdk\n'     > "$dir/sdk/sdk.js"
    mkdir -p "$dir/system-knowhow" && printf '# glossary\n' > "$dir/system-knowhow/glossary.md"
    mkdir -p "$dir/pg/bin" "$dir/pg/lib"
    printf 'postgres\n' > "$dir/pg/bin/postgres"; chmod +x "$dir/pg/bin/postgres"
    printf 'libpq\n'    > "$dir/pg/lib/libpq.so.5"
    ln -s libpq.so.5 "$dir/pg/lib/libpq.so"
    printf '%s' "$dir"
}

echo ""
echo "test: stage_runtime_assemble lays down exactly the 7 RESOURCE_NAMES"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
OUT="$(stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow")"; rc=$?
if [ $rc -eq 0 ] && [ "$OUT" = "$STAGE" ]; then pass "assemble exits 0 and prints the stage dir"; else fail "assemble failed (rc=$rc, out=$OUT)"; fi
for n in lucidos-engine lucidos-gateway lucidos frontend postgres sdk system-knowhow; do
    if [ -e "$STAGE/$n" ]; then pass "staged $n"; else fail "missing $n in stage"; fi
done
if [ -x "$STAGE/lucidos-engine" ]; then pass "lucidos-engine is executable"; else fail "lucidos-engine not executable"; fi
if [ -x "$STAGE/lucidos-gateway" ]; then pass "lucidos-gateway is executable"; else fail "lucidos-gateway not executable"; fi
if [ -x "$STAGE/lucidos" ]; then pass "lucidos CLI is executable"; else fail "lucidos CLI not executable"; fi
if [ -x "$STAGE/postgres/bin/postgres" ]; then pass "nested postgres/bin/postgres present + executable"; else fail "nested postgres binary missing"; fi
if [ -L "$STAGE/postgres/lib/libpq.so" ]; then pass "PG symlink preserved"; else fail "PG symlink not preserved"; fi
if [ -f "$STAGE/system-knowhow/glossary.md" ]; then pass "system-knowhow contents copied"; else fail "system-knowhow contents missing"; fi
# No extra top-level entries beyond the 7 resources.
count="$(find "$STAGE" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')"
if [ "$count" = "7" ]; then pass "stage has exactly 7 top-level entries"; else fail "stage has $count top-level entries (want 7)"; fi

echo ""
echo "test: stage_runtime_assemble re-stages cleanly (removes stale entries)"
printf 'stale\n' > "$STAGE/stale-file"
stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" >/dev/null
if [ -e "$STAGE/stale-file" ]; then fail "stale file survived a re-stage"; else pass "stale file removed on re-stage"; fi
rm -rf "$IN" "$STAGE"

echo ""
echo "test: stage_runtime_assemble refuses a missing input"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
rm -rf "$IN/sdk"
out="$(stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "sdk dist not found"; then pass "missing sdk input refused"; else fail "expected missing-sdk refusal (rc=$rc): $out"; fi
if [ -d "$STAGE" ]; then fail "stage dir should not exist after a refusal"; else pass "no stage written on refusal"; fi
rm -rf "$IN"

echo ""
echo "test: stage_runtime_assemble refuses a missing lucidos CLI binary"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
rm -f "$IN/cli"
out="$(stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "lucidos CLI binary not found"; then pass "missing lucidos CLI refused"; else fail "expected missing-cli refusal (rc=$rc): $out"; fi
if [ -d "$STAGE" ]; then fail "stage dir should not exist after a refusal"; else pass "no stage written on refusal"; fi
rm -rf "$IN"

echo ""
echo "test: stage_runtime_assemble refuses a missing system-knowhow dir"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
rm -rf "$IN/system-knowhow"
out="$(stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "system-knowhow dir not found"; then pass "missing system-knowhow refused"; else fail "expected missing-system-knowhow refusal (rc=$rc): $out"; fi
if [ -d "$STAGE" ]; then fail "stage dir should not exist after a refusal"; else pass "no stage written on refusal"; fi
rm -rf "$IN"

# ── staged-knowhow freshness (the hand-run-build guard) ──────────────────────
# The stage survives between builds, so a `cargo tauri build` typed by hand can
# package a months-old copy. Clean must mean "absent OR identical", never
# "absent" alone: a developer who has never run build-dmg.sh has no stage at all
# and their build must not go red.

echo ""
echo "test: stage_runtime_staged_knowhow_fresh accepts an absent or knowhow-less stage"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
if stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>/dev/null; then pass "absent stage is clean"; else fail "absent stage should be clean"; fi
mkdir -p "$STAGE"
if stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>/dev/null; then pass "stage with no system-knowhow/ is clean"; else fail "knowhow-less stage should be clean"; fi
# ABSENT is the only thing the fast path waves through: a staged system-knowhow
# that is not a directory is drift, not "nothing to check".
printf 'not a directory\n' > "$STAGE/system-knowhow"
if stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>/dev/null; then fail "a non-directory staged system-knowhow must not read as clean"; else pass "non-directory staged copy caught"; fi
rm -rf "$IN" "$STAGE"

echo ""
echo "test: stage_runtime_staged_knowhow_fresh accepts a stage assemble just wrote"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" >/dev/null
if stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>/dev/null; then pass "freshly staged copy is clean"; else fail "a copy assemble just made must be clean"; fi

echo ""
echo "test: stage_runtime_staged_knowhow_fresh catches every shape of drift"
printf '# glossary v2\n' > "$IN/system-knowhow/glossary.md"
out="$(stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "drifted from the live tree"; then pass "changed file caught"; else fail "expected changed-file drift (rc=$rc): $out"; fi
if echo "$out" | grep -q "rm -rf"; then pass "drift message says how to fix it"; else fail "drift message must name the remedy: $out"; fi
stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" >/dev/null
printf '# new doc\n' > "$IN/system-knowhow/triggers.md"
if stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>/dev/null; then fail "a doc added since staging should be drift"; else pass "added file caught"; fi
stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" >/dev/null
rm -f "$IN/system-knowhow/triggers.md"
if stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>/dev/null; then fail "a doc deleted since staging should be drift"; else pass "removed file caught"; fi
rm -rf "$IN" "$STAGE"

echo ""
echo "test: stage_runtime_staged_knowhow_fresh refuses a missing live tree"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" "$IN/system-knowhow" >/dev/null
rm -rf "$IN/system-knowhow"
out="$(stage_runtime_staged_knowhow_fresh "$STAGE" "$IN/system-knowhow" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "system-knowhow dir not found"; then pass "missing live tree refused, never read as clean"; else fail "expected missing-live-tree refusal (rc=$rc): $out"; fi
rm -rf "$IN" "$STAGE"

echo ""
echo "test: stage_runtime_build_binaries requires at least one package"
out="$(stage_runtime_build_binaries /tmp/nope 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "at least one package"; then pass "empty package list refused"; else fail "expected empty-package refusal (rc=$rc): $out"; fi

echo ""
echo "stage_runtime: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
