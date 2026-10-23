#!/usr/bin/env bash
# Canonical mirrored-rule measurement: the SINGLE source of truth for which
# rules are deliberately stated on BOTH instruction surfaces, and what proves
# each copy is still there. Sourced by:
#   - scripts/check-prompt-mirror.sh          (the gate, /harden Phase 4.5)
#   - scripts/lib/prompt_mirror_scan_test.sh  (its test)
# Do NOT restate the needle or the file list anywhere else, reference this file.
#
# WHY A MIRROR EXISTS AT ALL. Two instruction surfaces reach a coding-agent
# session unconditionally: the engine system prompt
# (crates/lucidos-engine/src/engine/agent_session/prompts.rs, system-prompt
# authority) and the always-loaded set (CLAUDE.md plus the unscoped rules, a
# user message). `docs/agent-config.md` § Which surface owns a rule splits them
# by session truth versus repo truth, and the normal case is that a rule lands
# on exactly one. Stating it twice is what this repo already did by accident for
# six rules: each was paid twice on every request and each was free to drift out
# of agreement with its twin.
#
# Exactly ONE rule cannot be written once, because no single surface reaches
# everyone it binds:
#
#   - The engine prompt is the only surface reaching a session with NO Lucidos
#     checkout. Four of the seven prompt flavors are in that position
#     (external_repo, app_worktree, and both recovery variants), and a broad
#     kill from any of them destroys every other workspace's engine just the
#     same.
#   - CLAUDE.md is the only surface reaching a hand-run `claude` in this repo,
#     which gets no engine prompt. That session IS gated by
#     `.claude/hooks/pre-kill.sh`, registered in the repo's own
#     `.claude/settings.json`, so the prohibition explaining the refusal has to
#     be reachable from CLAUDE.md.
#
# That rule is the process-safety prohibition (ADR 0025). The engine prompt
# carries the full text, CLAUDE.md carries a one-line prohibition, and this gate
# fails if either half goes missing.
#
# WHY SHELL AND NOT A RUST TEST. The failure mode is a CLAUDE.md-only edit,
# which never triggers `cargo test`. `/harden` Phase 4.5 runs this for every
# diff including docs-only, which is precisely the diff that can break it. It
# also reaches Codex, which has no PreToolUse hooks.
#
# WHAT THIS CHECKS, AND WHAT IT DOES NOT. It checks that each surface still
# carries the prohibition's vocabulary (the tokens below) and still phrases it
# as a prohibition (a negation near the `pkill` mention). It does NOT check that
# the two copies say the same thing in the same words, deliberately: the engine
# copy is a full paragraph and the CLAUDE.md copy is one line, and pinning exact
# wording would force churn on every reword while catching nothing real. Same
# posture as check-em-dashes.sh, which checks characters rather than meaning.
#
# ADDING A SECOND MIRROR needs the proof spelled out in
# `docs/agent-config.md` § The one sanctioned mirror: name the populations the
# rule binds, show no single surface covers them, and extend this file in the
# same change. Duplicated prose with no entry here is drift, not a mirror.

# The two surfaces, relative to the repo root.
PROMPT_MIRROR_FILES=(
    "crates/lucidos-engine/src/engine/agent_session/prompts.rs"
    "CLAUDE.md"
)

# The command the negation has to sit next to, and the token the phrasing check
# anchors on. Named once and shared with the required list below, so the two
# cannot drift into checking different words.
PROMPT_MIRROR_ANCHOR_TOKEN="pkill"

# Vocabulary the prohibition cannot lose without changing meaning. Both kill
# commands are named because the anchor alone would pass a copy that dropped the
# `killall` half, and macOS `pkill` excluding ancestors is exactly why the
# calling engine survives while every other workspace dies.
PROMPT_MIRROR_REQUIRED_TOKENS=(
    "$PROMPT_MIRROR_ANCHOR_TOKEN"
    "killall"
    "lucidos-engine"
)

# Printed verbatim by the gate so the fix is always spelled the same way.
# shellcheck disable=SC2034 # printed by scripts/check-prompt-mirror.sh
PROMPT_MIRROR_ADVICE='The process-safety prohibition (ADR 0025) is the one rule deliberately stated on both instruction surfaces, because neither alone reaches every session it binds. Restore the missing half rather than deleting the other. See docs/agent-config.md, section "The one sanctioned mirror".'

# How many lines either side of a `pkill` mention count as "near" when looking
# for the negation. The engine copy is a Rust string literal split across
# continuation lines, so "NEVER use" and "pkill" land on DIFFERENT source lines;
# a single-line grep would report a missing prohibition that is plainly there.
PROMPT_MIRROR_WINDOW=3

# prompt_mirror_missing_tokens <file>
# Print each required token the file does not contain, one per line. Empty
# output means every token is present.
prompt_mirror_missing_tokens() {
    local file="$1" token
    [ -f "$file" ] || return 0
    for token in "${PROMPT_MIRROR_REQUIRED_TOKENS[@]}"; do
        grep -qF -- "$token" "$file" || printf '%s\n' "$token"
    done
}

# prompt_mirror_has_prohibition <file>
# True when some mention of the anchor token has a negation within
# PROMPT_MIRROR_WINDOW lines of it, i.e. the text still forbids the thing rather
# than merely naming it. Matching "never" case-insensitively covers both the
# engine's shouted "NEVER use" and ordinary prose.
prompt_mirror_has_prohibition() {
    local file="$1"
    [ -f "$file" ] || return 1
    awk -v window="$PROMPT_MIRROR_WINDOW" -v anchor="$PROMPT_MIRROR_ANCHOR_TOKEN" '
        { lines[NR] = $0 }
        END {
            for (i = 1; i <= NR; i++) {
                if (index(lines[i], anchor) == 0) continue
                lo = i - window; if (lo < 1) lo = 1
                hi = i + window; if (hi > NR) hi = NR
                text = ""
                for (j = lo; j <= hi; j++) text = text " " lines[j]
                if (tolower(text) ~ /never/) exit 0
            }
            exit 1
        }
    ' "$file"
}

# prompt_mirror_scan <repo-root>
# Print one tab-separated verdict per mirrored surface:
#   ok      <file>
#   absent  <file>   (the file itself is gone or unreadable)
#   tokens  <file>   <comma-separated missing tokens>
#   phrase  <file>   (tokens present, but nothing forbids it any more)
# Exit status is always 0: the caller decides what a non-ok row means, the same
# split context_budget_scan uses.
prompt_mirror_scan() {
    local root="$1" file missing

    for file in "${PROMPT_MIRROR_FILES[@]}"; do
        if [ ! -f "$root/$file" ]; then
            printf 'absent\t%s\n' "$file"
            continue
        fi
        missing="$(prompt_mirror_missing_tokens "$root/$file" | paste -sd, - | tr -d ' ')"
        if [ -n "$missing" ]; then
            printf 'tokens\t%s\t%s\n' "$file" "$missing"
            continue
        fi
        if ! prompt_mirror_has_prohibition "$root/$file"; then
            printf 'phrase\t%s\n' "$file"
            continue
        fi
        printf 'ok\t%s\n' "$file"
    done
}
