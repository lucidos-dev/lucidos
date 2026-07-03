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
[ "$(stage_runtime_theseus_os Darwin)" = "apple-darwin" ]     && pass "Darwin -> apple-darwin"     || fail "Darwin wrong"
[ "$(stage_runtime_theseus_os Linux)" = "unknown-linux-gnu" ] && pass "Linux -> unknown-linux-gnu" || fail "Linux wrong"
if stage_runtime_theseus_os Windows 2>/dev/null; then fail "Windows should be unsupported"; else pass "unsupported OS rejected"; fi

echo ""
echo "test: stage_runtime_arch maps uname -m"
[ "$(stage_runtime_arch arm64)" = "aarch64" ]   && pass "arm64 -> aarch64"   || fail "arm64 wrong"
[ "$(stage_runtime_arch aarch64)" = "aarch64" ] && pass "aarch64 -> aarch64" || fail "aarch64 wrong"
[ "$(stage_runtime_arch x86_64)" = "x86_64" ]   && pass "x86_64 -> x86_64"   || fail "x86_64 wrong"
[ "$(stage_runtime_arch amd64)" = "x86_64" ]    && pass "amd64 -> x86_64"    || fail "amd64 wrong"
if stage_runtime_arch riscv64 2>/dev/null; then fail "riscv64 should be unsupported"; else pass "unsupported arch rejected"; fi

echo ""
echo "test: stage_runtime_triple builds <arch>-<os>"
[ "$(stage_runtime_triple Linux x86_64)" = "x86_64-unknown-linux-gnu" ]   && pass "Linux/x86_64 -> x86_64-unknown-linux-gnu"   || fail "linux x86_64 triple wrong"
[ "$(stage_runtime_triple Linux aarch64)" = "aarch64-unknown-linux-gnu" ] && pass "Linux/aarch64 -> aarch64-unknown-linux-gnu" || fail "linux aarch64 triple wrong"
[ "$(stage_runtime_triple Darwin arm64)" = "aarch64-apple-darwin" ]       && pass "Darwin/arm64 -> aarch64-apple-darwin"       || fail "darwin arm64 triple wrong"
[ "$(stage_runtime_triple Darwin x86_64)" = "x86_64-apple-darwin" ]       && pass "Darwin/x86_64 -> x86_64-apple-darwin"       || fail "darwin x86_64 triple wrong"

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
[ "$got" = "$want" ] && pass "linux x86_64 PG url is the theseus asset" || fail "linux PG url wrong: $got"
want_mac="https://github.com/theseus-rs/postgresql-binaries/releases/download/18.4.0/postgresql-18.4.0-aarch64-apple-darwin.tar.gz"
got_mac="$(stage_runtime_pg_url 18.4.0 aarch64-apple-darwin)"
[ "$got_mac" = "$want_mac" ] && pass "macOS arm64 PG url unchanged from the step-1 recipe" || fail "macOS PG url wrong: $got_mac"

echo ""
echo "test: stage_runtime_pgvector_url targets the version tag tarball"
want_v="https://github.com/pgvector/pgvector/archive/refs/tags/v0.8.2.tar.gz"
got_v="$(stage_runtime_pgvector_url 0.8.2)"
[ "$got_v" = "$want_v" ] && pass "pgvector url = $got_v" || fail "pgvector url wrong: $got_v"

echo ""
echo "test: the macOS PG_SYSROOT dance is keyed on a Darwin host only"
if stage_runtime_needs_macos_sysroot Darwin; then pass "Darwin needs the sysroot override"; else fail "Darwin should need the override"; fi
if stage_runtime_needs_macos_sysroot Linux; then fail "Linux must NOT use the sysroot override"; else pass "Linux skips the override (system gcc)"; fi

# ── assemble: the 6-resource staging tree ─────────────────────────────────────
# A fake set of already-built inputs: three "binaries" (engine, gateway, the
# `lucidos` CLI), two static dirs, and a relocatable-PG-like nested tree with a
# symlink.
new_inputs() {
    local dir; dir="$(mktemp -d)"
    printf 'engine\n'  > "$dir/engine";  chmod +x "$dir/engine"
    printf 'gateway\n' > "$dir/gateway"; chmod +x "$dir/gateway"
    printf 'cli\n'     > "$dir/cli";     chmod +x "$dir/cli"
    mkdir -p "$dir/frontend" && printf '<html>\n' > "$dir/frontend/index.html"
    mkdir -p "$dir/sdk"      && printf 'sdk\n'     > "$dir/sdk/sdk.js"
    mkdir -p "$dir/pg/bin" "$dir/pg/lib"
    printf 'postgres\n' > "$dir/pg/bin/postgres"; chmod +x "$dir/pg/bin/postgres"
    printf 'libpq\n'    > "$dir/pg/lib/libpq.so.5"
    ln -s libpq.so.5 "$dir/pg/lib/libpq.so"
    printf '%s' "$dir"
}

echo ""
echo "test: stage_runtime_assemble lays down exactly the 6 RESOURCE_NAMES"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
OUT="$(stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk")"; rc=$?
if [ $rc -eq 0 ] && [ "$OUT" = "$STAGE" ]; then pass "assemble exits 0 and prints the stage dir"; else fail "assemble failed (rc=$rc, out=$OUT)"; fi
for n in lucidos-engine lucidos-gateway lucidos frontend postgres sdk; do
    [ -e "$STAGE/$n" ] && pass "staged $n" || fail "missing $n in stage"
done
[ -x "$STAGE/lucidos-engine" ] && pass "lucidos-engine is executable" || fail "lucidos-engine not executable"
[ -x "$STAGE/lucidos-gateway" ] && pass "lucidos-gateway is executable" || fail "lucidos-gateway not executable"
[ -x "$STAGE/lucidos" ] && pass "lucidos CLI is executable" || fail "lucidos CLI not executable"
[ -x "$STAGE/postgres/bin/postgres" ] && pass "nested postgres/bin/postgres present + executable" || fail "nested postgres binary missing"
[ -L "$STAGE/postgres/lib/libpq.so" ] && pass "PG symlink preserved" || fail "PG symlink not preserved"
# No extra top-level entries beyond the 6 resources.
count="$(find "$STAGE" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')"
[ "$count" = "6" ] && pass "stage has exactly 6 top-level entries" || fail "stage has $count top-level entries (want 6)"

echo ""
echo "test: stage_runtime_assemble re-stages cleanly (removes stale entries)"
printf 'stale\n' > "$STAGE/stale-file"
stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" >/dev/null
[ -e "$STAGE/stale-file" ] && fail "stale file survived a re-stage" || pass "stale file removed on re-stage"
rm -rf "$IN" "$STAGE"

echo ""
echo "test: stage_runtime_assemble refuses a missing input"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
rm -rf "$IN/sdk"
out="$(stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "sdk dist not found"; then pass "missing sdk input refused"; else fail "expected missing-sdk refusal (rc=$rc): $out"; fi
[ -d "$STAGE" ] && fail "stage dir should not exist after a refusal" || pass "no stage written on refusal"
rm -rf "$IN"

echo ""
echo "test: stage_runtime_assemble refuses a missing lucidos CLI binary"
IN="$(new_inputs)"; STAGE="$(mktemp -d)/stage"
rm -f "$IN/cli"
out="$(stage_runtime_assemble "$STAGE" "$IN/engine" "$IN/gateway" "$IN/cli" "$IN/frontend" "$IN/pg" "$IN/sdk" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "lucidos CLI binary not found"; then pass "missing lucidos CLI refused"; else fail "expected missing-cli refusal (rc=$rc): $out"; fi
[ -d "$STAGE" ] && fail "stage dir should not exist after a refusal" || pass "no stage written on refusal"
rm -rf "$IN"

echo ""
echo "test: stage_runtime_build_binaries requires at least one package"
out="$(stage_runtime_build_binaries /tmp/nope 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "at least one package"; then pass "empty package list refused"; else fail "expected empty-package refusal (rc=$rc): $out"; fi

echo ""
echo "stage_runtime: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
