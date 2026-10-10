#!/usr/bin/env bash
# Canonical prose measurement: the SINGLE source of truth for the deterministic
# side of the prose rule (see `.claude/rules/prose.md`). Sourced by:
#   - .claude/hooks/prose.sh          (write-time PreToolUse gate)
#   - scripts/check-prose.sh          (diff-scoped gate, /harden Phase 4.5)
#   - scripts/lib/prose_scan_test.sh  (its test)
# Do NOT restate a limit anywhere else, reference this file. The semantic side
# of the same rule lives in the rule doc and in a `code-review` angle, and it
# binds chat replies, which no script can see.
#
# EVERYTHING HERE IS ADDED-LINES-ONLY, on purpose, exactly like
# scripts/lib/em_dash_scan.sh. The tree carries 143,575 comment lines; a
# whole-file scanner would red-light nearly every file and be switched off in a
# day. Both consumers ask one question: "does the text this write ADDS break a
# limit?"
#
# WHY THESE FOUR AND NOT MORE. Three further rules from the same standard (a
# 20-word limit for an imperative step, active voice, 3-word noun clusters) need
# part-of-speech tagging to check. Regex passive-voice detection false-positives
# hard on technical prose ("is used", "is gated on"), so those three are a
# review angle rather than a gate. A gate nobody trusts gets disabled.
#
# TWO KNOWN GAPS, both false NEGATIVES, both a consequence of added-lines-only.
# Neither can make the gate refuse a clean write, which is the failure that
# would matter. Written down because an undocumented gap reads as coverage.
#
#   1. A UNIT is measured from added lines alone. Add one line inside an
#      existing 20-line comment block, 6-sentence paragraph or wrapped sentence
#      and the counters see only your line, so the now-over-limit unit passes.
#      Measuring the whole unit would flag prose the author did not write, which
#      is the retroactivity this rule refuses. The reviewer covers it instead.
#   2. An UNADORNED line inside a `/* ... */` block reads as code, since comment
#      recognition is per-line and a continuation carries no marker. Rust and
#      the JSDoc convention both lead every line with `//` or `*`, so this bites
#      only a bare block comment. The DATE check is exempt: it matches a
#      trailing `//` anywhere on the line.

# The four limits. Sentence and paragraph come from ASD-STE100 Issue 9
# (Simplified Technical English); the block limit is this tree's own 98th
# percentile, measured over 33,533 comment blocks.
PROSE_MAX_COMMENT_BLOCK=20
PROSE_MAX_SENTENCE_WORDS=25
PROSE_MAX_PARAGRAPH_SENTENCES=6

# shellcheck disable=SC2034 # printed by both consumers
PROSE_ADVICE='Cut it, or move the content to a doc, an ADR or a plan and link there. See .claude/rules/prose.md.'

# prose_kind <path>
# Which checks apply to this path: `md` (sentence + paragraph), `src` (all
# four), or empty for a path this gate says nothing about.
#
# Deliberately narrow. `.sql`, `.css` and `.sh` are out until someone measures
# them, because a limit picked without a distribution behind it is a guess, and
# a guessed limit is what makes a gate feel arbitrary.
prose_kind() {
    case "$1" in
        *.md) printf 'md\n' ;;
        *.rs | *.ts | *.tsx | *.js | *.jsx) printf 'src\n' ;;
        *) printf '\n' ;;
    esac
}

# prose_fenced_lines <file>
# Print the line number of every line inside a fenced code block, the fence
# markers included.
#
# This exists because the in-stream fence toggle in prose_check_lines can only
# see the lines a write ADDS, and a line added inside an EXISTING fence arrives
# with no opener in front of it. It then reads as prose, which is a false
# BLOCK: exactly the kind that gets a gate switched off. The first line this
# gate ever judged was one of these (a row added to a `bash` block in
# `.claude/rules/dev-runtime.md`). Whenever the real file is available, its
# fences are authoritative and this mask is used instead of the toggle.
prose_fenced_lines() {
    [ -f "$1" ] || return 0
    awk '
        /^[[:space:]]*(```|~~~)/ { print FNR; infence = !infence; next }
        infence { print FNR }
    ' "$1"
}

# prose_drop_fenced <mask-file>
# Read `<lineno><TAB><text>` on stdin and drop every record whose line number is
# in the mask. A missing or empty mask passes everything through.
prose_drop_fenced() {
    awk -v mask="$1" '
        BEGIN { while ((getline n < mask) > 0) skip[n + 0] = 1 }
        { ln = $0; sub(/\t.*$/, "", ln); if (!((ln + 0) in skip)) print }
    '
}

# prose_check_lines <kind>
# Read `<lineno><TAB><text>` on stdin, print `<lineno>:<message>` for every
# violation. Line numbers are the candidate file's own, so a caller can point
# the author straight at the line.
#
# Contiguity is read off the line numbers rather than assumed: added lines
# arrive from a diff and can be non-adjacent, so a block or paragraph ends
# wherever the numbering jumps. That is what keeps two three-line additions in
# different parts of a file from reading as one six-line block.
prose_check_lines() {
    awk -v kind="$1" \
        -v maxblock="$PROSE_MAX_COMMENT_BLOCK" \
        -v maxsent="$PROSE_MAX_SENTENCE_WORDS" \
        -v maxpara="$PROSE_MAX_PARAGRAPH_SENTENCES" '
        function flush_para(   n, i, s, words) {
            if (para == "") return
            # Sentence COUNT splits on terminators only. Sentence LENGTH (below)
            # also splits on a colon, because this repo uses colons as the
            # em-dash replacement and a colon-joined pair is two measurable
            # units, not one over-long sentence. Counting them as two sentences
            # here as well would make the paragraph limit unreachable in the
            # house style.
            # The trailing class carries markdown emphasis markers, not just
            # closing quotes and brackets: `**Bold sentence.** Next one.` is two
            # sentences, and without `*` and `_` the terminator does not close, so
            # the two fuse into one over-long one. That artifact inflated every
            # measurement taken before it was found.
            n = split(para, S, /[.!?]+[])"'"'"'`*_]*([ \t]+|$)/)
            for (i = 1; i <= n; i++) if (S[i] ~ /[A-Za-z]/) sentences++
            if (sentences > maxpara)
                printf "%d:paragraph of %d sentences (limit %d)\n", para_line, sentences, maxpara
            # Length, on colon-split units.
            n = split(para, U, /[.!?:]+[])"'"'"'`*_]*([ \t]+|$)/)
            for (i = 1; i <= n; i++) {
                s = U[i]
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", s)
                if (s !~ /[A-Za-z]/) continue
                words = split(s, W, /[[:space:]]+/)
                if (words > maxsent)
                    printf "%d:sentence of %d words (limit %d): %.60s\n", para_line, words, maxsent, s
            }
            para = ""; sentences = 0
        }
        function add_prose(text, ln) {
            if (para == "") para_line = ln
            para = (para == "") ? text : para " " text
        }
        {
            ln = $0; sub(/\t.*$/, "", ln); ln = ln + 0
            text = $0; sub(/^[0-9]+\t/, "", text)

            gap = (ln != prevln + 1)
            prevln = ln
            if (gap) { flush_para(); blockrun = 0 }

            # A fenced code block carries no prose. The toggle can only see
            # ADDED lines, so a fence opened outside the hunk is invisible; that
            # direction fails open, which is the right one for a gate.
            if (text ~ /^[[:space:]]*(```|~~~)/) { infence = !infence; flush_para(); next }
            if (infence) next

            is_comment = (kind == "src" && text ~ /^[[:space:]]*(\/\/|\/\*|\*)/)

            if (kind == "src") {
                if (is_comment) {
                    blockrun++
                    if (blockrun == maxblock + 1)
                        printf "%d:comment block runs past %d lines\n", ln, maxblock
                } else {
                    blockrun = 0
                }
                # An ISO date narrating history. A date inside a PATH is the
                # opposite behaviour and is exempt: linking a dated plan file is
                # what the rule asks for instead of narrating in place.
                #
                # A TRAILING comment counts here, unlike everywhere else in this
                # function. `let x = 1; // broke on <date>` is narration just as
                # much as a full-line comment is, and a check anchored at the
                # line start misses every one of them. The `[^:]` guard is what
                # keeps a URL out: `https://host/2026-08-13` is not a comment,
                # and its `//` is always preceded by a colon.
                has_comment = is_comment || match(text, /(^|[^:])\/\//)
                if (has_comment && match(text, /20[0-9][0-9]-[0-9][0-9]-[0-9][0-9]/)) {
                    before = (RSTART > 1) ? substr(text, RSTART - 1, 1) : " "
                    after = substr(text, RSTART + RLENGTH, 1)
                    if (before != "/" && after !~ /[-A-Za-z0-9_]/)
                        printf "%d:ISO date in a comment; git and docs/plans hold history\n", ln
                }
            }

            # Reduce to prose, or end the paragraph.
            if (kind == "src") {
                if (!is_comment) { flush_para(); next }
                sub(/^[[:space:]]*(\/\/[\/!]?|\/\*+|\*+\/?)[[:space:]]?/, "", text)
            }

            gsub(/`[^`]*`/, "CODE", text)          # an inline code span is one word
            gsub(/\[([^]]*)\]\([^)]*\)/, "\\1", text)  # a link reads as its label

            if (text ~ /^[[:space:]]*$/) { flush_para(); next }
            if (text ~ /^[[:space:]]*\|/) { flush_para(); next }        # table row
            if (text ~ /^[[:space:]]*#/ && kind == "md") { flush_para(); next }  # heading
            if (text ~ /^[[:space:]]*(<|-{3,}|={3,})/) { flush_para(); next }    # html, rule
            # A list item is its own unit, so a run of them is not one paragraph.
            if (text ~ /^[[:space:]]*([-*+]|[0-9]+[.)])[[:space:]]/) {
                flush_para()
                sub(/^[[:space:]]*([-*+]|[0-9]+[.)])[[:space:]]+/, "", text)
            }
            add_prose(text, ln)
        }
        END { flush_para() }
    '
}

# prose_added_lines <baseline-file> <candidate-file> <path>
# Print `<lineno>:<message>` for every violation among the candidate lines that
# do not appear verbatim in the baseline. The baseline subtraction is what makes
# a write that merely carries an existing line along pass clean, and it is a
# MULTISET, so duplicating an over-long line still counts. Same semantics as
# em_dash_added_lines, for the same reason.
prose_added_lines() {
    local kind
    kind="$(prose_kind "$3")"
    [ -n "$kind" ] || return 0
    awk -v base="$1" '
        FILENAME == base { seen[$0]++; next }
        { if (seen[$0] > 0) { seen[$0]--; next } printf "%d\t%s\n", FNR, $0 }
    ' "$1" "$2" | prose_check_lines "$kind"
}

# prose_filter_diff
# Read a `git diff -U0` on stdin, print `path:line:message` for every violation
# in an ADDED line. Removed and context lines are ignored, so a file that is
# merely touched never reports the prose it already had.
#
# Two passes over a temp file rather than one stream, because each path's added
# lines must reach `prose_check_lines` as ONE stream for the contiguity test to
# see a run, and a diff can revisit a path.
prose_filter_diff() {
    local tmp path kind rc=0
    tmp="$(mktemp "${TMPDIR:-/tmp}/prose-diff.XXXXXX")" || return 1

    # Pass 1: flatten to `path<TAB>lineno<TAB>text` for added lines only.
    awk '
        # `+++ b/<path>` opens a file. Checked before the generic `^\+` rule,
        # which it would otherwise match.
        /^\+\+\+ / {
            p = substr($0, 5)
            if (p == "/dev/null") { path = "" } else { sub(/^b\//, "", p); path = p }
            next
        }
        # `@@ -a,b +c,d @@ section` seeds the new-file counter with c. The
        # removed range cannot contain a `+`, so stopping at the first one lands
        # on the added range even when the section text has plus signs.
        /^@@ / {
            s = $0
            sub(/^@@[^+]*\+/, "", s)
            sub(/[ ,].*$/, "", s)
            ln = s + 0
            next
        }
        /^\+/ {
            if (path != "") printf "%s\t%d\t%s\n", path, ln, substr($0, 2)
            ln++
            next
        }
    ' > "$tmp" || rc=1

    if [ "$rc" -ne 0 ]; then
        rm -f "$tmp"
        echo "prose_filter_diff: could not parse the diff." >&2
        return 1
    fi

    # Pass 2: one check per path, in first-seen order.
    #
    # The diff compares the base against the WORKING TREE, so an added line's
    # number is that file's real line number and a fence mask taken from disk
    # lines up exactly.
    local mask
    mask="$(mktemp "${TMPDIR:-/tmp}/prose-mask.XXXXXX")" || { rm -f "$tmp"; return 1; }
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        kind="$(prose_kind "$path")"
        [ -n "$kind" ] || continue
        prose_fenced_lines "${PROSE_REPO_ROOT:-.}/$path" > "$mask"
        awk -F'\t' -v p="$path" '$1 == p { line = $0; sub(/^[^\t]*\t/, "", line); print line }' "$tmp" \
            | prose_drop_fenced "$mask" \
            | prose_check_lines "$kind" | awk -v p="$path" '{ print p ":" $0 }'
    done < <(awk -F'\t' '!seen[$1]++ { print $1 }' "$tmp")

    rm -f "$tmp" "$mask"
}

# prose_scan_diff <base-commit> [repo-root]
# Print `path:line:message` for every violation this branch ADDS to a tracked
# file, comparing the base commit against the WORKING TREE.
#
# Exit status is load-bearing: non-zero means the scan could not run, which a
# caller must never read as "clean". Every knob the parse depends on is pinned
# on the command line rather than inherited from the user's git config, for the
# reasons em_dash_scan_diff records.
prose_scan_diff() {
    local base="$1" repo="${2:-.}" out rc
    if out="$(git -C "$repo" -c core.quotePath=false diff --no-ext-diff --no-color \
        --src-prefix=a/ --dst-prefix=b/ -U0 "$base" --)"; then
        rc=0
    else
        rc=$?
    fi
    if [ "$rc" -ne 0 ]; then
        echo "prose_scan_diff: git diff failed (status $rc) against base: $base" >&2
        return 1
    fi
    # Where prose_filter_diff resolves a path to read its fences from. Set
    # explicitly rather than as a command prefix: bash restores a prefix
    # assignment after a FUNCTION call in its default mode but not in posix
    # mode, so the prefix form would work here and silently stop working there.
    PROSE_REPO_ROOT="$repo"
    printf '%s\n' "$out" | prose_filter_diff
    local filter_rc=$?
    unset PROSE_REPO_ROOT
    return "$filter_rc"
}

# prose_scan_untracked [repo-root]
# Print `path:line:message` for every violation in an untracked file. Untracked
# files are absent from `git diff` entirely, yet every one of their lines is
# new, so they are scanned whole.
#
# The listing is captured BEFORE the loop with its status checked: feeding the
# loop from a process substitution would discard `git ls-files`'s exit code, so
# a failure would report a clean scan of files it never looked at.
prose_scan_untracked() {
    local repo="${1:-.}" f kind rc listing
    if listing="$(git -C "$repo" ls-files --others --exclude-standard)"; then
        rc=0
    else
        rc=$?
    fi
    if [ "$rc" -ne 0 ]; then
        echo "prose_scan_untracked: git ls-files failed (status $rc) in: $repo" >&2
        return 1
    fi
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        [ -f "$repo/$f" ] || continue
        kind="$(prose_kind "$f")"
        [ -n "$kind" ] || continue
        awk '{ printf "%d\t%s\n", FNR, $0 }' "$repo/$f" \
            | prose_check_lines "$kind" | awk -v p="$f" '{ print p ":" $0 }'
    done << EOF
$listing
EOF
}
