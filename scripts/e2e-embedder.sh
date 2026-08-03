#!/bin/bash
# Run the real-embedder integration tests in lucidos-engine.
#
# Usage:
#   ./scripts/e2e-embedder.sh [-- cargo test args]
#
# Compiles lucidos-engine with `--features real-embedder-tests` and runs only
# the tests gated behind that feature. They exercise properties of the real
# fastembed model (MultilingualE5Small): single + batch embedding, semantic
# similarity, Norwegian synonyms, cross-language thread search. They download
# ~465 MB from huggingface.co on first run and are cached after.
#
# These tests do NOT need a running Lucidos workspace — they construct an
# embedder in-process.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Substring filters — cargo test runs any test whose name contains one of
# these. Each entry must be unique enough not to incidentally match a
# non-gated test. The sanity check below detects drift if a new gated test
# is added without updating this list.
GATED_TESTS=(
    test_embed_single_text
    test_embed_batch
    test_similar_texts_have_similar_embeddings
    test_norwegian_synonyms_have_high_similarity
    test_bil_verksted_query_matches_norwegian_vehicle_service_thread
    test_downloaded_model_loads_without_a_second_fetch
)

# Drift check: any test annotated with both `#[cfg(feature =
# "real-embedder-tests")]` and `#[(tokio::)?test]` must appear in GATED_TESTS,
# and vice versa. Catches forgotten list updates when a new gated test is added.
EXPECTED=$(rg --multiline -P --no-filename \
        '#\[cfg\(feature = "real-embedder-tests"\)\]\s*\n\s*#\[(tokio::)?test\]\s*\n\s*(async )?fn (\w+)' \
        -or '$3' \
        crates/lucidos-engine/src/ \
    | sort -u)
ACTUAL=$(printf '%s\n' "${GATED_TESTS[@]}" | sort -u)
if ! diff <(echo "$EXPECTED") <(echo "$ACTUAL") >/dev/null; then
    echo "ERROR: GATED_TESTS in $0 is out of sync with #[cfg(feature = \"real-embedder-tests\")] tests in crates/lucidos-engine/src/" >&2
    echo "Expected (from source):" >&2
    # shellcheck disable=SC2001 # per-line indent of a multi-line block; ${//} cannot anchor to ^
    echo "$EXPECTED" | sed 's/^/  /' >&2
    echo "Actual (in script):" >&2
    # shellcheck disable=SC2001 # per-line indent of a multi-line block; ${//} cannot anchor to ^
    echo "$ACTUAL" | sed 's/^/  /' >&2
    exit 1
fi

CARGO_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --) shift; CARGO_ARGS+=("$@"); break ;;
        *)  CARGO_ARGS+=("$1"); shift ;;
    esac
done

# Pin the fastembed model cache to a stable, machine-persistent location so the
# ~465 MB seed survives `cargo clean`, worktree churn, and is shared across every
# worktree + the nightly main checkout — instead of fastembed's default
# `.fastembed_cache` relative to cwd, which each worktree re-downloads. A warm
# cache makes hf-hub's `ApiRepo::get` short-circuit before any network call, so
# seeded runs are fully offline and deterministic. fastembed reads this var via
# `get_cache_dir()`. (If the cache is cold AND huggingface.co is unreachable, the
# tests skip rather than fail — see `shared_embedder()` in lucidos-engine.)
export FASTEMBED_CACHE_DIR="${FASTEMBED_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/fastembed}"
mkdir -p "$FASTEMBED_CACHE_DIR"
echo "Using fastembed model cache: $FASTEMBED_CACHE_DIR"

echo "Running real-embedder tests (downloads ~465 MB on first run)..."
# Test names go after `--` so libtest receives them as substring filters.
# `cargo test` itself only accepts one positional [TESTNAME] before --.
# `--nocapture` surfaces the SKIP line `shared_embedder()` prints when the model
# can't be fetched, so an HF outage reads as an explicit skip rather than a
# silent pass.
cargo test --locked -p lucidos-engine --features real-embedder-tests --lib \
    -- "${GATED_TESTS[@]}" --nocapture "${CARGO_ARGS[@]}"
