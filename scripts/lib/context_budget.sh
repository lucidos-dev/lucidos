#!/usr/bin/env bash
# Canonical always-loaded-context measurement: the SINGLE source of truth for
# which instruction files Claude Code holds resident in EVERY session, how big
# that set is, and what it is allowed to be. Sourced by:
#   - scripts/check-context-budget.sh        (the gate, /harden Phase 4.5)
#   - scripts/lib/context_budget_test.sh     (its test)
# Do NOT restate the ceiling or the expected file list anywhere else, reference
# this file.
#
# WHY A GATE AND NOT A NORM. `CLAUDE.md` went from 19,417 chars on 2026-06-15 to
# 38,355 on 2026-08-06: +98% in seven weeks, close to linear. Everyone appends,
# nobody deletes, and the documented "keep it under 200 lines" guidance does not
# bite here because this repo writes long paragraphs: 151 lines currently carry
# 5,404 words. Lines are a proxy for tokens and the proxy broke, so this gate
# counts bytes instead.
#
# TWO ARMS, BOTH HARD.
#
#   1. SIZE. The always-loaded set must stay at or under CONTEXT_BUDGET_CEILING.
#      Every byte here is paid on every request of every session, before the
#      agent has read a single line of code.
#
#   2. MEMBERSHIP. The set must be exactly CONTEXT_BUDGET_EXPECTED_ALWAYS. This
#      arm is the important one and it is not about size at all: it is the
#      regression detector for a rule that was meant to be path-scoped and
#      silently is not. That already happened here. Every rule file used the
#      `globs:` key (a Cursor convention Claude Code ignores) until 2026-07-25,
#      so the whole rule set was resident in every session and nothing said so.
#      A rule that silently fails to scope looks exactly like a rule that does
#      not exist, and the failure is invisible from inside a session.
#
# Deliberately NOT counted: path-scoped rules (they are the fix, not the
# problem), skill bodies (they load on invocation), and nested CLAUDE.md files
# (this repo has none; if one appears, the membership arm reports it).
#
# The engine system prompt is the OTHER unconditional surface, and it is gated
# separately rather than not at all: `PROMPT_FLAVOR_CEILINGS` in
# crates/lucidos-engine/src/engine/agent_session/prompts.rs ratchets each
# assembled prompt per flavor and per backend. It is a Rust test because only
# Rust can measure the assembled string, which includes per-flavor inline text
# and the backend tail that `append_backend_rules` adds; a scan of this file's
# kind would see the consts and miss both. Which content belongs on which
# surface is `docs/agent-config.md` § Which surface owns a rule.

# Byte ceiling for the always-loaded set. A RATCHET: lowering it is the point
# and needs no ceremony, raising it needs a reason in the commit message that
# says what became worth paying for on every request. Set to the measured total
# at the time of writing, so the gate starts exactly where the tree does.
# shellcheck disable=SC2034 # read by scripts/check-context-budget.sh, which sources this file
CONTEXT_BUDGET_CEILING=48113

# Every file that is allowed to be resident, relative to the repo root. An
# unscoped rule not on this list fails the membership arm even when the total is
# comfortably under the ceiling. Adding a name here is the deliberate act of
# saying "this must be in front of the agent before it touches any file".
#
# `CLAUDE.md` plus the five rules that genuinely cannot be gated on a path:
# safety rules and rules that govern prose and chat replies as well as code.
# Each one states its own reason in its opening line; see also
# `.claude/rules/` and the always-loaded section of `CLAUDE.md`.
# shellcheck disable=SC2034 # read by scripts/check-context-budget.sh, which sources this file
CONTEXT_BUDGET_EXPECTED_ALWAYS=(
    "CLAUDE.md"
    ".claude/rules/glossary.md"
    ".claude/rules/no-em-dashes.md"
    ".claude/rules/no-private-data.md"
    ".claude/rules/philosophy.md"
    ".claude/rules/temporary-measures.md"
)

# Printed verbatim by the gate so the fix is always spelled the same way.
# shellcheck disable=SC2034 # printed by scripts/check-context-budget.sh
CONTEXT_BUDGET_ADVICE='Move reference material to a skill (loads on invocation), a convention to a path-scoped rule (loads on a matching Read), or maintainer prose to docs/agent-config.md (never loads). See docs/agent-config.md.'

# context_budget_is_always_loaded <file>
# True when Claude Code holds this rule file resident in every session.
#
# The question is NOT "does it have frontmatter" but "does it have a usable
# `paths:` key", which is what makes this the `globs:` detector. Three ways a
# rule ends up always-loaded, and the check has to catch all three:
#
#   - no frontmatter at all                  (the documented, intended way)
#   - frontmatter with no `paths:` key       (the `globs:` bug, and any typo of
#                                             the key: `path:`, `Paths:`, ...)
#   - `paths:` whose only pattern is `**`    (matches everything, so scoping it
#                                             is the same as not scoping it)
#
# Reads only the frontmatter block, so a `paths:` mentioned in the body prose
# (every one of these files discusses the mechanism) cannot be mistaken for the
# real key.
context_budget_is_always_loaded() {
    local file="$1"
    [ -f "$file" ] || return 1

    awk '
        # No frontmatter opener on line 1: always loaded, decided immediately.
        NR == 1 && $0 != "---" { exit 0 }
        NR == 1 { next }
        # Closing fence ends the block; whatever we learned inside is the answer.
        $0 == "---" { closed = 1; exit (have_real_pattern ? 1 : 0) }
        # A top-level `paths:` key, at column 0 and spelled exactly.
        /^paths:[[:space:]]*$/ { in_paths = 1; next }
        /^paths:[[:space:]]*\[/ {
            # Inline flow sequence, e.g. `paths: ["a/**", "b/**"]`.
            line = $0
            sub(/^paths:[[:space:]]*\[/, "", line)
            sub(/\].*$/, "", line)
            gsub(/[",[:space:]]/, "", line)
            if (line != "" && line != "**") have_real_pattern = 1
            next
        }
        # Any other top-level key closes the paths block.
        /^[^[:space:]-]/ { in_paths = 0 }
        # A list item under `paths:`. `**` alone does not count as scoping.
        in_paths && /^[[:space:]]*-[[:space:]]*/ {
            pattern = $0
            sub(/^[[:space:]]*-[[:space:]]*/, "", pattern)
            gsub(/^["'"'"']|["'"'"']$/, "", pattern)
            sub(/[[:space:]]*#.*$/, "", pattern)
            if (pattern != "" && pattern != "**") have_real_pattern = 1
        }
        # Unterminated frontmatter is not frontmatter: CC would not parse a
        # `paths:` out of it either, so the file is resident.
        END { if (!closed) exit 0; }
    ' "$file"
}

# context_budget_scan <repo-root>
# Print `<bytes>\t<repo-relative-path>` for every always-loaded instruction
# file, sorted largest first. Discovery is `git ls-files`, not a hand-written
# list, for the same reason scripts/lint-shell.sh uses it: a rule added anywhere
# is measured the day it is committed.
#
# Streams rather than collecting into an array, and that is not a style choice.
# `bash` here is macOS's system 3.2.57, where expanding an EMPTY array as
# `"${arr[@]}"` under `set -u` aborts with `arr[@]: unbound variable` (bash only
# stopped treating that as unset in 4.4). An empty candidate list is exactly the
# discovery-is-broken case the caller has a fail-closed message for, so the
# array form died with a confusing bash error one line before the actionable one
# could print. No array, no special case.
context_budget_scan() {
    local root="$1" file bytes

    # Pathspecs are a directory and a filename, with the `.md` filter applied
    # here: git's wildmatch treats `*` inconsistently across versions for a
    # nested `.claude/rules/**/*.md`, and rules are discovered recursively.
    git -C "$root" ls-files 'CLAUDE.md' '.claude/rules/' 2>/dev/null | {
        while IFS= read -r file; do
            case "$file" in
                *.md) ;;
                *) continue ;;
            esac
            # CLAUDE.md has no frontmatter contract; it is always resident.
            if [ "$file" = "CLAUDE.md" ] || context_budget_is_always_loaded "$root/$file"; then
                bytes="$(wc -c <"$root/$file" | tr -d ' ')"
                printf '%s\t%s\n' "$bytes" "$file"
            fi
        done
    } | sort -rn
}
