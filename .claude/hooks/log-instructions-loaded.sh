#!/usr/bin/env bash
#
# InstructionsLoaded hook: record which instruction file Claude Code pulled into
# context, and why. Append-only JSONL at
# `<project>/.lucidos/instructions-loaded.jsonl` (gitignored).
#
# WHY THIS EXISTS. Path-scoped rules are the whole reason this repo's rule set
# is affordable: eight of thirteen rules are gated on `paths:` and only load
# when Claude reads a matching file. That saving is INVISIBLE from inside a
# session. When it silently stopped working, nobody noticed for weeks: every
# rule used the `globs:` key (a Cursor convention Claude Code ignores) until
# 2026-07-25, so the entire set was resident in every session. This log is the
# standing evidence, so the next regression is a diff instead of a discovery.
#
# It complements `scripts/check-context-budget.sh`, which reads the tree and
# says what SHOULD load. This reads a live session and says what DID. Upstream
# reports of `paths:` leaking through git worktree resolution
# (claude-code#23569) make the difference worth having, since every coding-agent
# session here runs in a worktree.
#
# SIDE EFFECT ONLY, BY DESIGN. Claude Code gives this event no decision control
# and ignores its exit code, and its stdout does not enter Claude's context, so
# nothing here can block a load, alter one, or spend a token. That is exactly
# what an audit wants: it cannot change the thing it measures. Always exits 0.
#
# The event-specific input fields are not in the published docs, so this logs
# the payload VERBATIM rather than parsing keys that may not exist. Pin the
# field names off a real session first, then narrow this if it is ever worth it.
# The matcher, which the settings file leaves open, filters on load reason
# (`session_start`, `nested_traversal`, `path_glob_match`, `include`, `compact`)
# and every reason is worth recording.

set -u

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
LOG_DIR="$PROJECT_DIR/.lucidos"
LOG="$LOG_DIR/instructions-loaded.jsonl"

# Read the payload before anything can fail: an unread stdin can wedge the
# writer on a full pipe buffer.
PAYLOAD="$(cat 2>/dev/null)"

mkdir -p "$LOG_DIR" 2>/dev/null || exit 0

# Cheap size cap. This fires on every lazy rule load in every session across
# every worktree, so an unbounded log quietly eats a workspace. Rotate at ~4 MB,
# keeping one previous generation.
if [ -f "$LOG" ]; then
    SIZE="$(wc -c <"$LOG" 2>/dev/null | tr -d ' ')"
    if [ -n "$SIZE" ] && [ "$SIZE" -gt 4194304 ]; then
        mv -f "$LOG" "$LOG.1" 2>/dev/null || true
    fi
fi

# One record per line: the wall-clock stamp this event does not carry, plus the
# payload verbatim. Written with a single `>>` so concurrent sessions interleave
# whole lines rather than fragments.
#
# Literal newlines are stripped so the file is real JSONL. A pretty-printed
# payload would otherwise span lines and break any reader that parses one object
# per line, which is the only reason this file exists in this format. Stripping
# is lossless for JSON: a newline INSIDE a string must be escaped as the two
# characters `\` and `n`, which survive, and a raw newline between tokens is
# only whitespace.
printf '{"at":"%s","payload":%s}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "$(printf '%s' "${PAYLOAD:-null}" | tr -d '\n\r')" >>"$LOG" 2>/dev/null || true

exit 0
