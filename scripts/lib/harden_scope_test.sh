#!/bin/bash
# Tests for scripts/harden-scope.sh: how much a re-run of `/harden` must
# review, given what the last hardening already covered.
#
# Hermetic: every case builds a throwaway git repo under a temp dir. Nothing
# reads the engine, the real checkout, or the network.
#
# Covered: no SHA, an unknown SHA, a SHA off HEAD's history, nothing new, a
# clean merge, a conflicted merge with an extra edit to a clean file, two
# merges, a merge of a branch that is not the base, and branch commits after
# the hardening (alone, two, before a merge and after one).
#
# Run: ./scripts/lib/harden_scope_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLI="$SCRIPT_DIR/../harden-scope.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }
expect_eq() { # <name> <expected> <actual>
    if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected [$2], got [$3]"; fi
}
expect_has() { # <name> <needle> <haystack>
    case "$3" in
        *"$2"*) pass "$1" ;;
        *) fail "$1: [$2] not in [$3]" ;;
    esac
}
expect_lacks() { # <name> <needle> <haystack>
    case "$3" in
        *"$2"*) fail "$1: [$2] unexpectedly in [$3]" ;;
        *) pass "$1" ;;
    esac
}

# mktemp is checked before the cd: `cd ""` succeeds where it stands, and the
# trap would then delete that directory.
MADE="$(mktemp -d)" || exit 1
[ -n "$MADE" ] && [ -d "$MADE" ] || exit 1
TMP="$(cd "$MADE" && pwd -P)" || exit 1
trap 'rm -rf "$TMP"' EXIT
# Git never walks above TMP, so a case that lost its cd finds no repo at all.
export GIT_CEILING_DIRECTORIES="$TMP"
cd "$TMP" || exit 1

# Every write goes through g, which refuses outside TMP. A cd inside $(...)
# never reaches this shell, and that once ran a case in the real checkout.
g() {
    case "$PWD/" in
        "$TMP"/?*/) ;;
        *)
            echo "refusing git outside the test repos: $PWD" >&2
            exit 1
            ;;
    esac
    git -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false "$@" >/dev/null 2>&1
}
commit_file() { # <path> <content> <message>
    printf '%s\n' "$2" > "$1" && g add "$1" && g commit -m "$3"
}

# A repo with main (a.txt, shared.txt) and a hardened branch that edits
# branch.txt and shared.txt's first line. Leaves this shell inside it, on
# the branch, and sets H to the hardened SHA. Never call it inside $(...).
new_repo() { # <name>
    mkdir -p "$TMP/$1" && cd "$TMP/$1" || exit 1
    g init -q -b main
    printf 'line1\nline2\nline3\n' > shared.txt
    commit_file a.txt a "main: init" && g add shared.txt && g commit -m "main: shared"
    g checkout -q -b feature
    commit_file branch.txt b "feature: branch file"
    printf 'feature1\nline2\nline3\n' > shared.txt && g commit -am "feature: shared"
    H=$(git rev-parse HEAD)
}

scope() { "$CLI" "$@" 2>&1; }

echo "no hardening to build on"
new_repo none
expect_eq "empty sha" "FULL no harden marker" "$(scope "")"
expect_has "unknown sha" "FULL hardened commit not found" "$(scope 0123456789abcdef0123456789abcdef01234567)"
g checkout -q main && commit_file off.txt o "main: off"
OFF=$(git rev-parse HEAD) && g checkout -q feature
expect_has "sha off HEAD's history" "FULL hardened commit is not in HEAD's history" "$(scope "$OFF")"
expect_has "HEAD is the hardened commit" "FULL no commit since the last hardening" "$(scope "$H")"

echo "a clean merge"
new_repo clean
g checkout -q main && commit_file a.txt a2 "main: edit a" && g checkout -q feature
g merge -q --no-edit main
M=$(git rev-parse HEAD)
OUT=$(scope "$H")
expect_eq "first line" "MERGE_ONLY" "$(printf '%s\n' "$OUT" | head -1)"
expect_has "names the merge" "merge $M" "$OUT"
expect_lacks "a.txt is main's alone, no overlap" "overlap a.txt" "$OUT"

echo "a conflicted merge, plus an edit to a clean file"
new_repo conflict
g checkout -q main
printf 'main1\nline2\nline3\n' > shared.txt && g commit -am "main: shared"
commit_file a.txt a2 "main: edit a"
g checkout -q feature
g merge -q --no-edit main
printf 'feature1 main1\nline2\nline3\n' > shared.txt
printf 'evil\n' > branch.txt
g add shared.txt branch.txt && g commit --no-edit
M=$(git rev-parse HEAD)
OUT=$(scope "$H")
expect_eq "first line" "MERGE_ONLY" "$(printf '%s\n' "$OUT" | head -1)"
expect_has "names the merge" "merge $M" "$OUT"
expect_has "overlap on the conflicted file" "overlap shared.txt" "$OUT"
expect_lacks "no overlap on main's file" "overlap a.txt" "$OUT"
RESOLUTION=$(git show --remerge-diff --format= "$M")
expect_has "resolution diff shows the resolved line" "+feature1 main1" "$RESOLUTION"
expect_has "resolution diff shows the clean-file edit" "+evil" "$RESOLUTION"

echo "two merges"
new_repo two
g checkout -q main && commit_file m1.txt 1 "main: one" && g checkout -q feature
g merge -q --no-edit main
M1=$(git rev-parse HEAD)
g checkout -q main && commit_file m2.txt 2 "main: two" && g checkout -q feature
g merge -q --no-edit main
M2=$(git rev-parse HEAD)
expect_eq "both merges, oldest first" "MERGE_ONLY
merge $M1
merge $M2" "$(scope "$H")"

echo "a merge of a branch that is not the base"
new_repo other
g checkout -q -b side main && commit_file s.txt s "side: work"
g checkout -q feature && g merge -q --no-edit side
expect_has "side is not the base" "FULL a merge brought in commits the base lacks" "$(scope "$H")"

echo "an explicit base"
new_repo base
g checkout -q -b trunk main && commit_file t.txt t "trunk: work"
g checkout -q feature && g merge -q --no-edit trunk
expect_eq "trunk as base" "MERGE_ONLY" "$(scope "$H" trunk | head -1)"

echo "a branch commit after the hardening"
new_repo own
commit_file late.txt l "feature: late fix"
C=$(git rev-parse HEAD)
expect_eq "own commit alone" "INCREMENTAL
commit $C
codex-base $H" "$(scope "$H")"

echo "two branch commits"
new_repo owntwo
commit_file late1.txt 1 "feature: fix one"
C1=$(git rev-parse HEAD)
commit_file late2.txt 2 "feature: fix two"
C2=$(git rev-parse HEAD)
expect_eq "both commits, oldest first" "INCREMENTAL
commit $C1
commit $C2
codex-base $H" "$(scope "$H")"

echo "a branch commit, then a merge"
new_repo ownmerge
commit_file late.txt l "feature: late fix"
C=$(git rev-parse HEAD)
g checkout -q main && commit_file m.txt m "main: moved" && g checkout -q feature
g merge -q --no-edit main
M=$(git rev-parse HEAD)
OUT=$(scope "$H")
expect_eq "first line" "INCREMENTAL" "$(printf '%s\n' "$OUT" | head -1)"
expect_has "names the commit" "commit $C" "$OUT"
expect_has "names the merge" "merge $M" "$OUT"
expect_lacks "main's commit is not the branch's" "main: moved" "$OUT"
expect_lacks "no Codex base once main's code is in range" "codex-base" "$OUT"

echo "a merge, then a fix (the merge session that finds a failing suite)"
new_repo mergeown
g checkout -q main
printf 'main1\nline2\nline3\n' > shared.txt && g commit -am "main: shared"
g checkout -q feature
g merge -q --no-edit main
printf 'feature1 main1\nline2\nline3\n' > shared.txt
g add shared.txt && g commit --no-edit
M=$(git rev-parse HEAD)
commit_file fix.txt f "feature: fix a failing suite"
C=$(git rev-parse HEAD)
OUT=$(scope "$H")
expect_eq "commit, merge, overlap, in that order" "INCREMENTAL
commit $C
merge $M
overlap shared.txt" "$OUT"

echo
echo "harden_scope: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
