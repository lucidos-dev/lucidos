#!/bin/bash
# A portable release gate. The Software Factory app keeps the blocker ledger in
# the workspace event store, keyed by the repository's root commit.
# release_blockers_ledger.py reads it, and this file judges each row against git.
# Return 0 for pass, 10 for blocked, 1 for invalid or unreadable input.
# Sourcing runs no python3 and no CLI, so the public CI job can verify a trailer.

RELEASE_BLOCKERS_LEDGER_PY="${BASH_SOURCE[0]%/*}"
[ "$RELEASE_BLOCKERS_LEDGER_PY" != "${BASH_SOURCE[0]}" ] || RELEASE_BLOCKERS_LEDGER_PY=.
RELEASE_BLOCKERS_LEDGER_PY="$RELEASE_BLOCKERS_LEDGER_PY/release_blockers_ledger.py"

release_blockers_check() {
    local repo="$1" cut="$2" version="$3" rows line out snapshot root
    local -a roots=()
    RELEASE_GATE_VERDICT=error
    RELEASE_GATE_OPEN=0
    RELEASE_GATE_WAIVED=0
    RELEASE_GATE_LEDGER=""
    RELEASE_GATE_SEQUENCE=""
    RELEASE_GATE_DIGEST=""
    RELEASE_GATE_SNAPSHOT=""
    RELEASE_GATE_REPORT="RELEASE GATE"
    RELEASE_GATE_WAIVERS=""
    RELEASE_GATE_TRAILER=""
    if { [ "$version" != auto ] && ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; } \
        || ! git -C "$repo" rev-parse --verify "$cut^{commit}" >/dev/null 2>&1; then
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: invalid version or release commit"
        return 1
    fi
    for root in $(git -C "$repo" rev-list --max-parents=0 "$cut"); do roots+=(--root "$root"); done
    snapshot="$(mktemp)" || return 1
    if ! out="$(python3 "$RELEASE_BLOCKERS_LEDGER_PY" --snapshot "$snapshot" "${roots[@]}" 2>&1)"; then
        rm -f "$snapshot"
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
${out:-ERROR: the blocker ledger reader failed}"
        return 1
    fi
    line="${out%%$'\n'*}"
    if ! [[ "$line" =~ ^ledger\ ([0-9a-f]{40})\ sequence\ ([0-9]+)$ ]]; then
        rm -f "$snapshot"
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: unreadable blocker ledger report"
        return 1
    fi
    RELEASE_GATE_LEDGER="${BASH_REMATCH[1]}"
    RELEASE_GATE_SEQUENCE="${BASH_REMATCH[2]}"
    RELEASE_GATE_DIGEST="$(git -C "$repo" hash-object "$snapshot")" || { rm -f "$snapshot"; return 1; }
    RELEASE_GATE_DIGEST="${RELEASE_GATE_DIGEST:0:12}"
    RELEASE_GATE_SNAPSHOT="$(cat "$snapshot")"
    rows="$(tail -n +2 "$snapshot")"
    rm -f "$snapshot"
    RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
Ledger ${RELEASE_GATE_LEDGER:0:12} at sequence $RELEASE_GATE_SEQUENCE, digest $RELEASE_GATE_DIGEST"
    [ "$out" = "$line" ] || RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
${out#*$'\n'}"
    local id state ref work_item title reason status
    while IFS=$'\t' read -r id state ref work_item title reason; do
        [ -n "$id" ] || continue
        status="$state"
        case "$state" in
            fixed)
                if ! git -C "$repo" rev-parse --verify --quiet "$ref^{commit}" >/dev/null 2>&1; then
                    RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: $id has an unresolved fix"
                    return 1
                fi
                if git -C "$repo" merge-base --is-ancestor "$ref" "$cut"; then
                    status=fixed-in-cut
                else
                    status='fix not in this cut'
                    RELEASE_GATE_OPEN=$((RELEASE_GATE_OPEN + 1))
                fi ;;
            open) RELEASE_GATE_OPEN=$((RELEASE_GATE_OPEN + 1)) ;;
            withdrawn) ;;
            waived)
                if [ "$ref" = "$version" ]; then
                    RELEASE_GATE_WAIVED=$((RELEASE_GATE_WAIVED + 1))
                    line="$id: $title ($reason)"
                    RELEASE_GATE_WAIVERS="${RELEASE_GATE_WAIVERS}${RELEASE_GATE_WAIVERS:+
}$line"
                else
                    status="waiver is only for $ref"
                    RELEASE_GATE_OPEN=$((RELEASE_GATE_OPEN + 1))
                fi ;;
            *)
                RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: $id is in state $state"
                return 1 ;;
        esac
        [ "$reason" != - ] || reason=""
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
$id: $status: $title [$work_item]${reason:+ ($reason)}"
    done <<ROWS
$rows
ROWS
    RELEASE_GATE_VERDICT=pass
    if [ "$RELEASE_GATE_OPEN" -gt 0 ]; then
        RELEASE_GATE_VERDICT=blocked
        return 10
    fi
    [ "$RELEASE_GATE_WAIVED" -eq 0 ] || RELEASE_GATE_VERDICT=waived
    RELEASE_GATE_TRAILER="Release-Gate: $RELEASE_GATE_VERDICT open=0 waived=$RELEASE_GATE_WAIVED ledger=$RELEASE_GATE_DIGEST"
    RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
$RELEASE_GATE_TRAILER"
}

# The public proof contains only counts and a digest, never private row content.
release_gate_verify_trailer() {
    local message trailer count
    message="$(git -C "$1" show -s --format=%B "$2")" || return 1
    count="$(printf '%s\n' "$message" | awk '/^Release-Gate:/ {n++} END {print n+0}')"
    [ "$count" -eq 1 ] || return 1
    trailer="$(printf '%s\n' "$message" | awk '/^Release-Gate:/ {print}')"
    [[ "$trailer" =~ ^Release-Gate:\ pass\ open=0\ waived=0\ ledger=[0-9a-f]{12}$ ]] \
        || [[ "$trailer" =~ ^Release-Gate:\ waived\ open=0\ waived=[1-9][0-9]*\ ledger=[0-9a-f]{12}$ ]]
}

# The tag keeps the whole snapshot, so each release's ledger survives on origin.
release_gate_tag_message() {
    printf 'Release v%s\n\n%s\n' "$1" "$RELEASE_GATE_REPORT"
    [ -z "${RELEASE_GATE_SNAPSHOT:-}" ] || printf '\nLedger snapshot:\n%s\n' "$RELEASE_GATE_SNAPSHOT"
}
