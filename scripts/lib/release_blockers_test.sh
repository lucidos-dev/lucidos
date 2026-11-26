#!/bin/bash
# Hermetic fixtures: local git and a fake `lucidos` first on PATH. No remote,
# no process signaling, and no live workspace.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
source "$ROOT/scripts/lib/release_blockers.sh"
source "$ROOT/scripts/lib/release_blockers_fake_cli.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
REPO="$WORK/repo"
FAKE_BIN="$WORK/bin"
release_blockers_fake_cli "$FAKE_BIN" "$WORK/events.jsonl"
PATH="$FAKE_BIN:$PATH"
git -C "$WORK" init -qb main repo
git -C "$REPO" config user.name Test
git -C "$REPO" config user.email test@example.com
git -C "$REPO" commit -qm root --allow-empty
CUT="$(git -C "$REPO" rev-parse HEAD)"
LEDGER="\"ledger\":\"$CUT\""
git -C "$REPO" commit -qm after-cut --allow-empty
AFTER="$(git -C "$REPO" rev-parse HEAD)"
HEADER=$'id\tstate\tref\twork_item\ttitle\treason'

fresh() {
    : > "$FAKE_LEDGER_EVENTS"
    rm -f "$FAKE_LEDGER_EVENTS.moves"
    ledger_event ReleaseBlockerLedgerOpened device "$LEDGER"
}
opened() {
    fresh
    ledger_event ReleaseBlockerOpened agent "$LEDGER,\"blocker_id\":\"one\",\"title\":\"Broken\""
}
expect() {
    local expected="$1" actual=0
    release_blockers_check "$REPO" "$CUT" "${2:-1.2.3}" || actual=$?
    [ "$actual" = "$expected" ] || { echo "expected $expected, got $actual: $RELEASE_GATE_REPORT"; return 1; }
    [ -z "$(git -C "$REPO" status --porcelain)" ] || { echo 'the check wrote into the work tree'; return 1; }
}

fresh
expect 0
[ "$RELEASE_GATE_LEDGER" = "$CUT" ]
[ "$RELEASE_GATE_DIGEST" = "$(printf '%s\n' "$HEADER" | git hash-object --stdin | cut -c1-12)" ]
[[ "$RELEASE_GATE_REPORT" = *"Ledger ${CUT:0:12} at sequence $RELEASE_GATE_SEQUENCE, digest $RELEASE_GATE_DIGEST"* ]]

opened
expect 10
[[ "$RELEASE_GATE_REPORT" = *'one: open: Broken [-]'* ]]

ledger_event ReleaseBlockerFixed agent "$LEDGER,\"blocker_id\":\"one\",\"commit\":\"$CUT\""
expect 0
SEQUENCE="$RELEASE_GATE_SEQUENCE" DIGEST="$RELEASE_GATE_DIGEST"
ledger_event ReleaseBlockerFixed agent "$LEDGER,\"blocker_id\":\"one\",\"commit\":\"$AFTER\""
expect 10
[[ "$RELEASE_GATE_REPORT" = *'fix not in this cut'* ]]
python3 "$RELEASE_BLOCKERS_LEDGER_PY" --snapshot "$WORK/replay.tsv" --root "$CUT" --until-sequence "$SEQUENCE" >/dev/null
[ "$(git hash-object "$WORK/replay.tsv" | cut -c1-12)" = "$DIGEST" ] || { echo 'replay gave another digest'; false; }
ledger_event ReleaseBlockerFixed agent "$LEDGER,\"blocker_id\":\"one\",\"commit\":\"deadbeef1\""
expect 1

opened
ledger_event ReleaseBlockerWaived device "$LEDGER,\"blocker_id\":\"one\",\"version\":\"1.2.3\",\"reason\":\"Owner decision\""
expect 0
[ "$RELEASE_GATE_VERDICT" = waived ] && [ "$RELEASE_GATE_WAIVED" = 1 ]
[[ "$RELEASE_GATE_WAIVERS" = *'Owner decision'* ]]
[[ "$(release_gate_tag_message 1.2.3)" = *"Ledger ${CUT:0:12}"*'one: waived: Broken [-] (Owner decision)'*"Ledger snapshot:"*$'\none\twaived\t1.2.3\t-\tBroken\tOwner decision'* ]]
git -C "$REPO" commit -qm "$RELEASE_GATE_TRAILER" --allow-empty
release_gate_verify_trailer "$REPO" HEAD
git -C "$REPO" reset -q --hard "$AFTER"
expect 10 1.2.4
[[ "$RELEASE_GATE_REPORT" = *'waiver is only for 1.2.3'* ]]
expect 10 auto

opened
ledger_event ReleaseBlockerWaived agent "$LEDGER,\"blocker_id\":\"one\",\"version\":\"1.2.3\",\"reason\":\"Agent tried\""
expect 10
[[ "$RELEASE_GATE_REPORT" = *'unconfirmed: ReleaseBlockerWaived'* ]]
ledger_event ReleaseBlockerWithdrawn agent "$LEDGER,\"blocker_id\":\"one\",\"reason\":\"Agent tried\""
expect 10
ledger_event ReleaseBlockerWithdrawn device "$LEDGER,\"blocker_id\":\"one\",\"reason\":\"Not a blocker\""
expect 0

# A request the owner has since carried out is not listed as waiting.
opened
ledger_event ReleaseBlockerWaived agent "$LEDGER,\"blocker_id\":\"one\",\"version\":\"1.2.3\",\"reason\":\"Ship it\""
ledger_event ReleaseBlockerWaived device "$LEDGER,\"blocker_id\":\"one\",\"version\":\"1.2.3\",\"reason\":\"Ship it\""
ledger_event ReleaseBlockerLedgerOpened agent "$LEDGER"
expect 0
[[ "$RELEASE_GATE_REPORT" != *unconfirmed* ]] || { echo "a settled request is listed: $RELEASE_GATE_REPORT"; false; }

# A rejected record refuses every release until a device voids it.
opened
ledger_event ReleaseBlockerFixed agent "$LEDGER,\"blocker_id\":\"ghost\",\"commit\":\"$CUT\""
REJECTED="00000000-0000-4000-8000-$(printf '%012d' "$FAKE_LEDGER_SEQUENCE")"
ledger_event ReleaseBlockerWithdrawn device "$LEDGER,\"blocker_id\":\"one\",\"reason\":\"Not a blocker\""
expect 1
[[ "$RELEASE_GATE_REPORT" = *'transition on an unknown blocker'* ]]
ledger_event ReleaseBlockerRecordVoided agent "$LEDGER,\"event_id\":\"$REJECTED\",\"reason\":\"Typo\""
expect 1
ledger_event ReleaseBlockerRecordVoided device "$LEDGER,\"event_id\":\"$REJECTED\",\"reason\":\"Typo\""
expect 0

# A void filed in another ledger leaves the rejection in place.
opened
ledger_event ReleaseBlockerFixed agent "$LEDGER,\"blocker_id\":\"ghost\",\"commit\":\"$CUT\""
REJECTED="00000000-0000-4000-8000-$(printf '%012d' "$FAKE_LEDGER_SEQUENCE")"
ledger_event ReleaseBlockerRecordVoided device "\"ledger\":\"$AFTER\",\"event_id\":\"$REJECTED\",\"reason\":\"Typo\""
expect 1
[[ "$RELEASE_GATE_REPORT" = *'transition on an unknown blocker'* ]]

# A ledger opening counts only from a device, and only once.
: > "$FAKE_LEDGER_EVENTS"
ledger_event ReleaseBlockerLedgerOpened agent "$LEDGER"
expect 1
[[ "$RELEASE_GATE_REPORT" = *'no device-opened ledger'* ]]
fresh
ledger_event ReleaseBlockerLedgerOpened device "$LEDGER"
expect 1
[[ "$RELEASE_GATE_REPORT" = *'ledger opened twice'* ]]
: > "$FAKE_LEDGER_EVENTS"
ledger_event ReleaseBlockerLedgerOpened device "\"ledger\":\"$(printf 'a%.0s' {1..40})\""
expect 1

# Every other input error fails closed.
opened
ledger_event ReleaseBlockerFixed agent "$LEDGER,\"blocker_id\":\"one\",\"commit\":\"not-hex\""
expect 1
[[ "$RELEASE_GATE_REPORT" = *'blocker one is invalid: bad commit'* ]]
opened
ledger_event ReleaseBlockerOpened agent "$LEDGER,\"v\":2,\"blocker_id\":\"two\",\"title\":\"Later\""
expect 1
[[ "$RELEASE_GATE_REPORT" = *'unknown v'* ]]
opened
ledger_event ReleaseBlockerOpened agent "\"blocker_id\":\"two\",\"title\":\"No ledger\""
expect 1
[[ "$RELEASE_GATE_REPORT" = *'no readable ledger'* ]]
opened
for mode in fail garbage moving; do
    FAKE_LEDGER_MODE="$mode" expect 1
done
[[ "$RELEASE_GATE_REPORT" = *'changed twice'* ]]
PATH="$WORK/empty:/usr/bin:/bin" expect 1
[[ "$RELEASE_GATE_REPORT" = *'lucidos CLI cannot run'* ]]
if release_blockers_check "$REPO" "$CUT" 1.2; then echo 'bad version accepted'; false; fi

# The public CI job sources this file with neither python3 nor the CLI on PATH.
mkdir -p "$WORK/ci-bin"
ln -s "$(command -v git)" "$WORK/ci-bin/git"
ln -s "$(command -v awk)" "$WORK/ci-bin/awk"
git -C "$REPO" commit -qm 'Release-Gate: pass open=0 waived=0 ledger=123456789abc' --allow-empty
PATH="$WORK/ci-bin" "$BASH" -c "source '$ROOT/scripts/lib/release_blockers.sh' && release_gate_verify_trailer '$REPO' HEAD" \
    || { echo 'the trailer check needs more than git and awk'; false; }

git -C "$REPO" commit -qm 'no trailer' --allow-empty
if release_gate_verify_trailer "$REPO" HEAD; then echo 'missing trailer accepted'; false; fi
for bad in 'pass open=1 waived=0 ledger=123456789abc' 'pass open=0 waived=1 ledger=123456789abc' 'waived open=0 waived=0 ledger=123456789abc' 'pass open=0 waived=0 ledger=123456789abc private-id'; do
    git -C "$REPO" commit -qm "Release-Gate: $bad" --allow-empty
    if release_gate_verify_trailer "$REPO" HEAD; then echo 'bad trailer accepted'; false; fi
done
echo 'release blockers: all fixtures passed'
