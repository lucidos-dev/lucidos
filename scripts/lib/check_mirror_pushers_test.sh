#!/usr/bin/env bash
# Tests for scripts/check-mirror-pushers.sh, the /harden gate that keeps
# release_tree_push the only writer to the public mirror (ADR 0462).
#
# Each case builds a throwaway git repo holding the allowed forms, adds one
# file, and runs the check against it. The real tree must pass too.
#
# Hermetic: the check only reads tracked files, and every fixture lives under
# mktemp. Nothing here pushes anywhere.
#
# Run: ./scripts/lib/check_mirror_pushers_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$SCRIPT_DIR/../check-mirror-pushers.sh"

PASS=0
FAIL=0
pass() { echo "  ok:   $*"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $*"; FAIL=$((FAIL + 1)); }

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

# put <repo> <path> <content>: write one file.
put() {
    mkdir -p "$(dirname "$1/$2")"
    printf '%s\n' "$3" >"$1/$2"
}

# The allowed forms, each in the place the allowlist names.
MIRROR_GIT_URL="git@github.com:lucidos-dev/lucidos.git"
# shellcheck disable=SC2016 # the fixtures are shell source, never expanded here
new_fixture() { # -> prints the repo path
    local repo
    repo="$(mktemp -d "$SANDBOX/repo.XXXXXX")"
    git init -q "$repo"
    put "$repo" scripts/lib/release_tree.sh 'git -C "$repo" push "$url" "$sha:$ref"'
    put "$repo" scripts/lib/release_main_sync.sh 'git -C "$repo" push "$remote" "refs/heads/main:refs/heads/main"'
    put "$repo" scripts/release.sh 'git -C "$REPO_ROOT" push $ORIGIN_REMOTE main'
    put "$repo" scripts/lib/thing_test.sh 'git -C "$root" push -q "$REMOTE" HEAD:refs/heads/main'
    put "$repo" scripts/notes.sh '# a comment may say: git push lucidos main'
    put "$repo" crates/engine/src/merge.rs '        match git_cmd(&["push", "origin", "main"], &repo_root).await {'
    put "$repo" install.sh "LUCIDOS_REPO_URL=\"\${LUCIDOS_REPO_URL:-$MIRROR_GIT_URL}\""
    put "$repo" docs/howto.md "Run git push lucidos main, or clone $MIRROR_GIT_URL."
    git -C "$repo" add -A
    git -C "$repo" -c user.email=dev@example.com -c user.name=Dev commit -qm fixture
    printf '%s' "$repo"
}

# expect <label> <want: pass|fail> <repo> [path-the-failure-must-name]
expect() {
    local label="$1" want="$2" repo="$3" path="${4:-}" out rc
    out="$("$CHECK" "$repo" 2>&1)" && rc=0 || rc=$?
    if [ "$want" = pass ] && [ "$rc" -eq 0 ]; then
        pass "$label"
    elif [ "$want" = fail ] && [ "$rc" -ne 0 ] && [[ "$out" == *"$path"* ]]; then
        pass "$label"
    else
        fail "$label (rc=$rc): $out"
    fi
}

# expect_stray <label> <path> <content>: a fixture plus this one tracked file fails.
expect_stray() {
    local repo
    repo="$(new_fixture)"
    put "$repo" "$2" "$3"
    git -C "$repo" add -A
    git -C "$repo" -c user.email=dev@example.com -c user.name=Dev commit -qm stray
    expect "$1" fail "$repo" "$2"
}

echo "test: the allowed forms pass"
expect "release_tree.sh, origin pushes, tests, comments, docs and the install clone URL pass" \
    pass "$(new_fixture)"

echo "test: a push past the one door fails"
# shellcheck disable=SC2016 # the strays are shell source, never expanded here
{
    expect_stray "a raw push in a script fails" scripts/publish.sh 'git -C "$repo" push lucidos "$c:refs/heads/main"'
    expect_stray "a bare push in a script fails" scripts/quick.sh 'out="$(git push)"'
    expect_stray "a mirror push in release.sh fails" scripts/release.sh 'git -C "$REPO_ROOT" push --force "$REMOTE" "$c:refs/tags/v1"'
    expect_stray "a push in a workflow fails" .github/workflows/sync.yml '        run: git push mirror HEAD:main'
    expect_stray "a push in a Makefile fails" Makefile '	git push lucidos main'
    expect_stray "a Rust push to a variable remote fails" crates/engine/src/sync.rs '    git_cmd(&["push", remote, "main"], &repo).await?;'
    expect_stray "a Rust .arg(\"push\") fails" crates/engine/src/other.rs '    cmd.arg("push");'
}

echo "test: the mirror's URL outside release_tree.sh fails"
expect_stray "the mirror URL in a script fails" scripts/remote.sh "git remote add lucidos $MIRROR_GIT_URL"
expect_stray "the mirror URL in a Rust source fails" crates/engine/src/url.rs "const M: &str = \"$MIRROR_GIT_URL\";"
expect_stray "the mirror URL in a comment fails" scripts/doc.sh "# clone $MIRROR_GIT_URL"
expect_stray "install.sh may name it only on its clone line" install.sh "PUSH_TO=$MIRROR_GIT_URL"

echo "test: an untracked file is not part of the tree"
REPO="$(new_fixture)"
put "$REPO" scripts/scratch.sh 'git push lucidos main'
expect "an untracked stray does not fail the check" pass "$REPO"

echo "test: the real tree passes"
expect "this checkout has one door to the mirror" pass "$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

echo "test: outside a git checkout the check fails rather than reading clean"
mkdir -p "$SANDBOX/not-a-repo"
expect "a directory that is not a checkout fails" fail "$SANDBOX/not-a-repo" "not inside a git checkout"

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
