#!/bin/bash
# Tests for scripts/merge-drivers.sh and scripts/lib/merge_drivers.sh.
#
# Hermetic: every case builds a throwaway repo and a stub `lucidos` CLI, so
# nothing reads or writes the real repository's config.
#
# Covered: registration is idempotent; a CLI without the subcommand is refused
# and registers nothing; the driver's result is used on exit 0; a driver that
# does not merge (exit 2), and one whose binary is gone, both leave git's own
# conflict markers rather than ours alone; the ADR index kind falls back to
# union; a repo with the attributes but no registration merges as plain text;
# `git merge-tree` (the engine's conflict probe) uses the same driver.
#
# Run: ./scripts/lib/merge_drivers_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGISTER="$SCRIPT_DIR/../merge-drivers.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# stub_cli <path> <merge-exit>: a CLI that knows `merge-driver`. On a merge it
# writes "driver result" over ours when told to exit 0, else leaves it alone.
stub_cli() {
  cat >"$1" <<EOF
#!/bin/bash
[ "\$1" = merge-driver ] || exit 64
[ "\$2" = --help ] && exit 0
[ $2 -eq 0 ] && printf 'driver result\n' > "\$4"
exit $2
EOF
  chmod +x "$1"
}

# conflicting_repo <dir>: two branches that both edit list.md's only line,
# with list.md marked merge=lucidos-blocks.
conflicting_repo() {
  git init -q -b main "$1"
  (
    cd "$1" || exit 1
    git config user.email t@example.com
    git config user.name t
    echo "list.md merge=lucidos-blocks" >.gitattributes
    echo "- base" >list.md
    git add . && git commit -qm base
    git checkout -qb side && echo "- side" >list.md && git commit -qam side
    git checkout -q main && echo "- main" >list.md && git commit -qam main
  )
}

test_registration_is_idempotent() {
  local repo="$TMP/idem" cli="$TMP/idem-cli"
  conflicting_repo "$repo"
  stub_cli "$cli" 0
  (cd "$repo" && "$REGISTER" "$cli" >/dev/null && "$REGISTER" "$cli" >/dev/null)
  local n
  n=$(cd "$repo" && git config --get-all merge.lucidos-blocks.driver | wc -l | tr -d ' ')
  if [ "$n" = 1 ]; then pass "registering twice leaves one driver entry"; else fail "registering twice left $n entries"; fi
}

test_refuses_a_cli_without_the_subcommand() {
  local repo="$TMP/old" cli="$TMP/old-cli"
  conflicting_repo "$repo"
  printf '#!/bin/bash\nexit 2\n' >"$cli" && chmod +x "$cli"
  if (cd "$repo" && "$REGISTER" "$cli" >/dev/null 2>&1); then
    fail "an old CLI was accepted"
  elif (cd "$repo" && git config --get merge.lucidos-blocks.driver >/dev/null); then
    fail "an old CLI was refused but a driver was still registered"
  else
    pass "a CLI without merge-driver is refused and registers nothing"
  fi
}

# merge_with <name> <merge-exit> [remove-cli]: register a stub, merge side.
merge_with() {
  local repo="$TMP/$1" cli="$TMP/$1-cli"
  conflicting_repo "$repo"
  stub_cli "$cli" "$2"
  (cd "$repo" && "$REGISTER" "$cli" >/dev/null)
  [ "${3:-}" = remove-cli ] && rm "$cli"
  (cd "$repo" && git merge -q --no-edit side >/dev/null 2>&1)
  echo $?
}

test_uses_the_driver_result_on_exit_0() {
  local status
  status=$(merge_with clean 0)
  if [ "$status" = 0 ] && [ "$(cat "$TMP/clean/list.md")" = "driver result" ]; then
    pass "a clean driver merge is the merge"
  else
    fail "clean driver merge: status $status, list.md: $(cat "$TMP/clean/list.md")"
  fi
}

assert_text_conflict() {
  local name=$1 status=$2 file="$TMP/$1/list.md"
  if [ "$status" != 0 ] && grep -q '^<<<<<<< ' "$file" && grep -q '^- main' "$file" && grep -q '^- side' "$file"; then
    pass "$name: git's own conflict markers, both sides kept"
  else
    fail "$name: status $status, list.md: $(cat "$file")"
  fi
}

test_a_driver_that_does_not_merge_leaves_text_markers() {
  assert_text_conflict not-merged "$(merge_with not-merged 2)"
}

test_a_missing_binary_leaves_text_markers() {
  assert_text_conflict missing "$(merge_with missing 0 remove-cli)"
}

test_the_index_kind_falls_back_to_union() {
  local repo="$TMP/union" cli="$TMP/union-cli"
  git init -q -b main "$repo"
  (
    cd "$repo" || exit 1
    git config user.email t@example.com
    git config user.name t
    echo "index.md merge=lucidos-sorted-lines" >.gitattributes
    echo "- [0001: a](1.md)" >index.md
    git add . && git commit -qm base
    git checkout -qb side && echo "- [0003: c](3.md)" >>index.md && git commit -qam side
    git checkout -q main && echo "- [0002: b](2.md)" >>index.md && git commit -qam main
  )
  stub_cli "$cli" 2
  (cd "$repo" && "$REGISTER" "$cli" >/dev/null)
  if (cd "$repo" && git merge -q --no-edit side >/dev/null 2>&1) &&
    grep -q '0002' "$repo/index.md" && grep -q '0003' "$repo/index.md"; then
    pass "a sorted-lines driver that does not merge falls back to union"
  else
    fail "sorted-lines fallback: $(cat "$repo/index.md")"
  fi
}

test_without_registration_the_attribute_merges_as_text() {
  local repo="$TMP/bare"
  conflicting_repo "$repo"
  if (cd "$repo" && git merge -q --no-edit side >/dev/null 2>&1); then
    fail "an unregistered driver name merged clean"
  else
    assert_text_conflict bare 1
  fi
}

test_merge_tree_uses_the_same_driver() {
  local repo="$TMP/probe" cli="$TMP/probe-cli" tree
  conflicting_repo "$repo"
  stub_cli "$cli" 0
  (cd "$repo" && "$REGISTER" "$cli" >/dev/null)
  tree=$(cd "$repo" && git merge-tree --write-tree --name-only --no-messages main side)
  if [ "$(cd "$repo" && git cat-file -p "$(head -1 <<<"$tree"):list.md")" = "driver result" ]; then
    pass "git merge-tree runs the registered driver, as git merge does"
  else
    fail "merge-tree did not use the driver: $tree"
  fi
}

test_registration_is_idempotent
test_refuses_a_cli_without_the_subcommand
test_uses_the_driver_result_on_exit_0
test_a_driver_that_does_not_merge_leaves_text_markers
test_a_missing_binary_leaves_text_markers
test_the_index_kind_falls_back_to_union
test_without_registration_the_attribute_merges_as_text
test_merge_tree_uses_the_same_driver

echo
echo "passed: $PASS   failed: $FAIL"
[ "$FAIL" -eq 0 ]
