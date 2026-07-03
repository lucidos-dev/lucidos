#!/usr/bin/env bash
# Canonical private-data patterns — the SINGLE source of truth for the
# deterministic side of the no-private-data rule (see
# `.claude/rules/no-private-data.md`). Sourced by:
#   - scripts/release-to-lucidos.sh  (the fail-closed pre-publish guard)
#   - scripts/lib/release_scrub_guard_test.sh  (its hermetic test)
# Do NOT copy these regexes anywhere else — reference this file. The semantic
# (LLM) side of the same rule lives in the rule doc and is enforced by
# /harden + /harden-project; this file is only the mechanical backstop.
#
# Why two halves: a denylist catches today's KNOWN private tokens; the
# heuristics catch FUTURE slips of the same SHAPE (home paths, possessive
# device labels, the maintainer's email/path form). Both are ERE (`grep -E`)
# so they run on stock macOS git (no PCRE `-P` / lookahead dependency).

# --- Denylist: exact private/internal tokens (case-insensitive) -------------
# Unambiguously private — none has a plausible generic meaning in this repo.
# Deliberately NOT here: bare "Kenneth Tiller" (legitimate authorship/copyright
# attribution) and bare "akram" (legitimate GOVERNANCE.md contributor credit) —
# both handled below so attribution passes.
PRIVATE_DATA_DENYLIST_RE='nettbank|pappa|alf tiller|fullmakt|folgebrev|nav-skatt|jamfcloud|m10s|momentum-autoresearch|user-acquisition|ua-analysis|ua-backoffice|ost-jira|workspaces/(personal|work)'

# --- Heuristics: generalizable shapes that catch future leaks ---------------
# - possessive personal device labels ("Kenneth's MacBook", "Alice's iPhone").
#   Generic labels are apostrophe-free ("My MacBook", "Test iPhone") so they
#   never match.
# - the maintainer's name in non-attribution form: possessive ("Kenneth's"),
#   email/path ("kenneth.tiller", "/Users/kenneth/"). "Kenneth Tiller" (space)
#   does NOT match, so LICENSE/GOVERNANCE/etc. attribution passes.
PRIVATE_DATA_HEURISTIC_RE="[A-Za-z]+'s (MacBook|iPhone|iPad)|[Kk]enneth['./]"

# --- Real-home-path heuristic (filtered, not a plain match) -----------------
# Any `/Users/<name>` whose <name> is NOT one of the agreed generic
# placeholders. Applied as: match PRIVATE_DATA_HOMEPATH_RE, then drop lines
# matching PRIVATE_DATA_HOMEPATH_ALLOW_RE.
PRIVATE_DATA_HOMEPATH_RE='/Users/[A-Za-z0-9._@&-]+'
# Generic stand-ins used across the repo's tests/docs — anything else under
# /Users/ is treated as a real home dir and flagged. The trailing boundary is
# any non-alphanumeric (or EOL), so `k`/`dev` match only the exact username
# (NOT `ken`/`kenneth`, which stay caught) and `...`-then-backtick is allowed.
# `...` is the common anonymized-path placeholder (`/Users/.../foo`); `a&amp;b`
# is the xml-escaped form of `a&b` (an escaping test fixture).
PRIVATE_DATA_HOMEPATH_ALLOW_RE='/Users/(me|alex|x|u|k|dev|Anne|someone|a&b|a&amp;b|\.\.\.)([^A-Za-z0-9]|$)'

# --- Akram: denied everywhere EXCEPT the GOVERNANCE.md contributor credit ---
PRIVATE_DATA_AKRAM_RE='akram'
PRIVATE_DATA_AKRAM_ALLOW_PATHSPEC=':(exclude)GOVERNANCE.md'

# --- Self-exclude: the files that DEFINE the patterns legitimately contain the
# tokens (this denylist, the rule's prohibited-examples + placeholder table).
# They are scanned-out so the guard never flags its own definition. The rule
# file's prohibited examples ("Kenneth's MacBook", "/Users/kenneth/…",
# "momentum-autoresearch", …) would otherwise self-trip.
PRIVATE_DATA_SELF_EXCLUDE=(
  ':(exclude).claude/rules/no-private-data.md'
  ':(exclude)scripts/lib/private_data_patterns.sh'
  ':(exclude)scripts/lib/release_scrub_guard_test.sh'
)

# --- Generated / minified files: machine-produced, never a place intentional
# private data is authored — but exactly where short tokens (`m10s`, `pappa`,
# `ost-jira`) false-positive (base64 integrity hashes, minified identifiers).
# Excluded so a new dependency's lockfile hash can't fail-closed a release.
PRIVATE_DATA_GENERATED_EXCLUDE=(
  ':(exclude)package-lock.json'
  ':(exclude)**/package-lock.json'
  ':(exclude)Cargo.lock'
  ':(exclude)**/*.min.js'
  ':(exclude)**/*.min.css'
  ':(exclude)**/*.map'
)

# private_data_grep_tree <tree-ish> [repo-root]
# Print `path:line:content` for every private-data hit in the given git
# tree/commit (default repo root = `.`). Empty output == clean. Used by the
# release guard against the post-exclude release tree, and by the test against
# a fixture tree.
private_data_grep_tree() {
  local tree="$1" repo="${2:-.}"
  local excl=("${PRIVATE_DATA_SELF_EXCLUDE[@]}" "${PRIVATE_DATA_GENERATED_EXCLUDE[@]}")
  {
    git -C "$repo" grep -nIiE -e "$PRIVATE_DATA_DENYLIST_RE" "$tree" -- . "${excl[@]}" 2>/dev/null || true
    git -C "$repo" grep -nIiE -e "$PRIVATE_DATA_HEURISTIC_RE" "$tree" -- . "${excl[@]}" 2>/dev/null || true
    git -C "$repo" grep -nIiE -e "$PRIVATE_DATA_HOMEPATH_RE" "$tree" -- . "${excl[@]}" 2>/dev/null \
      | grep -vE "$PRIVATE_DATA_HOMEPATH_ALLOW_RE" || true
    git -C "$repo" grep -nIiE -e "$PRIVATE_DATA_AKRAM_RE" "$tree" -- . "$PRIVATE_DATA_AKRAM_ALLOW_PATHSPEC" "${excl[@]}" 2>/dev/null || true
  } | sort -u
}
