#!/usr/bin/env bash
# run-tests.sh: the comment-strip tooling's own tests.
#
#   ./scripts/comment-strip/run-tests.sh
#
# Runs the stripper, pipeline, classifier and proof tests. None compiles anything. The
# Rust proof's tests run with its crate: cargo test -p lucidos-wire-codegen.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

python3 "$HERE/strip_comments_test.py"
python3 "$HERE/strip_tree_test.py"
python3 "$HERE/classify_test.py"
node --test "$HERE"/*.test.mjs
