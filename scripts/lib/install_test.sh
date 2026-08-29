#!/usr/bin/env bash
# Tests for install.sh (step 3 of docs/plans/2026-06-30-installer-step3-download-and-run.md)
# and its pure helper lib scripts/lib/install_common.sh. Two halves:
#   • PURE helpers — sourced + asserted directly (triple→URL, version, runtime dir),
#     the same way stage_runtime_test.sh / headless_tarball_test.sh do.
#   • INTEGRATION — install.sh invoked as a subprocess in --no-launch mode against a
#     LOCALLY built tarball (--from-tarball) and a file:// "release" dir, so the FULL
#     download path (resolve → curl file:// → verify → extract) runs with NO network.
# Run: ./scripts/lib/install_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
INSTALL="$PROJECT_DIR/install.sh"

# Sandbox env-as-flag inputs a live dev workspace exports (its engine carries
# LUCIDOS_TLS_CERT/KEY for https serving) so they can't leak into the installer
# subprocesses under test.
unset LUCIDOS_TLS_CERT LUCIDOS_TLS_KEY LUCIDOS_BIND

# Pure libs (release_staging for sha256, used by headless_tarball_emit to build the
# fake artifact; the rest are what install.sh + install_common share).
# shellcheck source=scripts/lib/release_staging.sh
source "$SCRIPT_DIR/release_staging.sh"
# shellcheck source=scripts/lib/headless_tarball.sh
source "$SCRIPT_DIR/headless_tarball.sh"
# shellcheck source=scripts/lib/stage_runtime.sh
source "$SCRIPT_DIR/stage_runtime.sh"
# shellcheck source=scripts/lib/install_common.sh
source "$SCRIPT_DIR/install_common.sh"

PASS=0
FAIL=0
fail() { echo "  FAIL: $*"; FAIL=$((FAIL+1)); }
pass() { echo "  ok:   $*"; PASS=$((PASS+1)); }

VERSION="0.14.0"
TRIPLE="$(stage_runtime_host_triple)"   # the host triple — the artifact the download path resolves
STEM="lucidos-$VERSION-$TRIPLE"
NAMES=(lucidos-engine lucidos-gateway lucidos frontend postgres sdk system-knowhow)

# A fake runtime tree shaped like the staged 7-resource bundle (three "binaries",
# three static dirs, a relocatable-PG-like nested tree) — same shape as
# headless_tarball_test.sh / stage_runtime_test.sh.
new_resources() {
    local dir; dir="$(mktemp -d)"
    # Executable SCRIPT fakes, not bare text: finish_install runs the extracted
    # gateway once (`lucidos-gateway --build-id`, the execution smoke that catches
    # a too-old glibc at install time), so the fake must execute and answer —
    # same shape as service_test.sh's fakes.
    printf '#!/bin/sh\necho engine\n'        > "$dir/lucidos-engine"
    printf '#!/bin/sh\necho fake-build-id\n' > "$dir/lucidos-gateway"
    printf '#!/bin/sh\necho cli\n'           > "$dir/lucidos"
    chmod +x "$dir/lucidos-engine" "$dir/lucidos-gateway" "$dir/lucidos"
    mkdir -p "$dir/frontend" && printf '<html>\n' > "$dir/frontend/index.html"
    mkdir -p "$dir/sdk"      && printf 'sdk\n'     > "$dir/sdk/sdk.js"
    mkdir -p "$dir/system-knowhow" && printf '# glossary\n' > "$dir/system-knowhow/glossary.md"
    mkdir -p "$dir/postgres/bin" "$dir/postgres/lib"
    printf 'postgres\n' > "$dir/postgres/bin/postgres"; chmod +x "$dir/postgres/bin/postgres"
    printf 'libpq\n'    > "$dir/postgres/lib/libpq.5"
    printf '%s' "$dir"
}

# Build lucidos-<version>-<triple>.tar.gz + .sha256 into a fresh dir; echo the dir.
new_release_dir() {
    local res out
    res="$(new_resources)"
    out="$(mktemp -d)"
    headless_tarball_emit "$res" "$out" "$VERSION" "$TRIPLE" "${NAMES[@]}" >/dev/null \
        || { echo "ERROR: could not build fake tarball" >&2; return 1; }
    rm -rf "$res"
    printf '%s' "$out"
}

# ── PURE: URL + version + dir helpers (no divergent mapping) ──────────────────
echo "test: install_default_base_url targets the GitHub Releases path for v<version>"
got="$(install_default_base_url "$VERSION")"
want="https://github.com/lucidos-dev/lucidos/releases/download/v$VERSION"
if [ "$got" = "$want" ]; then pass "default base url = $got"; else fail "base url wrong: $got"; fi

echo ""
echo "test: install_tarball_url = <base>/<stem>.tar.gz and matches headless_tarball_stem"
base="https://example.test/dl"
got="$(install_tarball_url "$base" "$VERSION" "$TRIPLE")"
want="$base/$(headless_tarball_stem "$VERSION" "$TRIPLE").tar.gz"
if [ "$got" = "$want" ]; then pass "tarball url = $got"; else fail "tarball url wrong: $got (want $want)"; fi
# A trailing slash on the base must not double up.
got2="$(install_tarball_url "$base/" "$VERSION" "$TRIPLE")"
if [ "$got2" = "$want" ]; then pass "trailing slash on base tolerated"; else fail "trailing slash not handled: $got2"; fi

echo ""
echo "test: install_checksum_url is the tarball url + .sha256"
got="$(install_checksum_url "$base" "$VERSION" "$TRIPLE")"
want="$(install_tarball_url "$base" "$VERSION" "$TRIPLE").sha256"
if [ "$got" = "$want" ]; then pass "checksum url = $got"; else fail "checksum url wrong: $got"; fi

echo ""
echo "test: install_resolve_version precedence (override > RELEASE file > default)"
if [ "$(install_resolve_version "9.9.9" "" "0.0.0")" = "9.9.9" ]; then pass "override wins"; else fail "override ignored"; fi
rel="$(mktemp)"; printf '0.77.0\n' > "$rel"
if [ "$(install_resolve_version "" "$rel" "0.0.0")" = "0.77.0" ]; then pass "RELEASE file used + trimmed"; else fail "RELEASE file ignored"; fi
if [ "$(install_resolve_version "" "/no/such/RELEASE" "0.0.0")" = "0.0.0" ]; then pass "falls back to default"; else fail "default fallback wrong"; fi
if [ "$(install_resolve_version "1.2.3" "$rel" "0.0.0")" = "1.2.3" ]; then pass "override beats RELEASE file"; else fail "override should beat RELEASE"; fi
rm -f "$rel"

echo ""
echo "test: install.sh's baked LUCIDOS_DEFAULT_VERSION equals the repo-root RELEASE"
# The DEFAULT branch above is the one the PUBLIC one-liner takes: piped through sh
# there is no checkout, so no RELEASE file sits next to the script and every
# `curl … | sh` install downloads whatever this constant says. Drifted, it 404s
# (0.14.0 shipped no headless tarball) rather than installing something merely old.
# release.sh rewrites the line when it bumps RELEASE; this catches a hand-edit — or
# a removed substitution — that pulls the two apart.
baked="$(sed -n 's/^LUCIDOS_DEFAULT_VERSION="\([^"]*\)".*/\1/p' "$INSTALL" | head -1)"
release_version="$(tr -d '[:space:]' < "$PROJECT_DIR/RELEASE" 2>/dev/null)"
if [ -z "$baked" ]; then
    fail "install.sh has no ^LUCIDOS_DEFAULT_VERSION=\"…\" assignment to parse"
elif [ "$baked" = "$release_version" ]; then
    pass "baked default = RELEASE = $baked"
else
    fail "install.sh LUCIDOS_DEFAULT_VERSION='$baked' != RELEASE '$release_version' — a piped 'curl | sh' install would fetch the wrong version"
fi

echo ""
echo "test: the dash re-exec pins the version it was piped with"
# REGRESSION (clean-machine smoke, 2026-07-28): on Debian/Ubuntu /bin/sh is dash,
# so `curl … | sh` re-fetches the installer from LUCIDOS_INSTALL_URL and execs
# THAT under bash. The re-fetched copy used to re-resolve its own baked default,
# so a user who piped lucidos.dev's current installer actually installed whatever
# older version happened to be baked into github main. The guard must carry its
# resolved version across the re-exec. Exercised against install.sh's REAL guard
# text, extracted verbatim, with curl+bash stubbed out.
GUARD_DIR="$(mktemp -d)"
# Extract from the baked-version constant through the guard's closing `fi`.
awk '/^LUCIDOS_DEFAULT_VERSION=/{on=1} on{print} on && /^fi$/{exit}' "$INSTALL" > "$GUARD_DIR/guard.sh"
if ! grep -q 'exec bash -c' "$GUARD_DIR/guard.sh"; then
    fail "could not extract the re-exec guard from install.sh (shape changed)"
else
    cat > "$GUARD_DIR/curl" <<'SH'
#!/bin/sh
# Stand in for the re-fetch: the payload just reports what it inherited. The
# leading shebang is load-bearing, not decoration: the guard sniffs for it
# before exec'ing (a soft-404 origin returns its landing page at status 200),
# so a stub without one is refused exactly like HTML would be.
printf '#!/usr/bin/env bash\necho "SEEN=${LUCIDOS_VERSION:-unset}"\n'
SH
    chmod +x "$GUARD_DIR/curl"
    baked_now="$(sed -n 's/^LUCIDOS_DEFAULT_VERSION="\([^"]*\)".*/\1/p' "$INSTALL" | head -1)"
    # `unset BASH_VERSION` + sourcing with $0 not a readable file = the piped-dash path.
    out="$(PATH="$GUARD_DIR:$PATH" sh -c 'unset BASH_VERSION; . '"$GUARD_DIR"'/guard.sh' 2>&1 || true)"
    if [ "$out" = "SEEN=$baked_now" ]; then
        pass "piped re-exec carries the version across ($out)"
    else
        fail "piped re-exec did not pin the version: got '$out', want 'SEEN=$baked_now'"
    fi
    # An explicit LUCIDOS_VERSION must still win over the baked default.
    out="$(PATH="$GUARD_DIR:$PATH" LUCIDOS_VERSION=1.2.3 sh -c 'unset BASH_VERSION; . '"$GUARD_DIR"'/guard.sh' 2>&1 || true)"
    if [ "$out" = "SEEN=1.2.3" ]; then
        pass "explicit LUCIDOS_VERSION still beats the baked default"
    else
        fail "explicit LUCIDOS_VERSION lost across the re-exec: $out"
    fi
    # Soft-404 on the installer ITSELF. The lib sniff cannot help here: this runs
    # before a single lib is fetched, and the payload goes to `exec bash -c`.
    cat > "$GUARD_DIR/curl" <<'SH'
#!/bin/sh
printf '<!DOCTYPE html>\n<html><head><title>Lucidos</title></head></html>\n'
SH
    chmod +x "$GUARD_DIR/curl"
    out="$(PATH="$GUARD_DIR:$PATH" sh -c 'unset BASH_VERSION; . '"$GUARD_DIR"'/guard.sh' 2>&1 || true)"
    if echo "$out" | grep -q "did not return a shell script" \
       && ! echo "$out" | grep -qiE "syntax error|unexpected token"; then
        pass "an HTML re-fetch is refused before it reaches exec bash -c"
    else
        fail "expected the re-exec guard to refuse an HTML payload: $out"
    fi
fi
rm -rf "$GUARD_DIR"

echo ""
echo "test: install_runtime_dir = <prefix>/runtime/<stem>"
got="$(install_runtime_dir "/opt/x" "$VERSION" "$TRIPLE")"
if [ "$got" = "/opt/x/runtime/$STEM" ]; then pass "runtime dir = $got"; else fail "runtime dir wrong: $got"; fi

echo ""
echo "test: install_ca_bundle_candidates lists the bundles rustls-native-certs reads"
got="$(install_ca_bundle_candidates)"
if echo "$got" | grep -qx '/etc/ssl/certs/ca-certificates.crt'; then pass "Debian/Ubuntu bundle listed"; else fail "Debian bundle missing: $got"; fi
if echo "$got" | grep -qx '/etc/pki/tls/certs/ca-bundle.crt'; then pass "Fedora/RHEL bundle listed"; else fail "RHEL bundle missing: $got"; fi

# ── INTEGRATION: --help + flag parsing ───────────────────────────────────────
echo ""
echo "test: --help documents the modes, flags, and env contract"
out="$(bash "$INSTALL" --help 2>&1)"
for token in --dev --source --from-tarball --version --base-url --prefix --force --no-launch \
             LUCIDOS_RELEASE_BASE_URL LUCIDOS_VERSION LUCIDOS_FORCE LUCIDOS_PREFIX; do
    if echo "$out" | grep -q -- "$token"; then pass "--help documents $token"; else fail "--help missing $token"; fi
done

echo ""
echo "test: unknown argument is rejected with a clear message"
out="$(bash "$INSTALL" --bogus 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "unknown argument"; then
    pass "unknown argument refused"
else
    fail "expected unknown-argument refusal (rc=$rc): $out"
fi

echo ""
echo "test: flags that need a value reject a missing one"
for flag in --from-tarball --version --base-url --prefix --port; do
    out="$(bash "$INSTALL" "$flag" 2>&1)"; rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -q "requires"; then
        pass "$flag without a value is refused"
    else
        fail "expected '$flag requires …' (rc=$rc): $out"
    fi
done

echo ""
echo "test: --from-tarball with a missing file fails clearly"
out="$(bash "$INSTALL" --from-tarball /no/such/file.tar.gz --no-launch 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "not found"; then
    pass "missing --from-tarball file refused"
else
    fail "expected missing-file refusal (rc=$rc): $out"
fi

# ── INTEGRATION: --from-tarball install (checksum pass) ───────────────────────
echo ""
echo "test: --from-tarball extracts the 6 resources and verifies the sidecar"
REL="$(new_release_dir)"; TARBALL="$REL/$STEM.tar.gz"
PREFIX="$(mktemp -d)"
out="$(bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
RUNTIME="$PREFIX/runtime/$STEM"
if [ $rc -eq 0 ]; then pass "--from-tarball exits 0"; else fail "--from-tarball failed (rc=$rc): $out"; fi
if echo "$out" | grep -qi "checksum verified"; then pass "checksum verified on the local path"; else fail "no checksum-verified line: $out"; fi
for n in "${NAMES[@]}"; do
    if [ -e "$RUNTIME/$n" ]; then pass "extracted $n"; else fail "missing $n under $RUNTIME"; fi
done
if [ -x "$RUNTIME/lucidos-gateway" ]; then pass "lucidos-gateway is executable"; else fail "gateway not executable"; fi
if [ -L "$PREFIX/runtime/current" ]; then pass "current symlink created"; else fail "current symlink missing"; fi

echo ""
echo "test: --no-launch did NOT start anything (prints how to start)"
if echo "$out" | grep -qi "installed"; then pass "prints the installed banner"; else fail "no installed banner: $out"; fi

# ── INTEGRATION: idempotency (skip unless --force) ───────────────────────────
echo ""
echo "test: a second --from-tarball run is idempotent (no re-extract); --force re-extracts"
touch "$RUNTIME/.keep"
out="$(bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ -e "$RUNTIME/.keep" ] && echo "$out" | grep -qi "already installed"; then
    pass "second run skipped extraction (marker survived)"
else
    fail "expected an idempotent skip (rc=$rc, marker present=$([ -e "$RUNTIME/.keep" ] && echo y || echo n)): $out"
fi
out="$(bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch --force 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ ! -e "$RUNTIME/.keep" ] && [ -x "$RUNTIME/lucidos-gateway" ]; then
    pass "--force re-extracted (marker gone, runtime intact)"
else
    fail "expected --force to re-extract (rc=$rc, marker present=$([ -e "$RUNTIME/.keep" ] && echo y || echo n)): $out"
fi
rm -rf "$PREFIX"

# ── INTEGRATION: --force never leaves the SHARED runtime missing ─────────────
echo ""
echo "test: a --force re-extract keeps the shared runtime present the whole way"
# Every registered instance's service runs <prefix>/runtime/current/<binary>, so
# the old code's "rm -rf then untar" left those binaries gone for the length of
# the extract and a KeepAlive respawn inside that window failed. The extract now
# stages beside the live tree and swaps. Two things to hold: the tree is never
# absent on the happy path, and a swap that fails restores the previous one.
PREFIX="$(mktemp -d)"
RUNTIME="$PREFIX/runtime/$STEM"
bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch >/dev/null 2>&1
printf 'old\n' > "$RUNTIME/.generation"
out="$(bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch --force 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ -x "$RUNTIME/lucidos-gateway" ] && [ ! -e "$RUNTIME/.generation" ]; then
    pass "--force swapped in a fresh runtime"
else
    fail "expected a clean swap (rc=$rc): $out"
fi
# No staging or rollback residue is left under runtime/.
leftovers="$(find "$PREFIX/runtime" -maxdepth 1 -name '.staging-*' -o -maxdepth 1 -name '.previous-*' 2>/dev/null)"
if [ -z "$leftovers" ]; then pass "left no .staging-/.previous- residue"; else fail "residue: $leftovers"; fi
rm -rf "$PREFIX"

echo ""
echo "test: a --force whose swap cannot complete restores the previous runtime"
# The rollback arm. Forcing the failure needs care: making the runtime parent
# read-only fails the EARLIER `mkdir -p "$staging"`, so the run dies before it
# has moved anything and the test passes without reaching the rollback at all.
# Shim `mv` instead, and fail only the first move whose destination is the live
# runtime dir. That is the staging swap; the rollback move that follows has the
# same destination, so the shim is one-shot and lets it through.
PREFIX="$(mktemp -d)"
RUNTIME="$PREFIX/runtime/$STEM"
bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch >/dev/null 2>&1
printf 'original\n' > "$RUNTIME/.generation"
MVBIN="$(mktemp -d)"; MVFIRED="$MVBIN/fired"
# Quoted heredoc, and the three values arrive as env vars rather than being
# baked in: nothing in the shim body may expand HERE, and taking them from the
# environment says so without a suppression.
cat > "$MVBIN/mv" <<'MV_SHIM'
#!/bin/sh
last=""
for a in "$@"; do last="$a"; done
if [ "$last" = "$SHIM_TARGET" ] && [ ! -f "$SHIM_FIRED" ]; then
    : > "$SHIM_FIRED"
    exit 1
fi
exec "$SHIM_REAL_MV" "$@"
MV_SHIM
chmod +x "$MVBIN/mv"
# Resolve the real `mv` BEFORE the shim is on PATH. Doing it in the assignment
# prefix resolves it through the prepended PATH, so the shim execs itself and
# the run hangs.
SHIM_REAL_MV="$(command -v mv)"; export SHIM_REAL_MV
out="$(PATH="$MVBIN:$PATH" SHIM_TARGET="$RUNTIME" SHIM_FIRED="$MVFIRED" \
        bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch --force 2>&1)"; rc=$?
# Assert the shim actually fired, or the two checks below prove nothing: that is
# how the first version of this test passed while never reaching the rollback.
if [ -f "$MVFIRED" ]; then pass "the swap really did fail (shim fired)"; else fail "the shim never fired, so the rollback was not exercised: $out"; fi
if [ $rc -ne 0 ]; then pass "a failed swap exits non-zero"; else fail "a failed swap reported success: $out"; fi
if [ -x "$RUNTIME/lucidos-gateway" ] && [ -e "$RUNTIME/.generation" ]; then
    pass "the previous runtime is restored for the running services"
else
    fail "the shared runtime went missing on a failed swap: $out"
fi
rm -rf "$PREFIX" "$MVBIN"

# ── INTEGRATION: --from-tarball tamper → fail closed ─────────────────────────
echo ""
echo "test: --from-tarball with a tampered tarball fails closed"
PREFIX="$(mktemp -d)"
printf 'tampered' >> "$TARBALL"          # sidecar no longer matches
out="$(bash "$INSTALL" --from-tarball "$TARBALL" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "checksum verification failed"; then
    pass "tampered local tarball refused"
else
    fail "expected a checksum failure (rc=$rc): $out"
fi
if [ -e "$PREFIX/runtime/$STEM" ]; then fail "runtime should not exist after a refusal"; else pass "nothing extracted on tamper"; fi
rm -rf "$REL" "$PREFIX"

# ── INTEGRATION: a MANY-ENTRY tarball installs cleanly (GNU-tar SIGPIPE guard) ─
echo ""
echo "test: --from-tarball installs a many-entry tarball cleanly"
# A real runtime tarball has thousands of PG files, so `tar -tzf | head -1` closes
# the pipe long before tar finishes. On GNU tar (Linux — a must-work download
# platform) tar then dies on SIGPIPE (exit 141), which under `set -euo pipefail`
# aborted the whole installer; the `|| true` guard on the stem line fixes it.
# (BSD tar on macOS handles the broken pipe gracefully, so this case can't fail
# here — but it IS the regression guard on a GNU-tar host, and a many-file install
# smoke everywhere.) ~6000 small files under postgres/share to overflow the pipe.
BIGRES="$(new_resources)"
mkdir -p "$BIGRES/postgres/share/extension"
for i in $(seq 1 6000); do : > "$BIGRES/postgres/share/extension/f$i.sql"; done
BIGOUT="$(mktemp -d)"
headless_tarball_emit "$BIGRES" "$BIGOUT" "$VERSION" "$TRIPLE" "${NAMES[@]}" >/dev/null
PREFIX="$(mktemp -d)"
out="$(bash "$INSTALL" --from-tarball "$BIGOUT/$STEM.tar.gz" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ -x "$PREFIX/runtime/$STEM/lucidos-gateway" ]; then
    pass "many-entry tarball installed (no SIGPIPE abort)"
else
    fail "expected a clean install of a many-entry tarball (rc=$rc): $out"
fi
rm -rf "$BIGRES" "$BIGOUT" "$PREFIX"

# ── INTEGRATION: execution smoke — a runtime that can't run here fails loud ───
echo ""
echo "test: a runtime that cannot execute on this machine fails loud at install time"
# Emulates the too-old-glibc / wrong-arch case: an executable file that is not a
# runnable program (no shebang, not a valid binary). finish_install's
# verify_runtime_executes must refuse with the portability message instead of
# registering a service that would crash-loop.
BROKENRES="$(new_resources)"
printf 'ELF-not-really\n' > "$BROKENRES/lucidos-gateway"
chmod +x "$BROKENRES/lucidos-gateway"
BROKENOUT="$(mktemp -d)"
headless_tarball_emit "$BROKENRES" "$BROKENOUT" "$VERSION" "$TRIPLE" "${NAMES[@]}" >/dev/null
PREFIX="$(mktemp -d)"
out="$(bash "$INSTALL" --from-tarball "$BROKENOUT/$STEM.tar.gz" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "do not run on this machine"; then
    pass "non-executing runtime refused with the portability message"
else
    fail "expected the execution-smoke refusal (rc=$rc): $out"
fi
rm -rf "$BROKENRES" "$BROKENOUT" "$PREFIX"

# ── INTEGRATION: full download path over file:// (offline) ───────────────────
echo ""
echo "test: default download path over file:// resolves, verifies, and extracts"
REL="$(new_release_dir)"
PREFIX="$(mktemp -d)"
out="$(LUCIDOS_RELEASE_BASE_URL="file://$REL" LUCIDOS_VERSION="$VERSION" \
        bash "$INSTALL" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
RUNTIME="$PREFIX/runtime/$STEM"
if [ $rc -eq 0 ] && [ -x "$RUNTIME/lucidos-gateway" ]; then
    pass "file:// download installed the runtime"
else
    fail "expected a file:// download install (rc=$rc): $out"
fi
if echo "$out" | grep -qi "checksum verified"; then pass "download path verified the checksum"; else fail "download did not verify: $out"; fi
rm -rf "$PREFIX"

echo ""
echo "test: download path fails closed on a tampered file:// artifact"
PREFIX="$(mktemp -d)"
printf 'tampered' >> "$REL/$STEM.tar.gz"
out="$(LUCIDOS_RELEASE_BASE_URL="file://$REL" LUCIDOS_VERSION="$VERSION" \
        bash "$INSTALL" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "checksum verification failed"; then
    pass "tampered download refused"
else
    fail "expected a download checksum failure (rc=$rc): $out"
fi
if [ -e "$PREFIX/runtime/$STEM" ]; then fail "runtime should not exist after a tampered download"; else pass "nothing extracted on tampered download"; fi
rm -rf "$REL" "$PREFIX"

echo ""
echo "test: a missing release (404) surfaces an actionable message pointing at --dev"
PREFIX="$(mktemp -d)"; EMPTY="$(mktemp -d)"
out="$(LUCIDOS_RELEASE_BASE_URL="file://$EMPTY" LUCIDOS_VERSION="$VERSION" \
        bash "$INSTALL" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -qi "download failed" && echo "$out" | grep -q -- "--dev"; then
    pass "missing release points the user at --dev / --from-tarball"
else
    fail "expected an actionable 404 message (rc=$rc): $out"
fi
rm -rf "$PREFIX" "$EMPTY"

# ── INTEGRATION: fetched helper libs — a soft-404 HTML payload is refused ─────
echo ""
echo "test: a fetched helper lib that is HTML is refused and never sourced"
# REGRESSION (clean-machine smoke, 2026-07-29): a fresh ubuntu:22.04 running
# `curl -fsSL lucidos.dev/install.sh | sh` died with
#   stage_runtime.sh: line 1: syntax error near unexpected token `newline'
#   stage_runtime.sh: line 1: `<!DOCTYPE html>'
# Cloudflare Pages answers any path it doesn't have with the landing page and a
# 200 status, so `curl -fsSL` SUCCEEDED and the non-empty check passed — and the
# installer sourced the landing page as a shell script. _source_libs must sniff
# the payload BEFORE `.` runs, and fail closed. The fetch branch is taken only
# when install.sh has no adjacent scripts/lib, so the installer is copied to a
# checkout-less dir and the "origin" is a file:// directory.
NOCHECKOUT="$(mktemp -d)"
cp "$INSTALL" "$NOCHECKOUT/install.sh"
# Every lib install.sh fetches when piped: LUCIDOS_LIBS + the lazily sourced
# service pair (source_service_lib).
LIBS_FETCHED="stage_runtime.sh headless_tarball.sh install_common.sh service.sh"
for shape in doctype indented-html; do
    LIBDIR="$(mktemp -d)"; PREFIX="$(mktemp -d)"
    # An SPA fallback serves the landing page for EVERY unknown path, so every
    # lib comes back as HTML — seeding just one would make the run die on the
    # next lib's honest 404 instead of on the payload under test.
    for f in $LIBS_FETCHED; do
        if [ "$shape" = doctype ]; then
            printf '<!DOCTYPE html>\n<html><head><title>Lucidos</title></head></html>\n' > "$LIBDIR/$f"
        else
            # Leading blank lines + indentation: the sniff looks at the first
            # NON-BLANK line, not literally line 1.
            printf '\n\n   <html lang="en">\n<body>not shell</body>\n' > "$LIBDIR/$f"
        fi
    done
    out="$(LUCIDOS_LIB_BASE_URL="file://$LIBDIR" LUCIDOS_VERSION="$VERSION" \
            bash "$NOCHECKOUT/install.sh" --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -q "is HTML, not shell" \
       && echo "$out" | grep -q "stage_runtime.sh" && echo "$out" | grep -qF "file://$LIBDIR"; then
        pass "$shape payload refused, naming the lib and the origin"
    else
        fail "expected an HTML refusal naming lib + origin for $shape (rc=$rc): $out"
    fi
    # Positive proof it never reached `.`: sourcing the landing page is what
    # produced the shell syntax error in the original failure.
    if echo "$out" | grep -qiE "syntax error|unexpected token"; then
        fail "the HTML lib was sourced anyway ($shape): $out"
    else
        pass "$shape payload never reached the shell"
    fi
    rm -rf "$LIBDIR" "$PREFIX"
done

echo ""
echo "test: real helper libs fetched over file:// still source and install"
# The other half of fail-closed: the sniff must not reject legitimate shell. This
# is also the only coverage of _source_libs' FETCH branch end to end (the piped
# `curl … | sh` path), driven offline over file://.
LIBDIR="$(mktemp -d)"; PREFIX="$(mktemp -d)"; REL="$(new_release_dir)"
for f in $LIBS_FETCHED; do
    cp "$SCRIPT_DIR/$f" "$LIBDIR/$f"
done
out="$(LUCIDOS_LIB_BASE_URL="file://$LIBDIR" LUCIDOS_RELEASE_BASE_URL="file://$REL" \
        LUCIDOS_VERSION="$VERSION" bash "$NOCHECKOUT/install.sh" \
        --prefix "$PREFIX" --no-launch 2>&1)"; rc=$?
if [ $rc -eq 0 ] && [ -x "$PREFIX/runtime/$STEM/lucidos-gateway" ]; then
    pass "fetched helper libs sourced; the piped-install path completed"
else
    fail "expected the fetch path to install cleanly (rc=$rc): $out"
fi
rm -rf "$LIBDIR" "$PREFIX" "$REL" "$NOCHECKOUT"

# ── uninstall_hint: never name a file the user does not have ─────────────────
echo ""
echo "test: uninstall_hint adapts to whether install.sh ran from a checkout"
# The success banner used to hardcode `./uninstall.sh --name <slug>`, which is a
# file that exists only in a checkout. The default audience pipes the installer
# and the runtime tarball ships no uninstaller, so that line named nothing on
# disk. Extracted and exercised directly (install.sh runs main at the bottom, so
# it cannot simply be sourced), against both self-dir outcomes.
HINT_DIR="$(mktemp -d)"
sed -n '/^installer_self_dir()/,/^}/p;/^uninstall_hint()/,/^}/p' "$INSTALL" > "$HINT_DIR/hint.sh"
if ! grep -q '^uninstall_hint()' "$HINT_DIR/hint.sh"; then
    fail "could not extract uninstall_hint from install.sh (shape changed)"
else
    # No sibling uninstall.sh in $HINT_DIR = the piped case.
    got="$(cd "$HINT_DIR" && bash -c '. ./hint.sh; LUCIDOS_REPO_URL=https://example.invalid/repo.git; uninstall_hint demo')"
    if [ "${got#uninstall.sh --name demo}" != "$got" ] && echo "$got" | grep -qF "https://example.invalid/repo"; then
        pass "no checkout: points at the repository copy ($got)"
    else
        fail "expected a downloadable-uninstaller hint, got: $got"
    fi
    : > "$HINT_DIR/uninstall.sh"
    got="$(cd "$HINT_DIR" && bash -c '. ./hint.sh; LUCIDOS_REPO_URL=https://example.invalid/repo.git; uninstall_hint demo')"
    if [ "$got" = "./uninstall.sh --name demo" ]; then
        pass "checkout: points at the sibling script ($got)"
    else
        fail "expected the ./uninstall.sh form from a checkout, got: $got"
    fi
fi
rm -rf "$HINT_DIR"

# ── INTEGRATION: the FETCHED UNINSTALLER gets the same soft-404 treatment ─────
echo ""
echo "test: a fetched uninstaller that is HTML is refused and never exec'd"
# The lib sniff above guards `.` (source). dispatch_uninstall guards the OTHER
# place unknown remote content reaches a shell: `exec bash -c "$payload"`. It was
# unguarded until the 2026-07-30 docs audit, and it was NOT hypothetical: the
# site publisher uploads install.sh and scripts/lib/*.sh but not uninstall.sh, so
# <origin>/uninstall.sh soft-404s and a piped `install.sh --uninstall` handed the
# landing page to bash. Same shape as the lib test: a checkout-less install.sh
# (so the sibling-uninstall.sh branch is skipped) against a file:// "origin".
NOCHECKOUT="$(mktemp -d)"
cp "$INSTALL" "$NOCHECKOUT/install.sh"
for flag in --uninstall --list; do
    UDIR="$(mktemp -d)"; PREFIX="$(mktemp -d)"
    printf '<!DOCTYPE html>\n<html><head><title>Lucidos</title></head></html>\n' > "$UDIR/uninstall.sh"
    out="$(LUCIDOS_UNINSTALL_URL="file://$UDIR/uninstall.sh" \
            bash "$NOCHECKOUT/install.sh" "$flag" --prefix "$PREFIX" 2>&1)"; rc=$?
    if [ $rc -ne 0 ] && echo "$out" | grep -q "did not return a shell script" \
       && echo "$out" | grep -qF "file://$UDIR/uninstall.sh"; then
        pass "$flag refuses an HTML uninstaller, naming the origin"
    else
        fail "expected an HTML refusal naming the origin for $flag (rc=$rc): $out"
    fi
    # Positive proof it never reached `exec bash -c`: that is what turns the
    # landing page into shell syntax errors.
    if echo "$out" | grep -qiE "syntax error|unexpected token"; then
        fail "the HTML uninstaller was exec'd anyway ($flag): $out"
    else
        pass "$flag payload never reached the shell"
    fi
    rm -rf "$UDIR" "$PREFIX"
done

echo ""
echo "test: the real uninstaller fetched over file:// is exec'd and runs"
# Fail-closed's other half: the sniff must not reject legitimate shell. --list on
# an empty prefix is read-only (it touches no service, no data dir), so this is
# the safe end-to-end proof that the fetch+exec path still works.
UDIR="$(mktemp -d)"; PREFIX="$(mktemp -d)"; LIBDIR="$(mktemp -d)"
cp "$PROJECT_DIR/uninstall.sh" "$UDIR/uninstall.sh"
cp "$SCRIPT_DIR/service.sh" "$SCRIPT_DIR/install_common.sh" "$LIBDIR/"
out="$(LUCIDOS_UNINSTALL_URL="file://$UDIR/uninstall.sh" LUCIDOS_LIB_BASE_URL="file://$LIBDIR" \
        bash "$NOCHECKOUT/install.sh" --list --prefix "$PREFIX" 2>&1)"; rc=$?
if [ $rc -eq 0 ] && echo "$out" | grep -qi "Lucidos uninstaller"; then
    pass "the fetched uninstaller was exec'd and produced its own banner"
else
    fail "expected the fetched uninstaller to run (rc=$rc): $out"
fi
rm -rf "$PREFIX" "$LIBDIR"

echo ""
echo "test: uninstall.sh refuses a soft-404 service.sh instead of sourcing it"
# uninstall.sh has its own lib fetch (source_service_lib), separate from
# install.sh's _source_libs, and it is the likeliest of all these sites to meet a
# soft-404: the standalone `curl … /uninstall.sh | sh` path derives its lib base
# from the same origin that already soft-404s uninstall.sh itself. Reached here
# through install.sh --list, which execs the fetched uninstaller.
PREFIX="$(mktemp -d)"; LIBDIR="$(mktemp -d)"
printf '<!DOCTYPE html>\n<html><head><title>Lucidos</title></head></html>\n' > "$LIBDIR/service.sh"
out="$(LUCIDOS_UNINSTALL_URL="file://$UDIR/uninstall.sh" LUCIDOS_LIB_BASE_URL="file://$LIBDIR" \
        bash "$NOCHECKOUT/install.sh" --list --prefix "$PREFIX" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "is not a shell script" \
   && ! echo "$out" | grep -qiE "syntax error|unexpected token"; then
    pass "an HTML service.sh is refused before it reaches the shell"
else
    fail "expected uninstall.sh to refuse an HTML service.sh (rc=$rc): $out"
fi
rm -rf "$UDIR" "$PREFIX" "$LIBDIR" "$NOCHECKOUT"

# ── INTEGRATION: --dev routing (no compile — just assert the branch is taken) ─
echo ""
echo "test: --dev routes to the source-build branch (not the download path)"
# DO NOT run a real source build — it would clone/compile/launch a stack and mutate
# the dev machine. Instead, shim `uname` to an unsupported OS so detect_platform —
# the FIRST step of the dev branch — dies immediately, BEFORE any toolchain/clone/
# launch side effect. The download branch resolves the triple a different way and
# never prints "mode=source", so this cleanly proves the dev branch was selected.
FAKEBIN="$(mktemp -d)"
cat > "$FAKEBIN/uname" <<'SH'
#!/bin/sh
echo "PlanNine"
SH
chmod +x "$FAKEBIN/uname"
out="$(PATH="$FAKEBIN:$PATH" bash "$INSTALL" --dev 2>&1 || true)"
if echo "$out" | grep -qi "mode=source" && echo "$out" | grep -qi "Unsupported OS"; then
    pass "--dev enters the source branch and stops at detect_platform (no build)"
else
    fail "--dev did not route to the source branch safely: $out"
fi
rm -rf "$FAKEBIN"

echo ""
echo "install: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
