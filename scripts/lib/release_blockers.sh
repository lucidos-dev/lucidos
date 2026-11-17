#!/bin/bash
# A portable release gate. The private TSV is read from main, never the worktree.
# Columns: id, state, ref, work_item, title, reason. No embedded tabs or newlines.
# Return 0 for pass, 10 for blocked, 1 for invalid or unreadable input.

release_blockers_check() {
    local repo="$1" cut="$2" version="$3" ledger_ref="${4:-main}" ledger rows line
    RELEASE_GATE_VERDICT=error
    RELEASE_GATE_OPEN=0
    RELEASE_GATE_WAIVED=0
    RELEASE_GATE_LEDGER_COMMIT=""
    RELEASE_GATE_DIGEST=""
    RELEASE_GATE_REPORT="RELEASE GATE"
    RELEASE_GATE_WAIVERS=""
    RELEASE_GATE_TRAILER=""
    if { [ "$version" != auto ] && ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; } \
        || ! git -C "$repo" rev-parse --verify "$cut^{commit}" >/dev/null 2>&1; then
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: invalid version or release commit"
        return 1
    fi
    RELEASE_GATE_LEDGER_COMMIT="$(git -C "$repo" rev-parse --verify "$ledger_ref^{commit}")" || return 1
    ledger="$(git -C "$repo" show "$RELEASE_GATE_LEDGER_COMMIT:release-blockers.tsv")" || {
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: missing blocker ledger"
        return 1
    }
    local blob normalized
    blob="$(git -C "$repo" rev-parse "$RELEASE_GATE_LEDGER_COMMIT:release-blockers.tsv")" || return 1
    normalized="$(printf '%s\n' "$ledger" | git -C "$repo" hash-object --stdin)" || return 1
    if [ "$blob" != "$normalized" ]; then
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: ledger must contain text rows ending in one newline"
        return 1
    fi
    RELEASE_GATE_DIGEST="${blob:0:12}"
    if ! rows="$(printf '%s\n' "$ledger" | LC_ALL=C awk -F '\t' '
        NR == 1 { if ($0 != "id\tstate\tref\twork_item\ttitle\treason") exit 1; next }
        {
            if (NF != 6 || $1 !~ /^[a-z0-9]+(-[a-z0-9]+)*$/ || seen[$1]++ ||
                $4 == "" || $5 == "" || $0 ~ /[\r\001-\010\013\014\016-\037\177]/) exit 1
            if ($2 == "open") { if ($3 != "-" || $6 != "-") exit 1 }
            else if ($2 == "fixed") { if ($3 !~ /^[0-9a-f]+$/ || length($3) < 7 || $6 != "-") exit 1 }
            else if ($2 == "withdrawn") { if ($3 != "-" || $6 == "" || $6 == "-") exit 1 }
            else if ($2 == "waived") { if ($3 !~ /^[0-9]+\.[0-9]+\.[0-9]+$/ || $6 == "" || $6 == "-") exit 1 }
            else exit 1
            print
        }
        END { if (NR == 0) exit 1 }
    ')"; then
        RELEASE_GATE_REPORT="$RELEASE_GATE_REPORT
ERROR: malformed blocker ledger"
        return 1
    fi
    local id state ref work_item title reason status
    while IFS=$'\t' read -r id state ref work_item title reason; do
        [ -n "$id" ] || continue
        status="$state"
        case "$state" in
            fixed)
                if ! git -C "$repo" rev-parse --verify "$ref^{commit}" >/dev/null 2>&1; then
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
        esac
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

release_gate_tag_message() {
    printf 'Release v%s\n\n%s\n' "$1" "$RELEASE_GATE_REPORT"
}
