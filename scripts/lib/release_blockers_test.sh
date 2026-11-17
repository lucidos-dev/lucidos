#!/bin/bash
# Hermetic fixtures: local git only, with no remote or process signaling.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
source "$ROOT/scripts/lib/release_blockers.sh"
REPO="$(mktemp -d)"
trap 'rm -rf "$REPO"' EXIT
git -C "$REPO" init -qb main
git -C "$REPO" config user.name Test
git -C "$REPO" config user.email test@example.com
HEADER=$'id\tstate\tref\twork_item\ttitle\treason'
seed() {
    printf '%s\n' "$HEADER" > "$REPO/release-blockers.tsv"
    [ -z "$1" ] || printf '%s\n' "$1" >> "$REPO/release-blockers.tsv"
    git -C "$REPO" add .
    git -C "$REPO" commit -qm fixture --allow-empty
}
expect() {
    local expected="$1" actual=0
    release_blockers_check "$REPO" "$CUT" 1.2.3 || actual=$?
    [ "$actual" = "$expected" ] || { echo "expected $expected, got $actual: $RELEASE_GATE_REPORT"; return 1; }
}
seed ''
CUT="$(git -C "$REPO" rev-parse HEAD)"
expect 0
DIGEST="$(git -C "$REPO" rev-parse HEAD:release-blockers.tsv)"
[ "$RELEASE_GATE_DIGEST" = "${DIGEST:0:12}" ]
printf 'broken worktree' > "$REPO/release-blockers.tsv"
expect 0
seed $'one\topen\t-\t-\tBroken\t-'
expect 10
[[ "$RELEASE_GATE_REPORT" = *one* ]]
seed "one"$'\tfixed\t'"$CUT"$'\t-\tBroken\t-'
expect 0
FIX="$(git -C "$REPO" rev-parse HEAD)"
seed "one"$'\tfixed\t'"$FIX"$'\t-\tBroken\t-'
expect 10
seed $'one\twaived\t1.2.3\t-\tBroken\tOwner decision'
expect 0
[[ "$RELEASE_GATE_WAIVERS" = *'Owner decision'* ]]
[[ "$(release_gate_tag_message 1.2.3)" = *one* ]]
TRAILER="$RELEASE_GATE_TRAILER"
git -C "$REPO" commit -qm "$TRAILER" --allow-empty
release_gate_verify_trailer "$REPO" HEAD
seed $'one\twaived\t1.2.4\t-\tBroken\tOwner decision'
expect 10
seed $'one\twithdrawn\t-\t-\tBroken\tNot a blocker'
expect 0
for row in \
    $'one\tunknown\t-\t-\tBroken\t-' \
    $'one\tfixed\tdeadbeef\t-\tBroken\t-' \
    $'one\twaived\tbad\t-\tBroken\tReason' \
    $'one\twithdrawn\t-\t-\tBroken\t-' \
    $'one\topen\t-\t-\tBroken' \
    $'one\topen\t-\t-\tBroken\t-\none\topen\t-\t-\tBroken\t-'; do
    seed "$row"
    expect 1
done
# Bash drops NULs and trailing newlines in command capture. Neither may pass.
for suffix in null blank missing-newline; do
    case "$suffix" in
        null) printf '%s\000\n' "$HEADER" > "$REPO/release-blockers.tsv" ;;
        blank) printf '%s\n\n' "$HEADER" > "$REPO/release-blockers.tsv" ;;
        missing-newline) printf '%s' "$HEADER" > "$REPO/release-blockers.tsv" ;;
    esac
    git -C "$REPO" add .
    git -C "$REPO" commit -qm malformed
    expect 1
done
HEADER=bad
seed ''
expect 1
git -C "$REPO" rm -q release-blockers.tsv
git -C "$REPO" commit -qm missing
expect 1
if release_gate_verify_trailer "$REPO" HEAD; then echo 'missing trailer accepted'; false; fi
for bad in 'pass open=1 waived=0 ledger=123456789abc' 'pass open=0 waived=1 ledger=123456789abc' 'waived open=0 waived=0 ledger=123456789abc' 'pass open=0 waived=0 ledger=123456789abc private-id'; do
    git -C "$REPO" commit -qm "Release-Gate: $bad" --allow-empty
    if release_gate_verify_trailer "$REPO" HEAD; then echo 'bad trailer accepted'; false; fi
done
echo 'release blockers: all fixtures passed'
