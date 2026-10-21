# shellcheck shell=bash
# The functions behind scripts/codegen.sh, sourced so codegen_test.sh can drive
# them on a scratch repository. Generated files are build outputs
# (docs/plans/2026-10-12-generated-files-become-build-outputs.md).
#
# The Rust step is keyed by a *codegen fingerprint*: a hash of the blob ids of
# every codegen input. A hit copies the outputs from the *codegen cache*, which
# lives in the git common dir, so the main checkout and every worktree share it.
# A miss renders through a freshly built `lucidos-codegen`, or else through the
# engine's `write_generated_outputs` test, which reuses the `make test` compile.

# Everything whose change can change a generated file: the engine and every
# crate it compiles from this repo, plus the lockfile and the toolchain. A
# rendering run checks this list against cargo's dep-info (codegen_check_inputs).
CODEGEN_INPUT_ROOTS=(
    .cargo/config.toml
    Cargo.lock
    Cargo.toml
    rust-toolchain.toml
    scripts/codegen.sh
    scripts/lib/codegen.sh
    crates/lucidos-build-slot
    crates/lucidos-capabilities
    crates/lucidos-claude-code-managed
    crates/lucidos-engine
    crates/lucidos-engine-socket
    crates/lucidos-file-backup-exclusion
    crates/lucidos-frame-capability
    crates/lucidos-gateway-constants
    crates/lucidos-home
    crates/lucidos-local-token
    crates/lucidos-owner-lock
    crates/lucidos-pg-url
    crates/lucidos-tailscale
    crates/vendor
)

# Files the engine embeds that no renderer reads, so a change to one is no
# reason to render again. Keeping them out is what lets a frontend-only change
# stay a cache hit. A path here must never feed a generated file.
CODEGEN_COMPILE_ONLY=(
    .claude/skills/lucidos-cli/SKILL.md
    CHANGELOG.md
    RELEASE
    release-notices.toml
    crates/lucidos-app/public/
    crates/lucidos-app/src/assets/fonts/
    crates/lucidos-app/src/styles/global/
)

# How many renderings the cache keeps, newest first.
CODEGEN_CACHE_KEEP=32

# One file every rendering writes, so its presence means a checkout has its
# generated files. `codegen_sentinel_is_rendered` in the engine's codegen
# module pins it to `render_all`.
CODEGEN_SENTINEL=crates/lucidos-app/src/generated/thread-event-wire.ts

codegen_log() { echo "codegen: $*" >&2; }

# The fingerprint of <root>'s working tree, or of <rev> when given. Neither
# needs a checkout: a commit is read from its tree, and the working tree from
# the index plus the content of every changed or untracked file.
codegen_fingerprint() { # <root> [rev]
    local root=$1 rev=${2:-}
    {
        if [ -n "$rev" ]; then
            git -C "$root" ls-tree -r "$rev" -- "${CODEGEN_INPUT_ROOTS[@]}" |
                awk -F'\t' '{ split($1, f, " "); print f[3] " " $2 }'
        else
            codegen_working_tree_blobs "$root"
        fi
    } | LC_ALL=C sort -k2 | git hash-object --stdin
}

# "<blob> <path>" for each input in the working tree, unsorted.
codegen_working_tree_blobs() { # <root>
    local root=$1 changed path
    changed=$(git -C "$root" ls-files -m -o -d --exclude-standard -- "${CODEGEN_INPUT_ROOTS[@]}" | LC_ALL=C sort -u)
    git -C "$root" ls-files -s -- "${CODEGEN_INPUT_ROOTS[@]}" |
        awk -F'\t' '{ split($1, f, " "); print f[2] " " $2 }' |
        codegen_drop_paths "$changed"
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        [ -f "$root/$path" ] || continue
        echo "$(git -C "$root" hash-object -- "$path") $path"
    done <<<"$changed"
}

# Drop the lines whose path is in the newline-separated <paths>.
codegen_drop_paths() { # <paths>
    CODEGEN_DROP=$1 awk 'BEGIN { n = split(ENVIRON["CODEGEN_DROP"], d, "\n"); for (i = 1; i <= n; i++) if (d[i] != "") gone[d[i]] = 1 }
        { path = substr($0, index($0, " ") + 1); if (!(path in gone)) print }'
}

codegen_cache_root() { # <root>
    local common
    common=$(git -C "$1" rev-parse --git-common-dir)
    case $common in /*) ;; *) common="$1/$common" ;; esac
    echo "$common/lucidos-codegen"
}

# Copy each file of a rendering into <root>, writing only bytes that changed,
# so an unchanged file keeps its mtime and no watcher rebuilds for nothing.
codegen_sync() { # <rendering> <root>
    local from=$1 root=$2 rel tmp
    while IFS= read -r rel; do
        rel=${rel#./}
        if ! cmp -s "$from/$rel" "$root/$rel"; then
            mkdir -p "$(dirname "$root/$rel")"
            tmp=$(mktemp "$root/$rel.XXXXXX")
            cp "$from/$rel" "$tmp"
            mv "$tmp" "$root/$rel"
        fi
    done < <(cd "$from" && find . -type f | LC_ALL=C sort)
}

# The rendering for <fingerprint>, if the cache holds one.
codegen_cached() { # <root> <fingerprint>
    local dir
    dir="$(codegen_cache_root "$1")/$2"
    [ -d "$dir" ] && echo "$dir"
}

# Render into the cache under <fingerprint>. With <bin>, run that freshly built
# `lucidos-codegen`; without, run the engine's ignored writer test. <reused>
# marks a <bin> from the e2e engine cache, whose engine input manifest already
# matched this tree. Its age says nothing then, and its dep-info stayed behind
# with the build that made it, so its inputs are not checked again here.
codegen_render() { # <root> <fingerprint> [bin] [reused]
    local root=$1 fp=$2 bin=${3:-} reused=${4:-} cache tmp
    cache=$(codegen_cache_root "$root")
    mkdir -p "$cache"
    tmp=$(mktemp -d "$cache/.rendering.XXXXXX")
    if [ -n "$bin" ]; then
        codegen_log "rendering with $bin"
        "$bin" --out "$tmp" --repo-root "$root" || { rm -rf "$tmp"; return 1; }
        if [ -z "$reused" ]; then
            codegen_check_inputs "$root" "$bin.d" "$bin" || { rm -rf "$tmp"; return 1; }
        fi
    else
        codegen_log "rendering through the engine's write_generated_outputs test"
        local log test_bin
        log=$(mktemp)
        (cd "$root" && LUCIDOS_CODEGEN_OUT="$tmp" LUCIDOS_CODEGEN_REPO_ROOT="$root" \
            "$root/scripts/with-build-slot.sh" --label "codegen" -- \
            cargo test --locked -p lucidos-engine --lib -- --ignored --exact \
            codegen::tests::write_generated_outputs) > "$log" 2>&1
        local rc=$?
        cat "$log" >&2
        test_bin=$(codegen_test_binary "$root" < "$log")
        rm -f "$log"
        [ "$rc" -eq 0 ] || { rm -rf "$tmp"; return 1; }
        # The test binary also compiles test-only fixtures no renderer reads, so
        # only its freshness is checked here. The bin path checks coverage.
        codegen_check_inputs "$root" "$test_bin.d" "$test_bin" --freshness-only ||
            { rm -rf "$tmp"; return 1; }
    fi
    # Another checkout may have rendered the same fingerprint meanwhile. Its
    # copy is byte-identical, so the one already there stays. `mv` onto an
    # existing directory moves into it, so a lost race is removed from inside.
    if [ -d "$cache/$fp" ]; then
        rm -rf "$tmp"
    else
        mv "$tmp" "$cache/$fp"
        rm -rf "${cache:?}/$fp/$(basename "$tmp")"
    fi
    codegen_prune "$cache"
}

# The test binary cargo ran, read from its "Running unittests" line. Cargo ran
# in <root> and prints the path relative to it, while the caller may sit in
# another directory, such as npm's crates/lucidos-app.
codegen_test_binary() { # <root>
    local bin
    bin=$(sed -n 's/^ *Running unittests src\/lib\.rs (\(.*\))$/\1/p' | tail -1)
    case $bin in
        /* | '') echo "$bin" ;;
        *) echo "$1/$bin" ;;
    esac
}

codegen_prune() { # <cache>
    # shellcheck disable=SC2012 # fingerprints are hex, with no newlines
    ls -1t "$1" | tail -n "+$((CODEGEN_CACHE_KEEP + 1))" | while IFS= read -r old; do
        rm -rf "${1:?}/$old"
    done
}

# Fail when the renderer compiled a file of this repo that the fingerprint does
# not cover, or one newer than the renderer. Either would cache output that does
# not match its fingerprint, which is the failure this scheme exists to remove.
codegen_check_inputs() { # <root> <dep-info> <artifact> [--freshness-only]
    local root=$1 depinfo=$2 artifact=$3 coverage=${4:-} found
    [ -f "$depinfo" ] || { codegen_log "no dep-info at $depinfo, so the inputs cannot be checked"; return 1; }
    [ "$coverage" = --freshness-only ] || found=$(codegen_undeclared_inputs "$root" "$depinfo")
    if [ -n "${found:-}" ]; then
        codegen_log "the renderer compiled files the codegen fingerprint does not cover:"
        printf '%s\n' "$found" | sed 's/^/  /' >&2
        codegen_log "add each to CODEGEN_INPUT_ROOTS, or to CODEGEN_COMPILE_ONLY if no renderer reads it (scripts/lib/codegen.sh)"
        return 1
    fi
    found=$(codegen_repo_inputs "$root" "$depinfo" | while IFS= read -r rel; do
        if [ "$root/$rel" -nt "$artifact" ]; then echo "$rel"; fi
    done)
    if [ -n "$found" ]; then
        codegen_log "$artifact is older than files it compiled, so it would render stale output:"
        printf '%s\n' "$found" | sed 's/^/  /' >&2
        return 1
    fi
}

# The repo files <dep-info> lists, repo-relative with `.` and `..` resolved, one
# per line. rustc writes a workspace file relative to the root. The target dir
# and the git dir (the engine's build script watches HEAD) are skipped.
codegen_repo_inputs() { # <root> <dep-info>
    local root=$1 target=${CARGO_TARGET_DIR:-$1/target}
    sed 's/\\ /@@SP@@/g; s/\\$//' "$2" | tr ' ' '\n' | sed '/:$/d; /^$/d' |
        awk -v root="$root" -v target="$target" '
            function normal(p,    n, parts, out, i, k) {
                n = split(p, parts, "/"); k = 0
                for (i = 1; i <= n; i++) {
                    if (parts[i] == "" || parts[i] == ".") continue
                    if (parts[i] == "..") { if (k > 0) k--; continue }
                    out[++k] = parts[i]
                }
                p = ""; for (i = 1; i <= k; i++) p = p "/" out[i]
                return p
            }
            {
                gsub("@@SP@@", " ")
                p = $0
                if (substr(p, 1, 1) != "/") p = root "/" p
                p = normal(p)
                if (index(p, normal(target) "/") == 1) next
                if (index(p, normal(root) "/") != 1) next
                rel = substr(p, length(normal(root)) + 2)
                if (rel == ".git" || index(rel, ".git/") == 1) next
                print rel
            }' | LC_ALL=C sort -u
}

# The repo files in <dep-info> that no input root or compile-only entry
# covers, one per line.
codegen_undeclared_inputs() { # <root> <dep-info>
    codegen_repo_inputs "$1" "$2" |
        CODEGEN_COVERED=$(printf '%s\n' "${CODEGEN_INPUT_ROOTS[@]}" "${CODEGEN_COMPILE_ONLY[@]}") awk '
            BEGIN { n = split(ENVIRON["CODEGEN_COVERED"], c, "\n") }
            {
                for (i = 1; i <= n; i++) {
                    if (c[i] == "") continue
                    if ($0 == c[i] || index($0, c[i] (c[i] ~ /\/$/ ? "" : "/")) == 1) next
                }
                print
            }'
}

# Build the SDK bundles into packages/lucidos-sdk/dist/. They import the
# Rust-generated TS, so this always runs after the Rust step.
codegen_build_sdk() { # <root>
    (cd "$1/packages/lucidos-sdk" && node build.mjs) >&2
}

codegen_usage() {
    cat >&2 <<'EOF'
Usage: scripts/codegen.sh [--bin <lucidos-codegen> | --reused-bin <lucidos-codegen>] [--cached-only]
       scripts/codegen.sh --fingerprint [<rev>]
       scripts/codegen.sh --cache-dir [<rev>]

Writes every generated file: the Rust-rendered TS, CSS and JSON, then the SDK
bundles in packages/lucidos-sdk/dist/.

  --bin <path>     render a miss with this freshly built lucidos-codegen
  --reused-bin <path>
                   render a miss with a lucidos-codegen the e2e engine cache
                   matched to this tree, without checking its inputs again
  --cached-only    never compile: on a miss keep the outputs already there
  --fingerprint    print the codegen fingerprint of the working tree or <rev>
  --cache-dir      print the cached rendering for the working tree or <rev>
EOF
}

codegen_main() { # <root> [args...]
    local root=$1 bin="" reused="" cached_only="" fp dir
    shift
    case ${1:-} in
        --fingerprint) codegen_fingerprint "$root" "${2:-}"; return ;;
        --cache-dir) codegen_cached "$root" "$(codegen_fingerprint "$root" "${2:-}")"; return ;;
    esac
    while [ $# -gt 0 ]; do
        case $1 in
            --bin) bin=${2:?--bin needs a path}; shift 2 ;;
            --reused-bin) bin=${2:?--reused-bin needs a path}; reused=1; shift 2 ;;
            --cached-only) cached_only=1; shift ;;
            -h | --help) codegen_usage; return 0 ;;
            *) codegen_usage; return 2 ;;
        esac
    done
    fp=$(codegen_fingerprint "$root")
    if dir=$(codegen_cached "$root" "$fp"); then
        codegen_log "cache hit"
    elif [ -n "$cached_only" ]; then
        if [ -f "$root/$CODEGEN_SENTINEL" ]; then
            codegen_log "cache miss: keeping the generated files already here. Run ./scripts/codegen.sh to render them"
        else
            codegen_log "cache miss and no generated files. Run: ./scripts/codegen.sh"
            return 1
        fi
    else
        codegen_log "cache miss"
        codegen_render "$root" "$fp" "$bin" "$reused" || return 1
        dir=$(codegen_cached "$root" "$fp")
    fi
    [ -z "${dir:-}" ] || codegen_sync "$dir" "$root"
    codegen_build_sdk "$root"
}
