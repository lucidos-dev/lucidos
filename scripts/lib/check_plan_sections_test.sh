#!/bin/bash
# Tests for scripts/check-plan-sections.sh, the plan gate that refuses a plan
# without a user journey or visual decisions.
#
# Hermetic: every fixture is a throwaway file in a temp directory. The one read
# of the real tree is the pin test, which checks that the implementation-plan
# skill's template carries every heading the script requires.
#
# Run: ./scripts/lib/check_plan_sections_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLI="$SCRIPT_DIR/../check-plan-sections.sh"
SKILL="$SCRIPT_DIR/../../.claude/skills/implementation-plan/SKILL.md"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() {
    FAIL=$((FAIL + 1))
    echo "FAIL: $1" >&2
}

# expect <name> <exit> <output substring or empty> <plan body>
expect() {
    local name="$1" want_rc="$2" want_text="$3" body="$4"
    local plan="$TMP/$name.md" out rc
    printf '%s\n' "$body" > "$plan"
    out="$("$CLI" --path "$plan" 2>&1)"
    rc=$?
    if [ "$rc" != "$want_rc" ]; then
        bad "$name: exit $rc, want $want_rc. Output: $out"
        return
    fi
    if [ -n "$want_text" ] && [[ "$out" != *"$want_text"* ]]; then
        bad "$name: output lacks \"$want_text\". Output: $out"
        return
    fi
    ok
}

JOURNEY='### User Journey

1. The user opens the panel.
2. The user taps Save.'

expect clean 0 "" "## Implementation Plan

$JOURNEY

### Visual Decisions

- Save button: option B, ![B](artifacts/save/option-b.png)
- Empty state: the live preview,
  [Open](artifacts/save/preview.html)

### Implementation Invariants
- none of this is checked"

expect none-with-reason 0 "" "$JOURNEY

### Visual Decisions

None: a CLI change, nothing is drawn."

expect missing-journey 1 'missing section "### User Journey"' "### Visual Decisions

None: nothing drawn."

expect missing-visual 1 'missing section "### Visual Decisions"' "$JOURNEY"

expect empty-journey 1 'section "User Journey" is empty' "### User Journey

### Visual Decisions

None: nothing drawn."

expect none-without-reason 1 'needs a reason' "$JOURNEY

### Visual Decisions

None"

expect item-without-artifact 1 ':8: Visual Decisions item has no artifacts/ link' "$JOURNEY

### Visual Decisions

- Save button: option B, picked in the grill"

expect prose-without-items 1 'lists no decisions' "$JOURNEY

### Visual Decisions

We talked about it."

expect next-heading-ends-section 1 'item has no artifacts/ link' "$JOURNEY

### Visual Decisions

- Header: option A

### Phases
- see artifacts/elsewhere.png"

expect fenced-comment-is-not-a-heading 1 ':15: Visual Decisions item has no artifacts/ link' "$JOURNEY

### Visual Decisions

- Save button: option B, ![B](artifacts/save/option-b.png)

\`\`\`sh
# build it
make
\`\`\`

- Header: option A, picked in the grill"

expect artifacts-dir-without-target 1 'item has no artifacts/ link' "$JOURNEY

### Visual Decisions

- Header: option B; artifacts/ link will be added later"

expect plus-bullet 0 "" "$JOURNEY

### Visual Decisions

+ Header: [B](artifacts/header.png)"

expect prose-starting-with-none 0 "" "### User Journey

None of the existing steps change; the user taps Save.

### Visual Decisions

None: nothing drawn."

crlf_plan="$TMP/crlf.md"
printf '%s\n' "$JOURNEY" "" "### Visual Decisions" "" "None: nothing drawn." \
    | awk '{ printf "%s\r\n", $0 }' > "$crlf_plan"
if out="$("$CLI" --path "$crlf_plan" 2>&1)"; then
    ok
else
    bad "crlf plan refused: $out"
fi

if out="$("$CLI" --path "$TMP/absent.md" 2>&1)"; then
    bad "absent plan exits 0"
elif [[ "$out" != *"plan not found"* ]]; then
    bad "absent plan: $out"
else
    ok
fi

while IFS= read -r section; do
    if grep -qx "### $section" "$SKILL"; then
        ok
    else
        bad "skill template lacks \"### $section\""
    fi
done < <("$CLI" --list-sections)

echo "check_plan_sections_test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
