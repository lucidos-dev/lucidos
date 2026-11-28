#!/usr/bin/env bash
#
# check-plan-sections.sh: refuse an implementation plan that skips the user
# journey or the visual decisions. `lucidos planned mark --plan` runs it next to
# the prose and em-dash gates, so a plan without them never reaches approval.
#
#   ./scripts/check-plan-sections.sh --path docs/plans/<plan>.md
#   ./scripts/check-plan-sections.sh --list-sections
#
# Rules, per required section:
#   - the heading exists and its body is not empty;
#   - a body starting with "None" reads "None: <reason>";
#   - under Visual Decisions, every list item links its picture or live
#     preview under artifacts/.
#
# The section names live only in REQUIRED_SECTIONS. The implementation-plan
# skill's template must carry the same headings; the test pins that.
# Why: docs/plans/2026-11-27-plan-gate-user-journey-and-visual-decisions.md.
#
# Exit status: 0 clean, 1 refused OR the check could not run.

set -uo pipefail

ARTIFACT_SECTION="Visual Decisions"
REQUIRED_SECTIONS=("User Journey" "$ARTIFACT_SECTION")

usage() {
    echo "Usage: $0 --path <plan.md> | --list-sections" >&2
    exit 1
}

PLAN=""
case "${1:-}" in
    --list-sections)
        printf '%s\n' "${REQUIRED_SECTIONS[@]}"
        exit 0
        ;;
    --path)
        [ $# -eq 2 ] || usage
        PLAN="$2"
        ;;
    *) usage ;;
esac

if [ ! -f "$PLAN" ]; then
    echo "ERROR: plan not found: $PLAN" >&2
    exit 1
fi

# Prints one problem per line as "<line>: <message>", or nothing when clean.
section_problems() {
    local name="$1" needs_artifacts="$2"
    awk -v name="$name" -v needs_artifacts="$needs_artifacts" '
        function close_item() {
            if (item_line && item_text !~ /artifacts\/[^[:space:])]/)
                printf "%d: %s item has no artifacts/ link to its picture or preview\n", item_line, name
            item_line = 0; item_text = ""
        }
        function finish() {
            close_item()
            if (!found) { printf "0: missing section \"### %s\"\n", name; return }
            if (!first_line) { printf "%d: section \"%s\" is empty\n", heading_line, name; return }
            if (is_none) return
            if (needs_artifacts == "1" && !items)
                printf "%d: section \"%s\" lists no decisions; write items or \"None: <reason>\"\n", heading_line, name
        }
        { sub(/\r$/, "") }
        /^[[:space:]]*(```|~~~)/ { fenced = !fenced }
        !fenced && /^##?#? / {
            if (inside) { inside = 0; close_item() }
            title = $0; sub(/^#+ +/, "", title); sub(/[[:space:]]+$/, "", title)
            if (title == name && !found) { found = 1; inside = 1; heading_line = NR }
            next
        }
        !inside { next }
        /^[[:space:]]*$/ { next }
        {
            if (!first_line) {
                first_line = NR
                if ($0 ~ /^None([:.]|[[:space:]]*$)/) {
                    is_none = 1
                    if ($0 !~ /^None: *[^[:space:]]/)
                        printf "%d: \"None\" in \"%s\" needs a reason, as \"None: <reason>\"\n", NR, name
                }
            }
            if (is_none || needs_artifacts != "1") next
            if ($0 ~ /^([-*+]|[0-9]+\.) /) { close_item(); items++; item_line = NR; item_text = $0 }
            else if (item_line) item_text = item_text " " $0
        }
        END { finish() }
    ' "$PLAN"
}

problems=""
for section in "${REQUIRED_SECTIONS[@]}"; do
    needs_artifacts=0
    [ "$section" = "$ARTIFACT_SECTION" ] && needs_artifacts=1
    found="$(section_problems "$section" "$needs_artifacts")" || {
        echo "ERROR: could not parse $PLAN" >&2
        exit 1
    }
    [ -n "$found" ] && problems+="$found"$'\n'
done

if [ -z "$problems" ]; then
    exit 0
fi

echo "Plan sections refused for $PLAN (see the implementation-plan skill's template):"
printf '%s' "$problems" | awk -v plan="$PLAN" '{
    line = $0; sub(/:.*/, "", line); text = $0; sub(/^[0-9]+: /, "", text)
    printf "  %s%s: %s\n", plan, (line == "0" ? "" : ":" line), text
}'
exit 1
