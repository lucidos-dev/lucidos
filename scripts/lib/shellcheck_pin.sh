#!/usr/bin/env bash

SHELLCHECK_MIN_VERSION=0.11.0

shellcheck_supported() {
    local version
    version="$("$1" --version | awk '/^version:/ {print $2}')" || return 1
    awk -v version="$version" -v floor="$SHELLCHECK_MIN_VERSION" 'BEGIN {
        if (version !~ /^[0-9]+\.[0-9]+\.[0-9]+$/) exit 1
        split(version, actual, "."); split(floor, minimum, ".")
        for (i = 1; i <= 3; i++) {
            if (actual[i]+0 > minimum[i]+0) exit 0
            if (actual[i]+0 < minimum[i]+0) exit 1
        }
    }'
}

shellcheck_provision() (
    local cache=$1 platform digest archive work actual
    platform="$(uname -s).$(uname -m)"
    case "$platform" in
        Linux.x86_64) platform=linux.x86_64; digest=b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6 ;;
        Linux.aarch64) platform=linux.aarch64; digest=68a8133197a50beb8803f8d42f9908d1af1c5540d4bb05fdfca8c1fa47decefc ;;
        Darwin.x86_64) platform=darwin.x86_64; digest=c2c15e08df0e8fbc374c335b230a7ee958c313fa5714817a59aa59f1aa594f51 ;;
        Darwin.arm64) platform=darwin.aarch64; digest=339b930feb1ea764467013cc1f72d09cd6b869ebf1013296ba9055ab2ffbd26f ;;
        *) return 1 ;;
    esac
    mkdir -p "$cache" || return 1
    work="$(mktemp -d "$cache/.download.XXXXXX")" || return 1
    trap 'rm -rf "$work"' EXIT
    archive="shellcheck-v$SHELLCHECK_MIN_VERSION.$platform.tar.gz"
    curl -fsSL "https://github.com/koalaman/shellcheck/releases/download/v$SHELLCHECK_MIN_VERSION/$archive" -o "$work/$archive" || return 1
    if command -v sha256sum >/dev/null; then
        actual="$(sha256sum "$work/$archive")" || return 1
    else
        actual="$(shasum -a 256 "$work/$archive")" || return 1
    fi
    if [ "${actual%% *}" != "$digest" ]; then
        echo "ERROR: ShellCheck archive checksum mismatch ($archive)." >&2
        return 1
    fi
    tar -xzf "$work/$archive" -C "$work" "shellcheck-v$SHELLCHECK_MIN_VERSION/shellcheck" || return 1
    local binary="$work/shellcheck-v$SHELLCHECK_MIN_VERSION/shellcheck"
    shellcheck_supported "$binary" || return 1
    mv -f "$binary" "$cache/shellcheck" || return 1
)

shellcheck_resolve() {
    local binary cache="${XDG_CACHE_HOME:-$HOME/.cache}/lucidos/tools/shellcheck-$SHELLCHECK_MIN_VERSION"
    if binary="$(type -P shellcheck)" && shellcheck_supported "$binary" 2>/dev/null; then
        printf '%s\n' "$binary"
    elif [ -x "$cache/shellcheck" ] && shellcheck_supported "$cache/shellcheck" 2>/dev/null; then
        printf '%s\n' "$cache/shellcheck"
    elif [ "${LUCIDOS_NO_TOOL_DOWNLOAD:-}" != 1 ] && shellcheck_provision "$cache"; then
        printf '%s\n' "$cache/shellcheck"
    else
        cat >&2 <<FIX
ERROR: ShellCheck $SHELLCHECK_MIN_VERSION or later is required.
Install it with brew install shellcheck, sudo apt install shellcheck,
sudo dnf install ShellCheck, or sudo pacman -S shellcheck.
If your package manager ships an older version, use the upstream release:
https://github.com/koalaman/shellcheck/releases/tag/v$SHELLCHECK_MIN_VERSION
FIX
        return 1
    fi
}
