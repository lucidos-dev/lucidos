#!/bin/bash
# Tests prune_stale_incremental (scripts/lib/stale_incremental.sh).
#
# Hermetic: every file it creates or deletes lives in a mktemp directory,
# `lsof` and `cargo` are shell functions, and no real process is signalled.
# The perl test holds a real flock on a scratch file, in a subshell.
#
# Run: ./scripts/lib/stale_incremental_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

# shellcheck source=stale_incremental.sh
source "$SCRIPT_DIR/stale_incremental.sh"

SCRATCH="$(mktemp -d)"
trap 'chmod -R u+rwx "$SCRATCH" 2>/dev/null; rm -rf "$SCRATCH"' EXIT
NOW="$(date +%s)"
DAY=86400

# Sets a path's mtime the given number of seconds in the past, creating it as
# a directory when it is missing.
make_aged() {
    local path="$1" age_s="$2" stamp
    stamp="$(date -r "$((NOW - age_s))" +%Y%m%d%H%M.%S 2>/dev/null ||
        date -d "@$((NOW - age_s))" +%Y%m%d%H%M.%S)"
    [ -e "$path" ] || mkdir -p "$path"
    touch -t "$stamp" "$path"
}

# Whether a process holds cargo's build lock open, as `lsof -t` reports it.
LOCK_HELD="$SCRATCH/lock-held"
# shellcheck disable=SC2329 # called by the prune
lsof() { [ -e "$LOCK_HELD" ] && echo 4242; }

# Builds a fresh fake target dir at $1.
make_target() {
    local t="$1"
    rm -rf "$t"
    make_aged "$t/debug/incremental/api-28e9f3ts2tj7p/s-keep" $((30 * DAY))
    make_aged "$t/debug/incremental/api-28e9f3ts2tj7p" $((10 * DAY))
    make_aged "$t/debug/incremental/core_lib-1abc2def3ghi4" $((8 * DAY))
    make_aged "$t/debug/incremental/fresh-9zzzzzzzzzzzz" $((1 * DAY))
    make_aged "$t/debug/incremental/notes" $((30 * DAY))
    make_aged "$t/debug/incremental/.hidden-abc123" $((30 * DAY))
    make_aged "$t/debug/incremental/-dash-abc123" $((30 * DAY))
    make_aged "$t/wasm32-unknown-unknown/release/incremental/signer-5a5a5a5a5a5a5" $((9 * DAY))
    make_aged "$t/debug/deps/old-0a0a0a0a0a0a0" $((30 * DAY))
    : > "$t/debug/.cargo-lock"
}

present() { [ -e "$1" ]; }

# Lists the trash dirs left in the fake build dir, shared or claimed.
trash_dirs() { find "$T" -mindepth 1 -maxdepth 1 -type d -name "$STALE_INCREMENTAL_TRASH_NAME*" 2>/dev/null; }

T="$SCRATCH/target"
TRASH="$(stale_incremental_trash_dir "$T")"
# A trash dir an interrupted run left behind.
OLD_TRASH="$T/$STALE_INCREMENTAL_TRASH_NAME.999999"

echo "test: only stale, well-named crate dirs inside an incremental dir move"
make_target "$T"
OUT="$(prune_stale_incremental_build_dir "$T")"
gone=""
for d in debug/incremental/api-28e9f3ts2tj7p debug/incremental/core_lib-1abc2def3ghi4 \
    wasm32-unknown-unknown/release/incremental/signer-5a5a5a5a5a5a5; do
    present "$T/$d" && gone="$gone $d"
done
kept=""
for d in debug/incremental/fresh-9zzzzzzzzzzzz debug/incremental/notes \
    debug/incremental/.hidden-abc123 debug/incremental/-dash-abc123 \
    debug/deps/old-0a0a0a0a0a0a0; do
    present "$T/$d" || kept="$kept $d"
done
if [ -z "$gone$kept" ] && [ "$OUT" = "Pruned 3 stale incremental dirs from $T." ] &&
    [ "$(find "$TRASH" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" -eq 3 ]; then
    pass "$OUT"
else
    fail "output: $OUT; still there:$gone; missing:$kept"
fi

echo "test: a second run inside the throttle window scans nothing"
make_aged "$T/debug/incremental/late-7q7q7q7q7q7q7" $((12 * DAY))
OUT="$(prune_stale_incremental_build_dir "$T")"
if [ -z "$OUT" ] && present "$T/debug/incremental/late-7q7q7q7q7q7q7"; then
    pass "left the newly stale dir for tomorrow"
else
    fail "output: $OUT"
fi

echo "test: a stamp older than a day lets the prune run again"
make_aged "$T/$STALE_INCREMENTAL_STAMP_NAME" $((2 * DAY))
OUT="$(prune_stale_incremental_build_dir "$T")"
if [ "$OUT" = "Pruned 1 stale incremental dirs from $T." ] &&
    ! present "$T/debug/incremental/late-7q7q7q7q7q7q7"; then
    pass "$OUT"
else
    fail "output: $OUT"
fi

echo "test: a cargo holding the build lock leaves its profile alone and skips the stamp"
make_target "$T"
touch "$LOCK_HELD"
OUT="$(prune_stale_incremental_build_dir "$T")"
# The wasm profile has no lock file, so only the debug profile waits.
if [ "$OUT" = "Pruned 1 stale incremental dirs from $T." ] && present "$T/debug/incremental/api-28e9f3ts2tj7p" &&
    ! present "$T/$STALE_INCREMENTAL_STAMP_NAME"; then
    pass "the locked profile kept its dirs and the next build tries again"
else
    fail "output: $OUT"
fi

echo "test: with no lsof, perl's flock says whether cargo holds the lock"
make_target "$T"
rm -f "$LOCK_HELD"
# A PATH holding perl and the prune's own tools, and no lsof.
mkdir -p "$SCRATCH/perl-path"
for tool in perl find grep mv mkdir dirname basename touch; do
    ln -s "$(command -v "$tool")" "$SCRATCH/perl-path/$tool"
done
FREE="$(unset -f lsof; PATH="$SCRATCH/perl-path" prune_stale_incremental_build_dir "$T")"
make_target "$T"
# A real flock holder on the debug profile's lock, gone when the subshell ends.
HELD="$(unset -f lsof; export PATH="$SCRATCH/perl-path"
    exec 7>> "$T/debug/.cargo-lock"; _flock_perl 7 ex nb
    prune_stale_incremental_build_dir "$T")"
if [ "$FREE" = "Pruned 3 stale incremental dirs from $T." ] &&
    [ "$HELD" = "Pruned 1 stale incremental dirs from $T." ] &&
    present "$T/debug/incremental/api-28e9f3ts2tj7p"; then
    pass "a free lock prunes, a held one defers"
else
    fail "free: $FREE; held: $HELD"
fi

echo "test: the moves run under cargo's own lock, so no cargo starts mid-prune"
make_target "$T"
rm -f "$LOCK_HELD"
# shellcheck disable=SC2329 # called by the prune
mv() { lock_is_held "$T/debug/.cargo-lock" && echo held >> "$SCRATCH/mv-saw"; command mv "$@"; }
OUT="$(prune_stale_incremental_build_dir "$T")"
unset -f mv
if [ "$OUT" = "Pruned 3 stale incremental dirs from $T." ] &&
    [ "$(grep -c held "$SCRATCH/mv-saw" 2>/dev/null)" = 2 ] && ! lock_is_held "$T/debug/.cargo-lock"; then
    pass "both debug moves saw the lock held, and it was released after"
else
    fail "output: $OUT; saw: $(cat "$SCRATCH/mv-saw" 2>&1)"
fi

echo "test: an unreadable lock is named as such, never as a running cargo"
mkdir -p "$SCRATCH/unreadable"
: > "$SCRATCH/unreadable/.cargo-lock"
chmod 000 "$SCRATCH/unreadable/.cargo-lock"
WHY="$(unset -f lsof; PATH="$SCRATCH/perl-path" _cargo_build_lock_holder "$SCRATCH/unreadable")"
chmod 600 "$SCRATCH/unreadable/.cargo-lock"
if [ "$(id -u)" -eq 0 ]; then
    pass "skipped: root reads a mode 000 file"
elif [ "$WHY" = "could not read cargo's build lock" ]; then
    pass "$WHY"
else
    fail "reason: $WHY"
fi

echo "test: with neither lsof nor perl to ask, nothing is deleted and one line says why"
mkdir -p "$SCRATCH/empty-path"
make_target "$T"
# `hash -r` forgets the perl an earlier test ran, which bash would still find.
OUT="$(unset -f lsof; hash -r; PATH="$SCRATCH/empty-path" prune_stale_incremental_build_dir "$T")"
if [ "$OUT" = "Stale incremental dirs: no lsof or perl to tell whether another cargo is building here; skipped." ] &&
    present "$T/debug/incremental/api-28e9f3ts2tj7p"; then
    pass "skipped"
else
    fail "output: $OUT"
fi

echo "test: a lock nobody holds lets the prune run"
make_target "$T"
rm -f "$LOCK_HELD"
OUT="$(prune_stale_incremental_build_dir "$T")"
if [ "$OUT" = "Pruned 3 stale incremental dirs from $T." ]; then
    pass "$OUT"
else
    fail "output: $OUT"
fi

echo "test: the entry point deletes the moved dirs and a day-old leftover"
make_target "$T"
make_aged "$OLD_TRASH/api-3cccccccccccc.1" $((2 * DAY))
make_aged "$OLD_TRASH" $((2 * DAY))
# shellcheck disable=SC2329 # called by prune_stale_incremental
cargo() { printf '{"packages":[],"target_directory":"%s","version":1}\n' "$T"; }
OUT="$(prune_stale_incremental "$SCRATCH/checkout")"
wait
unset -f cargo
# The prune ran in a subshell, so its background delete is not ours to wait on.
for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -n "$(trash_dirs)" ] || break
    sleep 0.3
done
if [ "$OUT" = "Pruned 3 stale incremental dirs from $T." ] &&
    [ -z "$(trash_dirs)" ] && ! present "$T/debug/incremental/api-28e9f3ts2tj7p"; then
    pass "trash dirs removed in the background"
else
    fail "output: $OUT; trash: $(trash_dirs)"
fi

echo "test: another run's fresh trash dir is left to that run, even inside the throttle window"
make_aged "$OLD_TRASH/api-4dddddddddddd.1" 60
make_aged "$OLD_TRASH" 60
# shellcheck disable=SC2329 # called by prune_stale_incremental
cargo() { printf '{"target_directory":"%s"}\n' "$T"; }
prune_stale_incremental "$SCRATCH/checkout" > /dev/null
wait
unset -f cargo
if present "$OLD_TRASH/api-4dddddddddddd.1"; then
    pass "a running delete's tree untouched"
else
    fail "another run's trash was taken"
fi
rm -rf "$OLD_TRASH"

echo "test: the build dir comes from cargo, then falls back to the repo convention"
# shellcheck disable=SC2329 # called by stale_incremental_build_dir
cargo() { printf '{"target_directory":"/somewhere/else/target","build_directory":"/fast/disk/build"}\n'; }
FROM_BUILD_DIR="$(stale_incremental_build_dir "$SCRATCH/checkout")"
# shellcheck disable=SC2329 # called by stale_incremental_build_dir
cargo() { printf '{"target_directory":"/somewhere/else/target"}\n'; }
FROM_CARGO="$(stale_incremental_build_dir "$SCRATCH/checkout")"
# shellcheck disable=SC2329 # called by stale_incremental_build_dir
cargo() { return 101; }
FALLBACK="$(unset CARGO_TARGET_DIR; stale_incremental_build_dir "$SCRATCH/checkout")"
OVERRIDE="$(CARGO_TARGET_DIR=/custom/target stale_incremental_build_dir "$SCRATCH/checkout")"
unset -f cargo
if [ "$FROM_BUILD_DIR" = "/fast/disk/build" ] && [ "$FROM_CARGO" = "/somewhere/else/target" ] &&
    [ "$FALLBACK" = "$SCRATCH/checkout/target" ] && [ "$OVERRIDE" = "/custom/target" ]; then
    pass "cargo's build dir, its target dir, <checkout>/target, then CARGO_TARGET_DIR"
else
    fail "build dir: $FROM_BUILD_DIR; cargo: $FROM_CARGO; fallback: $FALLBACK; override: $OVERRIDE"
fi

echo "test: a missing target dir is not an error for a set -e caller"
OUT="$(set -e; prune_stale_incremental_build_dir "$SCRATCH/absent"; echo "carried on")"
if [ "$OUT" = "carried on" ]; then
    pass "carried on"
else
    fail "output: $OUT"
fi

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
