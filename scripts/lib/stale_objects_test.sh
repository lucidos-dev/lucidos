#!/bin/bash
# Tests prune_stale_debug_objects (scripts/lib/stale_objects.sh) and its call
# from build_or_find_engine (scripts/lib/workspace.sh).
#
# Hermetic: every file it creates or deletes lives in a mktemp directory, `nm`
# is a shell function reading a fixture table, and the build test stubs the
# compile, publish and sign steps.
#
# Run: ./scripts/lib/stale_objects_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=stale_objects.sh
source "$SCRIPT_DIR/stale_objects.sh"

SCRATCH="$(mktemp -d)"
trap 'chmod -R u+rwx "$SCRATCH" 2>/dev/null; rm -rf "$SCRATCH"' EXIT
NOW="$(date +%s)"
HOUR=3600

# Sets a file's mtime the given number of seconds in the past.
make_aged() {
    local path="$1" age_s="$2" stamp
    stamp="$(date -r "$((NOW - age_s))" +%Y%m%d%H%M.%S 2>/dev/null ||
        date -d "@$((NOW - age_s))" +%Y%m%d%H%M.%S)"
    mkdir -p "$(dirname "$path")"
    touch -t "$stamp" "$path"
}

# One generation: two codegen units of an artifact, all of one mtime.
make_generation() {
    local deps="$1" artifact="$2" generation="$3" age_s="$4" cgu
    for cgu in 0aaa 1bbb; do
        make_aged "$deps/$artifact.$cgu.$generation.rcgu.o" "$age_s"
    done
}

present() { [ -e "$1/$2" ]; }

# Which generation each fixture binary names in its OSO stabs, as `nm -ap`
# prints them: `<binary basename> <generation>` per line.
NM_TABLE="$SCRATCH/nm-table"
# shellcheck disable=SC2329 # called by prune_stale_debug_objects
nm() {
    local binary="${!#}" artifact generation
    artifact="$(basename "$binary")"
    generation="$(awk -v a="$artifact" '$1 == a { print $2 }' "$NM_TABLE")"
    [ -n "$generation" ] || return 1
    printf '0000000000000000 - 00 0001   OSO %s/%s.0aaa.%s.rcgu.o\n' \
        "$(dirname "$binary")" "$artifact" "$generation"
    printf '0000000000000000 - 00 0001   OSO /x/libdep-7777777777777777.rlib(dep.0aaa.g.rcgu.o)\n'
}

# Whether a process holds cargo's build lock open, as `lsof -t` reports it.
LOCK_HELD="$SCRATCH/lock-held"
# shellcheck disable=SC2329 # called by prune_stale_debug_objects
lsof() { [ -e "$LOCK_HELD" ] && echo 4242; }

DEPS="$SCRATCH/deps"
make_generation "$DEPS" app-1111111111111111 gen1 $((5 * HOUR))
make_generation "$DEPS" app-1111111111111111 gen2 $((4 * HOUR))
make_generation "$DEPS" app-1111111111111111 gen3 $((3 * HOUR))
make_generation "$DEPS" app-1111111111111111 gen4 $((2 * HOUR))
make_generation "$DEPS" tst-2222222222222222 only $((9 * HOUR))
make_generation "$DEPS" tie-6666666666666666 tie1 $((6 * HOUR))
make_generation "$DEPS" tie-6666666666666666 tie2 $((6 * HOUR))
make_generation "$DEPS" tie-6666666666666666 tie3 $((6 * HOUR))
for other in app-1111111111111111 tie-6666666666666666 \
    libfoo-4444444444444444.rlib libbar-5555555555555555.dylib \
    app-1111111111111111.d weird.rcgu.o -dash-8888888888888888.0aaa.g.rcgu.o \
    app-1111111111111111.lib.abc-cgu.0.rcgu.o; do
    make_aged "$DEPS/$other" $((9 * HOUR))
done
make_aged "$DEPS/sub/app-1111111111111111.0aaa.gen1.rcgu.o" $((9 * HOUR))
printf '%s\n' "app-1111111111111111 gen2" "tie-6666666666666666 tie1" > "$NM_TABLE"

OUT="$(prune_stale_debug_objects "$DEPS")"
RC=$?

echo "test: the prune succeeds and says what it removed"
if [ "$RC" -eq 0 ] && [ "$OUT" = "Pruned 6 stale debug objects from $DEPS." ]; then
    pass "$OUT"
else
    fail "exit $RC, output: $OUT"
fi

echo "test: the generation the binary links stays, even when it is not the newest"
if present "$DEPS" app-1111111111111111.0aaa.gen2.rcgu.o &&
    present "$DEPS" app-1111111111111111.1bbb.gen2.rcgu.o; then
    pass "gen2, named by the binary's OSO stabs, kept"
else
    fail "the linked generation is gone: $(ls "$DEPS")"
fi

echo "test: the newest other generation stays beside it"
if present "$DEPS" app-1111111111111111.0aaa.gen4.rcgu.o &&
    present "$DEPS" app-1111111111111111.1bbb.gen4.rcgu.o; then
    pass "gen4 kept"
else
    fail "gen4 is gone: $(ls "$DEPS")"
fi

echo "test: the rest of a crowded artifact goes"
if present "$DEPS" app-1111111111111111.0aaa.gen1.rcgu.o ||
    present "$DEPS" app-1111111111111111.0aaa.gen3.rcgu.o ||
    present "$DEPS" app-1111111111111111.1bbb.gen3.rcgu.o; then
    fail "gen1 or gen3 survived: $(ls "$DEPS")"
else
    pass "gen1 and gen3 deleted"
fi

echo "test: among generations tied on mtime, the linked one stays"
tied_kept=0
for g in tie2 tie3; do
    present "$DEPS" "tie-6666666666666666.0aaa.$g.rcgu.o" && tied_kept=$((tied_kept + 1))
done
if present "$DEPS" tie-6666666666666666.0aaa.tie1.rcgu.o && [ "$tied_kept" -eq 1 ]; then
    pass "tie1 kept, plus one of the other two"
else
    fail "tie1 present: $(present "$DEPS" tie-6666666666666666.0aaa.tie1.rcgu.o && echo yes || echo no), others kept: $tied_kept"
fi

echo "test: an artifact's only generation stays, however old"
if present "$DEPS" tst-2222222222222222.0aaa.only.rcgu.o; then
    pass "single old generation kept"
else
    fail "the only generation of a test binary was deleted"
fi

echo "test: nothing but generation-shaped objects directly in deps is touched"
missing=""
for other in app-1111111111111111 tie-6666666666666666 \
    libfoo-4444444444444444.rlib libbar-5555555555555555.dylib \
    app-1111111111111111.d weird.rcgu.o -dash-8888888888888888.0aaa.g.rcgu.o \
    app-1111111111111111.lib.abc-cgu.0.rcgu.o \
    sub/app-1111111111111111.0aaa.gen1.rcgu.o; do
    present "$DEPS" "$other" || missing="$missing $other"
done
if [ -z "$missing" ]; then
    pass "binaries, rlibs, dylibs, dep files, odd names and subdirectories kept"
else
    fail "deleted:$missing"
fi

echo "test: a cargo holding the build lock defers the whole prune"
BUSY="$SCRATCH/profile/deps"
make_generation "$BUSY" app-1111111111111111 gen1 $((5 * HOUR))
make_generation "$BUSY" app-1111111111111111 gen2 $((4 * HOUR))
make_generation "$BUSY" app-1111111111111111 gen3 $((3 * HOUR))
touch "$SCRATCH/profile/.cargo-lock" "$LOCK_HELD"
OUT="$(prune_stale_debug_objects "$BUSY")"
if [ "$OUT" = "Stale debug objects: another cargo is building here; left for the next build." ] &&
    present "$BUSY" app-1111111111111111.0aaa.gen1.rcgu.o; then
    pass "nothing deleted while another cargo builds"
else
    fail "output: $OUT; left: $(ls "$BUSY")"
fi

echo "test: with no lsof to ask, the prune waits and says why"
mkdir -p "$SCRATCH/empty-path"
OUT="$(unset -f lsof; PATH="$SCRATCH/empty-path" prune_stale_debug_objects "$BUSY")"
if [ "$OUT" = "Stale debug objects: no lsof to tell whether another cargo is building here; left for the next build." ] &&
    present "$BUSY" app-1111111111111111.0aaa.gen1.rcgu.o; then
    pass "nothing deleted when the lock cannot be read"
else
    fail "output: $OUT; left: $(ls "$BUSY")"
fi

echo "test: a build lock nobody holds lets the prune run"
rm -f "$LOCK_HELD"
OUT="$(prune_stale_debug_objects "$BUSY")"
if [ "$OUT" = "Pruned 2 stale debug objects from $BUSY." ] &&
    ! present "$BUSY" app-1111111111111111.0aaa.gen1.rcgu.o; then
    pass "$OUT"
else
    fail "output: $OUT; left: $(ls "$BUSY")"
fi

echo "test: a missing deps directory is not an error"
OUT="$(prune_stale_debug_objects "$SCRATCH/absent")"
RC=$?
if [ "$RC" -eq 0 ] && [ -n "$OUT" ]; then
    pass "$OUT"
else
    fail "exit $RC, output: $OUT"
fi

echo "test: an unreadable deps directory never fails a set -e caller"
LOCKED="$SCRATCH/locked"
make_generation "$LOCKED" app-1111111111111111 gen1 $((5 * HOUR))
chmod 000 "$LOCKED"
OUT="$(set -e; prune_stale_debug_objects "$LOCKED"; echo "carried on")"
RC=$?
chmod 700 "$LOCKED"
case "$OUT" in
    *WARNING*"carried on") pass "warned and carried on" ;;
    *) fail "exit $RC, output: $OUT" ;;
esac

echo "test: every engine build prunes after compiling and before publishing"
ORDER="$SCRATCH/order.log"
: > "$ORDER"
STATE="$SCRATCH/state"
echo stale > "$STATE"
CHECKOUT="$SCRATCH/checkout"
mkdir -p "$CHECKOUT/target/debug/deps"
# shellcheck disable=SC2329 # the stubs are called by build_or_find_engine
(
    set -e
    unset RELEASE
    # shellcheck source=workspace.sh
    source "$SCRIPT_DIR/workspace.sh"
    lsof() { return 1; }
    run_engine_cargo_build() { echo build >> "$ORDER"; }
    prune_stale_debug_objects() { echo "prune $1" >> "$ORDER"; }
    publish_launch_binaries() { echo publish >> "$ORDER"; }
    # Stale once, so the build takes its rebuild-once path too.
    published_build_state() { cat "$STATE"; echo current > "$STATE"; }
    sign_engine_binary() { :; }
    BUILD=1
    PROJECT_DIR="$CHECKOUT"
    build_or_find_engine
) > "$SCRATCH/build.out" 2>&1
BUILD_RC=$?
EXPECTED="build
prune $CHECKOUT/target/debug/deps
publish
build
prune $CHECKOUT/target/debug/deps
publish"
if [ "$BUILD_RC" -eq 0 ] && [ "$(cat "$ORDER")" = "$EXPECTED" ]; then
    pass "build, prune, publish on both the first build and the rebuild"
else
    fail "exit $BUILD_RC, order: $(tr '\n' ';' < "$ORDER") output: $(cat "$SCRATCH/build.out")"
fi

echo ""
echo "stale_objects_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
