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
# Why two halves: enumerated TOKENS catch today's known private data; SHAPE
# heuristics catch future slips nobody has enumerated (home paths, possessive
# device labels). Both are ERE (`grep -E`) so they run on stock macOS git (no
# PCRE `-P` / lookahead dependency).
#
# NO NAME OR PRIVATE TOKEN APPEARS IN THIS FILE. Not a contributor's name, not
# an employer domain, not a family name — spelling any of them out would make
# the tracked, publicly-mirrored guard the very leak it exists to prevent. They
# all live in the one sanctioned place: the two marker-fenced blocks in
# WORKSPACES.md (internal-only; the release swaps its content for a public
# stub). This file holds the loaders, the shape heuristics, and nothing else.

# --- Denylist: enumerated private tokens (case-insensitive) -----------------
# Two blocks, two policies:
#   `private-data-denylist`   — denied everywhere.
#   `private-data-exceptions` — denied everywhere EXCEPT listed paths. This is
#     where legitimate project identity lives: a real name is fine as the
#     copyright holder / maintainer contact / contributor credit and nowhere
#     else, so the name is denied outright and its attribution sites are
#     enumerated. Narrowing the token instead (matching only the possessive or
#     path form) silently permits every other bare use.
PRIVATE_DATA_DENYLIST_MARKER_BEGIN='<!-- BEGIN private-data-denylist -->'
PRIVATE_DATA_DENYLIST_MARKER_END='<!-- END private-data-denylist -->'
PRIVATE_DATA_EXCEPTIONS_MARKER_BEGIN='<!-- BEGIN private-data-exceptions -->'
PRIVATE_DATA_EXCEPTIONS_MARKER_END='<!-- END private-data-exceptions -->'
PRIVATE_DATA_DENYLIST_RE=''
# Parallel arrays: token i is allowed only in the paths of entry i.
PRIVATE_DATA_EXCEPTION_TOKENS=()
PRIVATE_DATA_EXCEPTION_PATHS=()

# Assert `src` delimits exactly one block with the given markers. Counting both
# (rather than trusting the extractor) is what catches an unterminated block —
# see the fail-closed note on `private_data_load_denylist`.
_private_data_check_markers() { # <src> <begin> <end> <label>
  local src="$1" begin="$2" end="$3" label="$4" begins ends
  begins="$(grep -Fxc -- "$begin" "$src" || true)"
  ends="$(grep -Fxc -- "$end" "$src" || true)"
  if [ "$begins" != 1 ] || [ "$ends" != 1 ]; then
    echo "private-data guard: $src must contain exactly one $label block" >&2
    echo "  found $begins begin marker(s) and $ends end marker(s); expected 1 and 1:" >&2
    echo "    $begin … $end" >&2
    return 1
  fi
}

# Print the meaningful lines between two markers: fence, `#` comment and blank
# lines dropped, surrounding whitespace trimmed.
_private_data_block_lines() { # <src> <begin> <end>
  awk -v b="$2" -v e="$3" '
    $0 == b { inblock = 1; next }
    $0 == e { inblock = 0; next }
    !inblock { next }
    /^[[:space:]]*```/ { next }
    /^[[:space:]]*#/ { next }
    /^[[:space:]]*$/ { next }
    { gsub(/^[[:space:]]+|[[:space:]]+$/, ""); print }
  ' "$1"
}

# True when `re` compiles as an ERE. `grep` exits 1 on a valid pattern that
# doesn't match and >1 on a pattern it can't compile.
_private_data_valid_ere() { # <re>
  printf '' | grep -qE -- "$1" 2>/dev/null
  [ "$?" -le 1 ]
}

# private_data_load_denylist [source-file]
# Populate PRIVATE_DATA_DENYLIST_RE (denied everywhere) and the parallel
# PRIVATE_DATA_EXCEPTION_TOKENS / _PATHS arrays (denied except at listed
# paths) from the two marker-fenced blocks in `source-file` (default:
# `$PRIVATE_DATA_DENYLIST_SOURCE`, else `<repo-root>/WORKSPACES.md`).
#
# FAILS CLOSED: returns non-zero with a diagnostic on stderr for EVERY way the
# blocks can be wrong — source missing/unreadable, either block's markers
# absent or duplicated, no denied-everywhere tokens, a malformed exception
# line, or tokens that don't assemble into a valid ERE. Callers must not treat
# any of those as "clean": a silently empty or uncompilable denylist would
# disarm the guard at exactly the irreversible moment it exists for.
#
# The unterminated-block case is why marker COUNTS are checked rather than just
# "did we get a token". With the begin marker present and the end marker
# deleted, the extractor runs to EOF and swallows the surrounding prose as
# tokens; a stray `[` or `(` in that prose makes the joined ERE invalid, and an
# invalid pattern makes every later `grep` error out — which, before the
# status-aware wrapper below, read as "no hits found".
private_data_load_denylist() {
  local src="${1:-${PRIVATE_DATA_DENYLIST_SOURCE:-}}"
  if [ -z "$src" ]; then
    echo "private_data_load_denylist: no denylist source given" >&2
    return 1
  fi
  if [ ! -r "$src" ]; then
    echo "private_data_load_denylist: denylist source not readable: $src" >&2
    return 1
  fi

  _private_data_check_markers "$src" \
    "$PRIVATE_DATA_DENYLIST_MARKER_BEGIN" "$PRIVATE_DATA_DENYLIST_MARKER_END" denylist || return 1
  _private_data_check_markers "$src" \
    "$PRIVATE_DATA_EXCEPTIONS_MARKER_BEGIN" "$PRIVATE_DATA_EXCEPTIONS_MARKER_END" exceptions || return 1

  local tokens
  tokens="$(
    _private_data_block_lines "$src" \
      "$PRIVATE_DATA_DENYLIST_MARKER_BEGIN" "$PRIVATE_DATA_DENYLIST_MARKER_END" | paste -sd '|' -
  )"
  if [ -z "$tokens" ]; then
    echo "private_data_load_denylist: no tokens in the denylist block of $src" >&2
    return 1
  fi
  # An ERE that won't compile matches nothing, which is indistinguishable from
  # "clean" downstream. Reject it here, where we can say why.
  if ! _private_data_valid_ere "$tokens"; then
    echo "private_data_load_denylist: denylist tokens in $src do not form a valid ERE:" >&2
    echo "  $tokens" >&2
    return 1
  fi

  # Exceptions: `<ERE token> => <space-separated allowed paths>`. An empty
  # exceptions block is legitimate (no attribution carve-outs); a malformed or
  # uncompilable line is not.
  PRIVATE_DATA_EXCEPTION_TOKENS=()
  PRIVATE_DATA_EXCEPTION_PATHS=()
  local line token paths
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    case "$line" in
      *" => "*) ;;
      *)
        echo "private_data_load_denylist: malformed exception line in $src" >&2
        echo "  expected '<token> => <paths>', got: $line" >&2
        return 1
        ;;
    esac
    token="${line%% => *}"
    paths="${line#* => }"
    if [ -z "$token" ] || [ -z "$paths" ]; then
      echo "private_data_load_denylist: exception line needs both a token and paths in $src" >&2
      echo "  got: $line" >&2
      return 1
    fi
    if ! _private_data_valid_ere "$token"; then
      echo "private_data_load_denylist: exception token in $src is not a valid ERE: $token" >&2
      return 1
    fi
    PRIVATE_DATA_EXCEPTION_TOKENS+=("$token")
    PRIVATE_DATA_EXCEPTION_PATHS+=("$paths")
  done < <(_private_data_block_lines "$src" \
    "$PRIVATE_DATA_EXCEPTIONS_MARKER_BEGIN" "$PRIVATE_DATA_EXCEPTIONS_MARKER_END")

  PRIVATE_DATA_DENYLIST_RE="$tokens"
}

# --- Heuristics: generalizable shapes that catch future leaks ---------------
# - possessive personal device labels — a real first name followed by `'s` and
#   a device ("<Name>'s MacBook", "<Name>'s iPhone"). Generic labels are
#   apostrophe-free ("My MacBook", "Test iPhone") so they never match.
# Contributor names are NOT here — a name is an enumerated token, not a shape,
# so it lives in WORKSPACES.md (the exceptions block, which also enumerates
# where that name is legitimate attribution).
PRIVATE_DATA_HEURISTIC_RE="[A-Za-z]+'s (MacBook|iPhone|iPad)"

# --- Real-home-path heuristic (filtered, not a plain match) -----------------
# Any `/Users/<name>` or `/home/<name>` whose <name> is NOT one of the agreed
# generic placeholders. Both roots, because a home dir name carries the same
# identity on either OS — a Linux `/home/<realname>` leaks a person or an
# employer domain exactly as a macOS `/Users/<realname>` does. Applied as:
# match PRIVATE_DATA_HOMEPATH_RE, then drop the lines on which EVERY match is
# an approved placeholder — per OCCURRENCE, never per line; see
# `_private_data_homepath_allow_filter`.
PRIVATE_DATA_HOMEPATH_ROOT_RE='/(Users|home)/'
# A real home path STARTS a path token, so the character before the root must
# be start-of-line or one that does not read as the tail of a path component.
# Without it, `/home` matches as a substring of legitimate strings already in
# the tree: the `/fake/home/Library/…` desktop fixtures and the
# `https://www.dropbox.com/home/…` URL. Deliberately WIDER than the name set
# below — `@` and `&` count as boundaries here even though a name may contain
# them, because in practice they delimit (`user@host:/home/…`). Erring wide
# over-matches, which the allow filter then narrows; erring narrow would MISS.
PRIVATE_DATA_HOMEPATH_BOUNDARY_RE='(^|[^A-Za-z0-9._-])'
# The character set a home-dir NAME is made of. Used POSITIVELY by the match
# regex and NEGATED by the allow-list's trailing boundary, from this one
# definition — an approved placeholder has to be the WHOLE name, never a prefix
# of one. Drift between the two is a silent hole: with the boundary at merely
# `[^A-Za-z0-9]`, the `k` placeholder swallowed `/Users/k.<surname>` and `user`
# swallowed `/home/user_<surname>` — real home dirs, dropped line and all.
# Every placeholder below must therefore be SPELLED in this set: one containing
# anything else can never equal a whole name, so it is dead as an allow token
# (the test asserts the two stay in step).
PRIVATE_DATA_HOMEPATH_NAME_CHARS='A-Za-z0-9._@&-'
# Generic stand-ins used across the repo's tests/docs — anything else under a
# home root is treated as a real home dir and flagged. The trailing boundary is
# any non-name character (or EOL), so a short placeholder like `k`/`dev` matches
# only the exact username, and `...`-then-backtick is allowed. `...` is the
# common anonymized-path placeholder (`/Users/.../foo`); `a&amp` is how the
# xml-escaped `a&b` escaping fixture reads to the matcher — `;` is not a name
# character, so the name ENDS there and the full `a&amp;b` would never be a
# whole name. `Anne` pairs with the `/Users/Anne Doe` shell-quoting fixture — a
# Jane-Doe stand-in, not a real person. `u` and `user` are the Linux stand-ins
# (`/home/u`, `/home/user`). One list, interpolated into an allow-regex covering
# both roots — a per-root copy would drift.
PRIVATE_DATA_HOMEPATH_PLACEHOLDER_RE='me|alex|x|u|k|dev|user|Anne|someone|a&b|a&amp|\.\.\.'
PRIVATE_DATA_HOMEPATH_RE="${PRIVATE_DATA_HOMEPATH_BOUNDARY_RE}${PRIVATE_DATA_HOMEPATH_ROOT_RE}[${PRIVATE_DATA_HOMEPATH_NAME_CHARS}]+"
PRIVATE_DATA_HOMEPATH_ALLOW_RE="${PRIVATE_DATA_HOMEPATH_BOUNDARY_RE}${PRIVATE_DATA_HOMEPATH_ROOT_RE}(${PRIVATE_DATA_HOMEPATH_PLACEHOLDER_RE})([^${PRIVATE_DATA_HOMEPATH_NAME_CHARS}]|\$)"

# --- Self-exclude: the files that DEFINE the patterns legitimately contain
# matching text. They are scanned-out so the guard never flags its own
# definition — the device-label heuristic above and the rule doc's illustrative
# examples would otherwise self-trip. Note WORKSPACES.md is NOT listed: the
# release stubs its content, so both token blocks are already gone from the
# tree the guard scans.
PRIVATE_DATA_SELF_EXCLUDE=(
  ':(exclude).claude/rules/no-private-data.md'
  ':(exclude)scripts/lib/private_data_patterns.sh'
  ':(exclude)scripts/lib/release_scrub_guard_test.sh'
)

# --- Generated / minified files: machine-produced, never a place intentional
# private data is authored — but exactly where the SHORT denylist tokens
# false-positive (base64 integrity hashes, minified identifiers).
# Excluded so a new dependency's lockfile hash can't fail-closed a release.
PRIVATE_DATA_GENERATED_EXCLUDE=(
  ':(exclude)package-lock.json'
  ':(exclude)**/package-lock.json'
  ':(exclude)Cargo.lock'
  ':(exclude)**/*.min.js'
  ':(exclude)**/*.min.css'
  ':(exclude)**/*.map'
)

# _private_data_git_grep <repo> <tree> <case-mode> <regex> <pathspec...>
# One `git grep` pass, with the exit status read honestly: 0 = matches (print
# them), 1 = no matches (print nothing), anything else = the scan FAILED and
# the caller must not proceed.
#
# The distinction matters because the >1 cases — an uncompilable pattern, a bad
# tree-ish, an unreadable object — all produce EMPTY output. Collapsing them
# into `|| true`, as this did before, turns a broken scan into a clean bill of
# health right before an irreversible force-push.
#
# <case-mode> is `fold` or `exact`, and it is per-pattern on purpose. Names,
# domains and device labels FOLD — they can legitimately be typed in any case.
# The home-path roots are EXACT: `/Users` and `/home` are canonical OS
# spellings, and folding them both invents hits (prose like `↑/↓/Home/End`
# reads as a `/home/<name>` path) and desynchronises the scan from its
# case-sensitive allow filter, so `/users/me/x` would be flagged while
# `/Users/me/x` is not.
_private_data_git_grep() {
  local repo="$1" tree="$2" mode="$3" re="$4"
  shift 4
  local flags
  case "$mode" in
    fold) flags=(-nIiE) ;;
    exact) flags=(-nIE) ;;
    *)
      echo "_private_data_git_grep: case-mode must be 'fold' or 'exact', got: $mode" >&2
      return 1
      ;;
  esac
  # `rc`, not `status` — the latter is a read-only special variable in zsh, and
  # this library is sourced ad hoc from interactive shells as well as by the
  # bash release script.
  #
  # stderr goes to its own file rather than `2>&1`: merging it would put any
  # benign git warning into the HIT LIST, where the release guard reads every
  # line as private data found. It's only wanted for the failure diagnostic.
  local out rc errfile
  errfile="$(mktemp -t private-data-grep)"
  # The capture MUST be the condition of an `if`, not a bare assignment followed
  # by `rc=$?`. rc=1 ("no matches") is the CLEAN case, but a bare assignment
  # makes that a failing command — and release.sh runs under `set -Eeuo pipefail`
  # with an ERR trap that EXITS. `-E` propagates that trap into this command
  # substitution, where it fires on rc=1 and kills the subshell BEFORE the case
  # below is reached. The caller's `|| return 1` cannot protect it: errexit/ERR
  # suppression from an enclosing guard does not reach a command substitution's
  # own ERR trap. The assignment then reads as a failure, and the only thing
  # `release_tree_scan` can conclude is "the denylist would not load" — so a
  # CLEAN tree aborts the release, guard inverted. An `if` condition suppresses
  # both errexit and the ERR trap in the same shell as the failing command,
  # which is the only placement that works.
  if out="$(git -C "$repo" grep "${flags[@]}" -e "$re" "$tree" -- "$@" 2>"$errfile")"; then
    rc=0
  else
    rc=$?
  fi
  case "$rc" in
    0) printf '%s\n' "$out" ;;
    1) : ;; # no matches — the clean case
    *)
      echo "private_data_grep_tree: git grep failed (status $rc) for pattern:" >&2
      echo "  $re" >&2
      sed 's/^/  /' "$errfile" >&2
      rm -f "$errfile"
      return 1
      ;;
  esac
  rm -f "$errfile"
}

# _private_data_homepath_allow_filter <hits>
# Given the raw home-path hit lines, print the ones that still carry at least
# one home path which is NOT an approved generic placeholder. Line format is
# passed through untouched — the release guard prints these verbatim and
# `release_tree.sh` reads them as the hit list.
#
# PER OCCURRENCE, never per line. A line-wise `grep -vE "$ALLOW"` drops the
# WHOLE line, so a line carrying an approved placeholder NEXT TO a real home
# path — `/Users/me/ws vs the real /Users/<name>/ws`, the shape doc comments
# and fixtures produce constantly — was discarded entirely and the real leak
# shipped. Splitting the line into its individual matches first is what makes
# the placeholder unable to vouch for anything but itself.
#
# The two regexes line up on their own: an occurrence extracted by `grep -oE`
# carries exactly ONE leading boundary character (or none at line start), which
# is what the allow regex's identical leading boundary expects, and the greedy
# name-character run means the occurrence ENDS at the name, where the allow
# regex's trailing `([^name]|$)` finds end-of-string. Both greps are case-EXACT,
# matching the `exact` mode of the pass that produced these hits.
#
# Fail-closed like everything else here: a grep that ERRORS aborts the whole
# filter (a short hit list would read as a cleaner tree than was scanned), and a
# hit line that yields no occurrence at all — which cannot happen for a line
# `git grep` just matched — is REPORTED rather than dropped, since nothing
# proved it clean.
_private_data_homepath_allow_filter() {
  local hits="$1"
  [ -n "$hits" ] || return 0
  local line occurrences rc
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    # `if`, not a bare assignment — same reason as in _private_data_git_grep:
    # under the release scripts' `set -Eeuo pipefail` + exiting ERR trap, a
    # non-zero anywhere else here kills this command substitution outright.
    if occurrences="$(printf '%s\n' "$line" | grep -oE -e "$PRIVATE_DATA_HOMEPATH_RE")"; then
      rc=0
    else
      rc=$?
    fi
    case "$rc" in
      0) ;;
      1) printf '%s\n' "$line"; continue ;; # unprovable ⇒ report it
      *)
        echo "private_data_grep_tree: home-path re-scan failed (status $rc) on hit:" >&2
        echo "  $line" >&2
        return 1
        ;;
    esac
    # `-v` to /dev/null, NOT `-q`: rc 0 = some occurrence is NOT an approved
    # placeholder (a real home dir, keep the line), rc 1 = every one of them is
    # (drop it). `-q` would exit at the FIRST such occurrence, closing the pipe
    # while `printf` is still writing — and on a line with more occurrences than
    # the pipe buffer holds, printf then dies of SIGPIPE and `pipefail` (the
    # release scripts' shell) reports 141, which the status check below would
    # read as a failed scan and abort the release. Reading to EOF costs nothing
    # and cannot race.
    if printf '%s\n' "$occurrences" | grep -vE -e "$PRIVATE_DATA_HOMEPATH_ALLOW_RE" >/dev/null; then
      rc=0
    else
      rc=$?
    fi
    case "$rc" in
      0) printf '%s\n' "$line" ;;
      1) ;;
      *)
        echo "private_data_grep_tree: home-path allow test failed (status $rc) on hit:" >&2
        echo "  $line" >&2
        return 1
        ;;
    esac
  done <<EOF
$hits
EOF
}

# private_data_grep_tree <tree-ish> [repo-root]
# Print `path:line:content` for every private-data hit in the given git
# tree/commit (default repo root = `.`). Used by the release guard against the
# post-exclude release tree, and by the test against a fixture tree.
#
# Exit status is load-bearing: 0 means the scan RAN (empty output == clean),
# non-zero means it could NOT run — the denylist failed to load, or a grep pass
# errored. A caller that only checks for non-empty output would read either
# failure as "clean", so check the status too.
private_data_grep_tree() {
  local tree="$1" repo="${2:-.}"
  private_data_load_denylist "${PRIVATE_DATA_DENYLIST_SOURCE:-$repo/WORKSPACES.md}" || return 1
  local excl=("${PRIVATE_DATA_SELF_EXCLUDE[@]}" "${PRIVATE_DATA_GENERATED_EXCLUDE[@]}")

  # Collected before printing so a mid-scan failure aborts the whole function
  # instead of emitting a partial (and therefore falsely short) hit list.
  local denylist_hits heuristic_hits homepath_hits exception_hits=''
  denylist_hits="$(_private_data_git_grep "$repo" "$tree" fold "$PRIVATE_DATA_DENYLIST_RE" . "${excl[@]}")" || return 1
  heuristic_hits="$(_private_data_git_grep "$repo" "$tree" fold "$PRIVATE_DATA_HEURISTIC_RE" . "${excl[@]}")" || return 1
  homepath_hits="$(_private_data_git_grep "$repo" "$tree" exact "$PRIVATE_DATA_HOMEPATH_RE" . "${excl[@]}")" || return 1

  # One pass per excepted token, each with its own attribution sites excluded —
  # they can't join the single denylist ERE because the exclusions are
  # per-token. Word-split `paths` deliberately: the block stores them
  # space-separated.
  local i token paths pathspecs one hits
  for i in "${!PRIVATE_DATA_EXCEPTION_TOKENS[@]}"; do
    token="${PRIVATE_DATA_EXCEPTION_TOKENS[$i]}"
    paths="${PRIVATE_DATA_EXCEPTION_PATHS[$i]}"
    pathspecs=()
    # shellcheck disable=SC2086 # intentional split on the space-separated list
    for one in $paths; do pathspecs+=(":(exclude)$one"); done
    hits="$(_private_data_git_grep "$repo" "$tree" fold "$token" . "${pathspecs[@]}" "${excl[@]}")" || return 1
    [ -n "$hits" ] && exception_hits="${exception_hits}${hits}"$'\n'
  done

  # The home-path heuristic is match-then-filter: drop the lines on which EVERY
  # home path is an approved generic placeholder, keep the rest.
  homepath_hits="$(_private_data_homepath_allow_filter "$homepath_hits")" || return 1

  {
    [ -n "$denylist_hits" ] && printf '%s\n' "$denylist_hits"
    [ -n "$heuristic_hits" ] && printf '%s\n' "$heuristic_hits"
    [ -n "$homepath_hits" ] && printf '%s\n' "$homepath_hits"
    [ -n "$exception_hits" ] && printf '%s' "$exception_hits"
    true
  } | sort -u
}
