#!/usr/bin/env bash
# onnxruntime_mirror.sh: which ONNX Runtime archives the Lucidos mirror must hold.
#
# The vendored ort-sys build script (crates/vendor/ort-sys/build.rs, ADR 0595)
# fetches each archive from a mirror before parcel.pyke.io. This lib reads the
# same two inputs it does, so no value is defined twice:
#   - MIRROR_BASE_URL, parsed out of build.rs;
#   - the rows of dist.txt, byte-identical to upstream.
# The shipped triples come from release_draft_triples (release_draft.sh), and the
# caller sources that lib first.
#
# The mirror URL is MIRROR_BASE_URL plus the upstream path after `msort-binary/`,
# exactly as build.rs derives it. The first path segment is the release tag.

ONNXRUNTIME_MIRROR_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ONNXRUNTIME_MIRROR_SYS_DIR="${ONNXRUNTIME_MIRROR_SYS_DIR:-$ONNXRUNTIME_MIRROR_ROOT/crates/vendor/ort-sys}"

# onnxruntime_mirror_base_url: the base URL build.rs fetches the mirror from.
onnxruntime_mirror_base_url() {
    sed -n 's/^const MIRROR_BASE_URL: &str = "\(.*\)";$/\1/p' "$ONNXRUNTIME_MIRROR_SYS_DIR/build.rs"
}

# onnxruntime_mirror_repo: the mirror's <owner>/<name> slug, from the base URL.
onnxruntime_mirror_repo() {
    onnxruntime_mirror_base_url | sed -n 's#^https://github.com/\([^/]*/[^/]*\)/releases/download$#\1#p'
}

# onnxruntime_mirror_rows: one tab-separated row per archive a release build
# needs: triple, upstream URL, lowercase SHA-256, mirror URL, release tag,
# asset name. Fails if the base URL, the repo or a shipped triple's row is
# missing, so a renamed constant cannot make a check pass on zero rows.
onnxruntime_mirror_rows() {
    local base triple row upstream hash path
    base="$(onnxruntime_mirror_base_url)"
    [[ -n "$base" && -n "$(onnxruntime_mirror_repo)" ]] \
        || { echo "ERROR: no usable MIRROR_BASE_URL in $ONNXRUNTIME_MIRROR_SYS_DIR/build.rs" >&2; return 1; }
    for triple in $(release_draft_triples); do
        row="$(awk -F'\t' -v t="$triple" '$1 == "none" && $2 == t { print $3 "\t" $4 }' "$ONNXRUNTIME_MIRROR_SYS_DIR/dist.txt")"
        [[ -n "$row" ]] || { echo "ERROR: dist.txt has no static archive for $triple" >&2; return 1; }
        upstream="${row%%$'\t'*}"
        hash="$(printf '%s' "${row##*$'\t'}" | tr 'A-F' 'a-f')"
        path="${upstream#*msort-binary/}"
        printf '%s\t%s\t%s\t%s/%s\t%s\t%s\n' \
            "$triple" "$upstream" "$hash" "$base" "$path" "${path%%/*}" "${path##*/}"
    done
}
