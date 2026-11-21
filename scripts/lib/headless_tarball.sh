#!/usr/bin/env bash
# headless_tarball.sh — package the self-contained Lucidos runtime tree (engine +
# gateway + frontend + postgres + sdk) as a plain per-platform tarball plus a
# sha256 sidecar: the headless twin of the .app/.dmg.
#
# This is step 1 of docs/plans/2026-06-30-installer-step1-headless-tarball.md — the headless
# artifact a later install.sh rewrite downloads instead of compiling from source.
#
# Pure shell + tar/gzip + python3 (sha256 via release_staging_sha256). No git, gh,
# or network, and NO dependence on build-dmg.sh globals, so the caller passes
# everything explicitly and scripts/lib/headless_tarball_test.sh exercises it
# offline with fake resource files. The CALLER chooses the source dir; build-dmg.sh
# passes the SIGNED .app Contents/Resources so the Mach-O code signatures survive
# into the tarball (bundle-resources/ itself is never signed). scripts/build-headless.sh
# (the Linux/Tauri-free path, step 2) passes a plain staged runtime tree.
#
# Cross-platform: macOS copies with `ditto` (preserves the embedded Mach-O code
# signatures of the .app-sourced tree); Linux falls back to `cp -a` (perms +
# symlinks, no code signatures to preserve), so the same lib runs on a CI Linux
# runner. COPYFILE_DISABLE=1 keeps BSD tar from emitting AppleDouble members and is
# a harmless no-op for GNU tar.
#
# Depends on release_staging_sha256 from scripts/lib/release_staging.sh — the
# caller must source that lib first (build-dmg.sh, build-headless.sh, and the test
# all do).

# headless_tarball_stem <version> <triple> — print the artifact stem (no extension),
# e.g. lucidos-0.14.0-aarch64-apple-darwin.
headless_tarball_stem() {
    printf 'lucidos-%s-%s' "$1" "$2"
}

# headless_tarball_copy <src> <dst> — copy preserving perms + symlinks. Uses
# `ditto` on macOS (also preserves embedded Mach-O code signatures); `cp -a`
# elsewhere (Linux runners have no `ditto`, and there are no signatures to keep).
headless_tarball_copy() {
    if command -v ditto >/dev/null 2>&1; then
        ditto "$1" "$2"
    else
        cp -a "$1" "$2"
    fi
}

# headless_tarball_emit <resources-dir> <out-dir> <version> <triple> <name…>
# Copy each <name> out of <resources-dir> into a clean <stem>/ prefix, tar+gzip it
# to <out-dir>/<stem>.tar.gz, and write <out-dir>/<stem>.tar.gz.sha256 (a
# `shasum -a 256 -c`-compatible "<hex>  <file>" line). Prints the tarball path on
# success; returns non-zero with a stderr message on any problem.
headless_tarball_emit() {
    local resources="$1" out_dir="$2" version="$3" triple="$4"
    shift 4
    [ "$#" -ge 1 ]      || { echo "ERROR: headless_tarball_emit needs at least one resource name" >&2; return 1; }
    [ -d "$resources" ] || { echo "ERROR: resources dir '$resources' does not exist" >&2; return 1; }

    local stem tar_path sha_path name tmp sum
    stem="$(headless_tarball_stem "$version" "$triple")"
    tar_path="$out_dir/$stem.tar.gz"
    sha_path="$tar_path.sha256"

    for name in "$@"; do
        [ -e "$resources/$name" ] || { echo "ERROR: resource '$name' missing under $resources" >&2; return 1; }
    done

    mkdir -p "$out_dir" || return 1
    tmp="$(mktemp -d)"  || return 1
    mkdir -p "$tmp/$stem" || { rm -rf "$tmp"; return 1; }
    for name in "$@"; do
        # headless_tarball_copy preserves perms, symlinks, and (on macOS) the
        # embedded Mach-O code signatures.
        headless_tarball_copy "$resources/$name" "$tmp/$stem/$name" || { rm -rf "$tmp"; return 1; }
    done

    rm -f "$tar_path"
    # COPYFILE_DISABLE=1 keeps BSD tar from injecting ._ AppleDouble members; the
    # code signature lives inside the Mach-O file, not in an xattr, so dropping
    # AppleDouble is safe and yields a clean cross-platform tarball.
    if ! COPYFILE_DISABLE=1 tar -C "$tmp" -czf "$tar_path" "$stem"; then
        rm -rf "$tmp"; return 1
    fi
    rm -rf "$tmp"

    sum="$(release_staging_sha256 "$tar_path")" || return 1
    printf '%s  %s\n' "$sum" "$stem.tar.gz" > "$sha_path" || return 1

    printf '%s\n' "$tar_path"
}
