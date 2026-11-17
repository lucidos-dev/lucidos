# shellcheck shell=bash
# test_skip.sh: let a scripts/lib suite skip when its subject is not in the
# tree. The public mirror strips the release scripts (RELEASE_TREE_EXCLUDE_PATHS
# in scripts/lib/release_tree.sh), and the main check tests that mirror copy.
# scripts/test-scripts.sh reports such a suite as SKIP, never as a pass.

# The exit code of a skipped suite: the automake convention.
TEST_SKIP_EXIT=77

# test_skip_unless_darwin <subject>: exit TEST_SKIP_EXIT off macOS, for a
# suite whose subject runs only on a Mac. The main check's macOS job runs it.
test_skip_unless_darwin() {
    [ "$(uname -s)" = Darwin ] && return 0
    echo "SKIP: $1 runs only on macOS"
    exit "$TEST_SKIP_EXIT"
}

# test_skip_unless_present <repo-root> <path>...: exit TEST_SKIP_EXIT, naming
# the first path the tree lacks. Returns when every path is there.
test_skip_unless_present() {
    local root="$1" path
    shift
    for path in "$@"; do
        if [ ! -e "$root/$path" ]; then
            echo "SKIP: $path is not in this tree (the public mirror strips it)"
            exit "$TEST_SKIP_EXIT"
        fi
    done
}
