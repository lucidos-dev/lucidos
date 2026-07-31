#!/usr/bin/env bash
# Canonical em-dash detection: the SINGLE source of truth for the
# deterministic side of the no-em-dash rule (see
# `.claude/rules/no-em-dashes.md`). Sourced by:
#   - .claude/hooks/no-em-dashes.sh        (write-time PreToolUse gate)
#   - scripts/check-em-dashes.sh           (diff-scoped gate, /harden Phase 4.5)
#   - scripts/lib/em_dash_scan_test.sh     (its test)
# Do NOT copy the characters or the advice text anywhere else, reference this
# file. The semantic side of the same rule lives in the rule doc and binds chat
# replies, which no script can see.
#
# EVERYTHING HERE IS ADDED-LINES-ONLY, on purpose. Roughly 29,000 lines in this
# repo already carry the character; a whole-file or whole-tree scanner would
# red-light on all of them and be switched off within a day. Both consumers ask
# the same question: "does the text this write ADDS carry a banned character?"

# The two banned characters, spelled as byte escapes rather than literals so
# this file and its consumers stay clean under the rule they enforce, and so a
# future edit to any of them is not blocked by our own write-time hook. No
# exemption list exists anywhere in this gate, and this is why none is needed.
EM_DASH_U2014=$'\xe2\x80\x94' # U+2014 EM DASH
EM_DASH_U2015=$'\xe2\x80\x95' # U+2015 HORIZONTAL BAR, a visual lookalike that
                              # would slip through a check written for U+2014.
# U+2013 EN DASH is deliberately absent: it is legitimate in numeric ranges
# (`3-5`, `2024-2026`) and is NOT banned. Do not widen this on a guess.

# One sentence, shared by every failure message so the fix is always spelled the
# same way. Callers print it verbatim.
# shellcheck disable=SC2034 # printed by both consumers: check-em-dashes.sh and .claude/hooks/no-em-dashes.sh
EM_DASH_ADVICE='Use a comma, a colon, parentheses, or split it into two sentences. See .claude/rules/no-em-dashes.md.'

# em_dash_text_has <text>
# True when the text carries either banned character. `grep -F` on the raw
# bytes, so it is locale-independent.
em_dash_text_has() {
    printf '%s' "$1" | grep -qF -e "$EM_DASH_U2014" -e "$EM_DASH_U2015"
}

# em_dash_added_lines <baseline-file> <candidate-file>
# Print `<lineno-in-candidate>:<content>` for every candidate line that carries
# a banned character AND does not appear verbatim in the baseline.
#
# The baseline subtraction is the whole point: it is what makes a write that
# merely CARRIES an existing line along (an Edit whose old_string and new_string
# share it, a Write that rewrites a file around it) pass clean, while a line
# this write introduces or rewords is flagged. Same semantics as a git diff,
# where a modified line reads as an added one: touch a line and you own it.
#
# Compares FILENAME against the passed-in baseline path rather than the usual
# `NR==FNR`, which mis-attributes the candidate's first record to the baseline
# when the baseline file is empty (a brand-new Write target, the common case).
#
# The baseline is a MULTISET, not a set: each candidate line consumes one
# baseline copy. With plain membership, duplicating a line that already carried
# a dash would let the new copy through, since the original vouched for it. A
# git diff counts that second copy as added, and so does this.
em_dash_added_lines() {
    awk -v base="$1" -v em="$EM_DASH_U2014" -v hb="$EM_DASH_U2015" '
        FILENAME == base { seen[$0]++; next }
        (index($0, em) || index($0, hb)) {
            if (seen[$0] > 0) { seen[$0]--; next }
            printf "%d:%s\n", FNR, $0
        }
    ' "$1" "$2"
}

# em_dash_filter_diff
# Read a `git diff -U0` on stdin, print `path:line:content` for every ADDED line
# carrying a banned character. Removed and context lines are ignored, so a file
# that is merely touched never reports the dashes it already had.
em_dash_filter_diff() {
    awk -v em="$EM_DASH_U2014" -v hb="$EM_DASH_U2015" '
        # `+++ b/<path>` opens a file. Checked before the generic `^\+` rule,
        # which it would otherwise match.
        /^\+\+\+ / {
            p = substr($0, 5)
            if (p == "/dev/null") { path = "" } else { sub(/^b\//, "", p); path = p }
            next
        }
        # `@@ -a,b +c,d @@ section` seeds the new-file line counter with c. The
        # removed range cannot contain a `+`, so stopping at the first one lands
        # on the added range even when the trailing section text has plus signs.
        /^@@ / {
            s = $0
            sub(/^@@[^+]*\+/, "", s)
            sub(/[ ,].*$/, "", s)
            ln = s + 0
            next
        }
        /^\+/ {
            line = substr($0, 2)
            if (path != "" && (index(line, em) || index(line, hb)))
                printf "%s:%d:%s\n", path, ln, line
            ln++
            next
        }
    '
}

# em_dash_scan_diff <base-commit> [repo-root]
# Print `path:line:content` for every banned character this branch ADDS to a
# tracked file, comparing the base commit against the WORKING TREE (so
# uncommitted edits count too, not just committed ones).
#
# Exit status is load-bearing: non-zero means the scan could not run, which a
# caller must never read as "clean".
#
# Every knob the parse depends on is pinned on the command line rather than
# inherited from the user's git config: `--src-prefix` / `--dst-prefix` because
# `diff.mnemonicPrefix` renames `b/` to `w/` (the path would then be reported
# with the prefix still attached, unresolvable for whoever has to fix it),
# `--no-ext-diff` and `--no-color` because either one reshapes the output the
# awk filter reads.
em_dash_scan_diff() {
    local base="$1" repo="${2:-.}" out rc
    if out="$(git -C "$repo" -c core.quotePath=false diff --no-ext-diff --no-color \
        --src-prefix=a/ --dst-prefix=b/ -U0 "$base" --)"; then
        rc=0
    else
        rc=$?
    fi
    if [ "$rc" -ne 0 ]; then
        echo "em_dash_scan_diff: git diff failed (status $rc) against base: $base" >&2
        return 1
    fi
    printf '%s\n' "$out" | em_dash_filter_diff
}

# em_dash_scan_untracked [repo-root]
# Print `path:line:content` for every banned character in an untracked file.
# Untracked files are absent from `git diff` entirely, yet every one of their
# lines is new, so they are scanned whole.
#
# The listing is captured BEFORE the loop, with its status checked. Feeding the
# loop from a process substitution instead would discard `git ls-files`'s exit
# code: the loop body would simply never run, and the function would return 0,
# reporting a clean scan of files it never looked at.
em_dash_scan_untracked() {
    local repo="${1:-.}" f hits rc listing
    if listing="$(git -C "$repo" ls-files --others --exclude-standard)"; then
        rc=0
    else
        rc=$?
    fi
    if [ "$rc" -ne 0 ]; then
        echo "em_dash_scan_untracked: git ls-files failed (status $rc) in: $repo" >&2
        return 1
    fi
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        # Regular files only. A broken symlink or a fifo carries no prose, and
        # grep on one errors, which the fail-closed arm below would turn into a
        # refusal of the whole gate. A permission-denied REGULAR file still
        # errors there, which is the case that should refuse.
        [ -f "$repo/$f" ] || continue
        if hits="$(grep -nIF -e "$EM_DASH_U2014" -e "$EM_DASH_U2015" -- "$repo/$f")"; then
            rc=0
        else
            rc=$?
        fi
        case "$rc" in
            0) printf '%s\n' "$hits" | awk -v p="$f" '{ print p ":" $0 }' ;;
            1) ;; # no match, the clean case
            *)
                echo "em_dash_scan_untracked: grep failed (status $rc) on: $f" >&2
                return 1
                ;;
        esac
    done << EOF
$listing
EOF
}
