#!/bin/bash
# Tests for scripts/lib/private_data_patterns.sh — the deterministic private-data
# guard that scripts/release-to-lucidos.sh runs against the release tree before
# the irreversible public push.
#
# Hermetic in BOTH halves: it builds a throwaway git repo AND writes its own
# fixture token blocks with INVENTED names and tokens, pointing
# PRIVATE_DATA_DENYLIST_SOURCE at them. So this file names no real person and
# no real private token (either would re-create the leak the split exists to
# prevent), and its outcome cannot drift when the real WORKSPACES.md blocks
# change.
#
# Covered: planted leaks flagged (enumerated tokens AND novel same-shape
# slips, under BOTH home roots — `/Users/<name>` and `/home/<name>`), the
# exceptions list letting a name through at its attribution sites while still
# catching it everywhere else, approved placeholders NOT flagged, a real home
# path sharing a LINE with an approved placeholder still flagged (in either
# order), the shapes the home-path heuristic must NOT mistake for a home dir (a
# `/home` substring, a URL path, case-folded prose), and the loader failing
# closed on every malformed-block shape.
#
# Run: ./scripts/lib/release_scrub_guard_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/private_data_patterns.sh
source "$SCRIPT_DIR/private_data_patterns.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

REPO="$(mktemp -d)"
# Fixture token blocks live OUTSIDE the scanned repo — mirroring reality, where
# the guard reads WORKSPACES.md from the working checkout while scanning a
# release tree that has it stubbed. Inside the repo, the fixture's own token
# lines would be committed into the tree and flagged as leaks.
FIXTURES="$(mktemp -d)"
trap 'rm -rf "$REPO" "$FIXTURES"' EXIT
git -C "$REPO" init -q
git -C "$REPO" config user.email "t@t"
git -C "$REPO" config user.name "t"

write() { # <relpath> <content>
  mkdir -p "$REPO/$(dirname "$1")"
  printf '%s\n' "$2" > "$REPO/$1"
}

# ── Fixture token blocks ────────────────────────────────────────────────
# Invented stand-ins for every real category: a personal-document word, an
# employer domain, an internal project name, a live-workspace regex, and two
# names with attribution carve-outs. Same marker/fence/comment syntax as the
# real WORKSPACES.md blocks.
DENYLIST_FIXTURE="$FIXTURES/denylist-fixture.md"
cat > "$DENYLIST_FIXTURE" <<'FIXTURE'
Prose before the block is ignored (contoso-secret must not be picked up here).

<!-- BEGIN private-data-denylist -->
```
# a personal-document word
skattemelding

# an employer domain + an internal project
contoso\.example
widget-pipeline

# a live workspace name
workspaces/(homelab|office)
```
<!-- END private-data-denylist -->

<!-- BEGIN private-data-exceptions -->
```
# a maintainer name, legitimate only as project identity
quilliam => LICENSE GOVERNANCE.md README.md
# a contributor name, legitimate only in the credits file
zephyrina => GOVERNANCE.md
```
<!-- END private-data-exceptions -->

Prose after the block is ignored too.
FIXTURE
export PRIVATE_DATA_DENYLIST_SOURCE="$DENYLIST_FIXTURE"

# ── Legitimate — must NOT be flagged ────────────────────────────────────
write LICENSE 'Copyright (c) 2026 Quilliam Vance'                   # attribution site for `quilliam`
write GOVERNANCE.md 'Maintained by Quilliam Vance. Zephyrina has contributed.'  # both names, credits file
write clean.md 'App habit-tracker, repo example-repo, "My MacBook", path /Users/me/x, /Users/.../foo'
write clean_ws.md 'Use ~/workspaces/dev or ~/workspaces/myws for development.'
# The Linux placeholders. Kept apart from the two cases below — a boundary case
# is not a home-path match at ALL, so a placeholder sharing its line would
# satisfy the allow filter single-handedly and the case would pass for the
# wrong reason.
write clean_home_linux.md 'a path /home/user/src/main.rs
and a workspace /home/u/workspaces/myws'
# TWO approved placeholders on one line, under both roots. The allow test runs
# per OCCURRENCE, so this pins that a line is dropped only when EVERY home path
# on it is approved — the per-match logic must not trade a masked leak (below)
# for a false positive here.
write clean_home_placeholder_pair.md 'either /Users/me/x or /home/user/x works'
# The escaping fixture, in both the raw and the xml-escaped spelling (the shape
# `desktop.rs` asserts on). The escaped one is why a placeholder has to be
# spelled in the NAME character set: `;` is not one, so the name the matcher
# extracts is `a&amp`, and an `a&amp;b` allow token never matches it.
write clean_home_escaped.rs 'let p = Path::new("/Users/a&b/<x>/Lucidos");
assert_eq!(xml_escape(p), "/Users/a&amp;b/&lt;x&gt;/Lucidos");'
# Not home paths at all: `/home` as an inner path component, and a URL path.
# Both exist in the real tree (the desktop.rs fixtures, the Dropbox share URL),
# and a rootward `/home/…` match without a leading boundary hits both.
write clean_home_boundary.rs 'let app_data = Path::new("/fake/home/Library/Application Support/com.lucidos.app");
assert_eq!(url, "https://www.dropbox.com/home/Lucidos%20Backups");'
# Keycap prose, not a path — pins the home-path pass being case-EXACT. Folded,
# `/Home/End` reads as `/home/<name>` and refuses the release.
write clean_home_keycaps.tsx '// ↑/↓/Home/End rove focus across the menu items'

# ── Planted leaks — must be flagged ─────────────────────────────────────
write leak_token.md 'the skattemelding form, filed via portal.contoso.example'   # denylist (2 fragments)
write leak_app.ts "const id = 'widget-pipeline';"                                # denylist (internal project)
write leak_ws.sh 'WS=~/workspaces/homelab'                                       # denylist (regex fragment)
write leak_home.rs 'let p = "/Users/bobsmith/secret";'                           # heuristic: real macOS home dir
write leak_home_linux.rs 'let p = "/home/alicejones/secret";'                    # heuristic: real Linux home dir
# A placeholder must be the WHOLE username. `k.`/`user_` are real home dirs that
# merely START with one — the allow-list's trailing boundary has to reject every
# character a name can contain, not just alphanumerics.
write leak_home_dotted.rs 'let p = "/Users/k.thornbury/secret";'                 # heuristic: `k` + `.` is not the `k` placeholder
write leak_home_underscored.rs 'let p = "/home/user_thornbury/secret";'          # heuristic: `user` + `_` is not the `user` placeholder
# An approved placeholder and a REAL home dir on the SAME line — the shape doc
# comments and fixtures produce constantly ("here's the generic example, here's
# the real thing"). A line-wise `grep -v` allow filter drops the whole line, so
# the placeholder MASKS the leak sitting next to it and it ships. Both roots,
# and both orderings, because neither may decide the outcome.
write leak_home_mixed.md '// e.g. /Users/me/ws vs the real /Users/bergstrom/ws'   # placeholder first, macOS root
write leak_home_mixed_linux.rs '// /home/user/src mirrors /home/carolwren/src'    # placeholder first, Linux root
write leak_home_real_first.rs 'let p = "/Users/danvarga/ws"; // like /Users/me/ws' # real path first, placeholder second
write leak_device.ts "const label = \"Alice's iPhone\";"                         # heuristic: possessive device
write leak_maintainer.rs 'let p = "/Users/quilliam/ws";'                         # excepted name, NOT an attribution site
write leak_contributor.md 'follow-up from zephyrina on the parser'               # excepted name, NOT the credits file

git -C "$REPO" add -A
TREE="$(git -C "$REPO" write-tree)"

# Call the loader in THIS shell (not through the command substitution below,
# whose subshell would swallow the assignment) so the parsed value is
# inspectable.
echo "test: the denylist loads from the marker-fenced block"
if private_data_load_denylist "$DENYLIST_FIXTURE"; then
  if [ "$PRIVATE_DATA_DENYLIST_RE" = 'skattemelding|contoso\.example|widget-pipeline|workspaces/(homelab|office)' ]; then
    pass "tokens parsed in order, comments/blanks/fences dropped"
  else
    fail "unexpected denylist: $PRIVATE_DATA_DENYLIST_RE"
  fi
  if printf '%s' "$PRIVATE_DATA_DENYLIST_RE" | grep -q 'contoso-secret'; then
    fail "picked up a token from prose OUTSIDE the markers"
  else
    pass "prose outside the markers is ignored"
  fi
else
  fail "loader rejected the fixture denylist"
fi

if ! HITS="$(private_data_grep_tree "$TREE" "$REPO")"; then
  echo "  FAIL: private_data_grep_tree failed to run against the fixture tree" >&2
  exit 1
fi

flagged() { printf '%s\n' "$HITS" | grep -q "$1"; }

echo "test: planted leaks are flagged"
for f in leak_token.md leak_app.ts leak_ws.sh leak_home.rs leak_home_linux.rs \
  leak_home_dotted.rs leak_home_underscored.rs leak_home_mixed.md \
  leak_home_mixed_linux.rs leak_home_real_first.rs leak_device.ts \
  leak_maintainer.rs leak_contributor.md; do
  if flagged "$f"; then pass "flagged $f"; else fail "did NOT flag $f"; fi
done

echo "test: attribution sites + approved placeholders pass"
for f in LICENSE GOVERNANCE.md clean.md clean_ws.md clean_home_linux.md \
  clean_home_placeholder_pair.md clean_home_escaped.rs clean_home_boundary.rs \
  clean_home_keycaps.tsx; do
  if flagged "$f"; then
    fail "wrongly flagged $f → $(printf '%s\n' "$HITS" | grep "$f")"
  else
    pass "passed $f"
  fi
done

# The allow test runs against ONE extracted home path, so a placeholder is only
# ever compared with a whole home-dir NAME — a run of
# PRIVATE_DATA_HOMEPATH_NAME_CHARS. A placeholder spelled with anything outside
# that set can therefore never match, and silently stops allowing the fixture it
# was added for. That is exactly what `a&amp;b` was: the matcher stops at the
# `;`, sees `a&amp`, and the entry was dead.
echo "test: every approved placeholder is spellable as a whole home-dir name"
unspellable=''
IFS='|' read -r -a placeholders <<< "$PRIVATE_DATA_HOMEPATH_PLACEHOLDER_RE"
for p in "${placeholders[@]}"; do
  literal="${p//\\/}" # `\.\.\.` is the ERE spelling of the literal `...`
  printf '%s\n' "$literal" | grep -qE "^[$PRIVATE_DATA_HOMEPATH_NAME_CHARS]+\$" \
    || unspellable="$unspellable '$literal'"
done
if [ -z "$unspellable" ]; then
  pass "all ${#placeholders[@]} placeholders are made of name characters only"
else
  fail "placeholder(s) no home-dir name can equal:$unspellable"
fi

# The release guard prints these lines verbatim and release_tree.sh reads them
# as the hit list, so splitting a line into occurrences must not reshape it.
echo "test: a filtered home-path hit keeps its path:line:content shape"
mixed_row="$(printf '%s\n' "$HITS" | grep 'leak_home_mixed\.md')"
if [ "$mixed_row" = "$TREE:leak_home_mixed.md:1:// e.g. /Users/me/ws vs the real /Users/bergstrom/ws" ]; then
  pass "the mixed line survives filtering byte-for-byte"
else
  fail "the filter reshaped the hit row: $mixed_row"
fi

# A hit line yielding NO home-path occurrence cannot come out of the git grep
# that produced it — but if it ever did, "no occurrence" must not read as "every
# occurrence approved". Nothing proved the line clean, so it gets reported.
echo "test: the home-path filter fails closed on an unprovable hit line"
UNPROVABLE='sometree:some/file.md:1:no home path on this line'
if out="$(_private_data_homepath_allow_filter "$UNPROVABLE")" && [ "$out" = "$UNPROVABLE" ]; then
  pass "a hit line with no extractable home path is reported, not dropped"
else
  fail "an unprovable hit line was dropped (got: '${out:-}')"
fi

# Splitting a line into occurrences means feeding them to a second grep, and a
# grep that stops early (`-q`) closes that pipe while printf is still writing.
# Past the pipe buffer printf then dies of SIGPIPE, which `pipefail` — the
# release scripts' shell — reports as 141, and a status-checking filter reads
# that as a failed scan and refuses a releasable tree. The trigger is a REAL
# home path early on a very long line, since that is what makes the grep exit
# before EOF.
echo "test: a hit line longer than the pipe buffer does not abort the filter"
# shellcheck disable=SC2183 # no args on purpose: %.0s repeats a constant
LONG_LINE="sometree:some/file.json:1: /Users/bergstrom/x$(printf ' /Users/me/x%.0s' $(seq 1 8000))"
if long_out="$(set -o pipefail; _private_data_homepath_allow_filter "$LONG_LINE")" \
  && [ "$long_out" = "$LONG_LINE" ]; then
  pass "an 8001-occurrence line is filtered on content, not on a broken pipe"
else
  fail "a long hit line broke the filter (${#LONG_LINE} bytes, got ${#long_out} back)"
fi

# ── Fail-closed: blocks that can't be loaded must NOT read as "clean" ─────
# This is the whole point of moving the tokens out of tracked source: they now
# live in a file the guard reads at runtime, so "source missing/malformed"
# became a reachable state. Reporting no hits there would silently disarm the
# guard at the irreversible push.
#
# Each fixture below is malformed in exactly ONE way and valid in every other,
# so a case can't pass for the wrong reason. `valid_exceptions` supplies the
# well-formed second block the denylist cases need.
echo "test: the loader fails closed"

valid_exceptions() {
  printf '%s\n%s\n%s\n' \
    '<!-- BEGIN private-data-exceptions -->' \
    'somename => LICENSE' \
    '<!-- END private-data-exceptions -->'
}
valid_denylist() {
  printf '%s\n%s\n%s\n' \
    '<!-- BEGIN private-data-denylist -->' \
    'sometoken' \
    '<!-- END private-data-denylist -->'
}
# fails_closed <label> <fixture-path>
fails_closed() {
  local label="$1" fixture="$2" out
  PRIVATE_DATA_DENYLIST_SOURCE="$fixture"
  if out="$(private_data_grep_tree "$TREE" "$REPO" 2>/dev/null)"; then
    fail "$label exited 0 (output: '${out:0:60}')"
  else
    pass "$label exits non-zero"
  fi
}

fails_closed "missing denylist source" "$FIXTURES/does-not-exist.md"

printf '%s\n' 'No markers here at all.' > "$FIXTURES/no-markers.md"
fails_closed "source without markers" "$FIXTURES/no-markers.md"

{ printf '%s\n%s\n%s\n' '<!-- BEGIN private-data-denylist -->' '# only a comment' \
    '<!-- END private-data-denylist -->'; valid_exceptions; } > "$FIXTURES/empty-block.md"
fails_closed "empty denylist block" "$FIXTURES/empty-block.md"

# An unterminated block used to run the extractor to EOF, swallowing the
# surrounding prose as "tokens". A stray `[` or `(` in that prose then made the
# joined ERE invalid — and an invalid pattern makes every grep error out with
# EMPTY output, which the old `|| true` reported as clean. Both halves are
# regression-tested: the marker count here, the ERE validity below.
{ cat <<'UNTERMINATED'
<!-- BEGIN private-data-denylist -->
```
sometoken
```
Prose that follows, with a stray bracket [ and a dangling paren (
UNTERMINATED
  valid_exceptions; } > "$FIXTURES/unterminated.md"
fails_closed "unterminated denylist block" "$FIXTURES/unterminated.md"

{ printf '%s\n%s\n%s\n%s\n%s\n%s\n' \
    '<!-- BEGIN private-data-denylist -->' 'tokenone' '<!-- END private-data-denylist -->' \
    '<!-- BEGIN private-data-denylist -->' 'tokentwo' '<!-- END private-data-denylist -->'
  valid_exceptions; } > "$FIXTURES/dup-markers.md"
fails_closed "duplicated denylist blocks" "$FIXTURES/dup-markers.md"

{ printf '%s\n%s\n%s\n%s\n' \
    '<!-- BEGIN private-data-denylist -->' 'goodtoken' 'a(b' '<!-- END private-data-denylist -->'
  valid_exceptions; } > "$FIXTURES/bad-ere.md"
fails_closed "uncompilable denylist ERE" "$FIXTURES/bad-ere.md"

# The exceptions block is required to EXIST — deleting it must not silently
# drop every attribution-scoped name from the scan.
valid_denylist > "$FIXTURES/no-exceptions.md"
fails_closed "missing exceptions block" "$FIXTURES/no-exceptions.md"

{ valid_denylist
  printf '%s\n%s\n%s\n' '<!-- BEGIN private-data-exceptions -->' \
    'somename LICENSE' '<!-- END private-data-exceptions -->'; } > "$FIXTURES/bad-exception-line.md"
fails_closed "exception line without ' => '" "$FIXTURES/bad-exception-line.md"

{ valid_denylist
  printf '%s\n%s\n%s\n' '<!-- BEGIN private-data-exceptions -->' \
    'somename => ' '<!-- END private-data-exceptions -->'; } > "$FIXTURES/exception-no-paths.md"
fails_closed "exception line with no paths" "$FIXTURES/exception-no-paths.md"

{ valid_denylist
  printf '%s\n%s\n%s\n' '<!-- BEGIN private-data-exceptions -->' \
    'a(b => LICENSE' '<!-- END private-data-exceptions -->'; } > "$FIXTURES/bad-exception-ere.md"
fails_closed "uncompilable exception ERE" "$FIXTURES/bad-exception-ere.md"

# A grep pass that ERRORS (here: a tree-ish that doesn't exist) must not read
# as "no matches" either — that is the same empty-output-means-clean trap one
# layer down from the denylist.
PRIVATE_DATA_DENYLIST_SOURCE="$DENYLIST_FIXTURE"
if out="$(private_data_grep_tree "0000000000000000000000000000000000000000" "$REPO" 2>/dev/null)"; then
  fail "a failing git grep exited 0 (output: '${out:0:40}')"
else
  pass "a failing git grep exits non-zero"
fi

# An exceptions block with no entries is legitimate — a project may simply have
# no attribution carve-outs. It must LOAD, not fail.
{ valid_denylist
  printf '%s\n%s\n%s\n' '<!-- BEGIN private-data-exceptions -->' \
    '# no carve-outs yet' '<!-- END private-data-exceptions -->'; } > "$FIXTURES/empty-exceptions.md"
if private_data_load_denylist "$FIXTURES/empty-exceptions.md" \
  && [ "${#PRIVATE_DATA_EXCEPTION_TOKENS[@]}" -eq 0 ]; then
  pass "an empty exceptions block loads with zero carve-outs"
else
  fail "an empty exceptions block should load, not fail"
fi

# ── Under the release script's real shell setup ──────────────────────────────
# Everything above runs under this suite's plain `set -u`. The release scripts
# run under `set -Eeuo pipefail` with an ERR trap that EXITS, and that alone
# once inverted the guard: `-E` propagates the trap into the command
# substitution around `git grep`, rc=1 ("no matches" — the CLEAN case) fired it,
# and its `exit` killed the subshell before _private_data_git_grep could read
# the status. The failed capture then surfaced to the caller as "the denylist
# would not load", so a CLEAN tree refused the release — while this suite
# passed, because nothing here installed the trap.
#
# The clean tree is the same fixture minus the planted leaks, so a pass here
# means the scan RAN and found nothing, not that it never ran.
echo "test: the guard survives -Eeuo pipefail + an exiting ERR trap"
CLEAN_INDEX="$FIXTURES/clean.index"
GIT_INDEX_FILE="$CLEAN_INDEX" git -C "$REPO" read-tree "$TREE"
GIT_INDEX_FILE="$CLEAN_INDEX" git -C "$REPO" rm --cached -rq -- \
  leak_token.md leak_app.ts leak_ws.sh leak_home.rs leak_home_linux.rs \
  leak_home_dotted.rs leak_home_underscored.rs leak_home_mixed.md \
  leak_home_mixed_linux.rs leak_home_real_first.rs leak_device.ts \
  leak_maintainer.rs leak_contributor.md
CLEAN_TREE="$(GIT_INDEX_FILE="$CLEAN_INDEX" git -C "$REPO" write-tree)"

# grep_under_release_shell <tree> [denylist-source] — exit status of
# private_data_grep_tree in a shell set up exactly like the release scripts'.
# The trap is the raw exiting kind on purpose: the library has to be robust on
# its own, not only under a caller that remembered to gate its trap.
grep_under_release_shell() {
  local scan_tree="$1" src="${2:-$DENYLIST_FIXTURE}"
  PRIVATE_DATA_DENYLIST_SOURCE="$src" \
  LIB="$SCRIPT_DIR/private_data_patterns.sh" REPO="$REPO" TREE="$scan_tree" \
    bash -c '
      set -Eeuo pipefail
      on_err() { local ec=$?; trap - ERR; exit "$ec"; }
      trap on_err ERR
      # shellcheck source=/dev/null
      source "$LIB"
      private_data_grep_tree "$TREE" "$REPO"
    ' 2>/dev/null
}

if CLEAN_OUT="$(grep_under_release_shell "$CLEAN_TREE")"; then
  if [ -z "$CLEAN_OUT" ]; then
    pass "a clean tree reports clean (exit 0, no hits) under an exiting ERR trap"
  else
    fail "a clean tree reported hits under an exiting ERR trap: ${CLEAN_OUT:0:80}"
  fi
else
  fail "a clean tree exited non-zero under an exiting ERR trap (the guard is inverted)"
fi

# The two fail-closed arms must still refuse in that same shell, or a "fix" that
# blanket-swallowed the status would pass the case above with the guard disarmed.
if LEAKY_OUT="$(grep_under_release_shell "$TREE")"; then
  if printf '%s\n' "$LEAKY_OUT" | grep -q 'leak_token.md'; then
    pass "a planted leak is still flagged under an exiting ERR trap"
  else
    fail "a planted leak went unreported under an exiting ERR trap"
  fi
else
  fail "the leaky-tree scan could not run under an exiting ERR trap"
fi
if grep_under_release_shell "$CLEAN_TREE" "$FIXTURES/does-not-exist.md" >/dev/null; then
  fail "an unloadable denylist read as clean under an exiting ERR trap"
else
  pass "an unloadable denylist still refuses under an exiting ERR trap"
fi
if grep_under_release_shell "0000000000000000000000000000000000000000" >/dev/null; then
  fail "a failing git grep read as clean under an exiting ERR trap"
else
  pass "a failing git grep still exits non-zero under an exiting ERR trap"
fi

echo ""
echo "  ($PASS passed, $FAIL failed)"
[ "$FAIL" -eq 0 ]
