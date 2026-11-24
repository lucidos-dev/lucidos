#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=scripts/lib/shellcheck_pin.sh
source scripts/lib/shellcheck_pin.sh
grep -Fq "ShellCheck $SHELLCHECK_MIN_VERSION or later" README.md

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export XDG_CACHE_HOME="$tmp/cache"
mkdir "$tmp/bin"
export PATH="$tmp/bin:$PATH"
cat > "$tmp/bin/shellcheck" <<'STUB'
#!/usr/bin/env bash
printf 'version: %s\n' "$TEST_SC_VERSION"
STUB
chmod +x "$tmp/bin/shellcheck"
export TEST_SC_VERSION="$SHELLCHECK_MIN_VERSION"
[[ "$(shellcheck_resolve)" == "$tmp/bin/shellcheck" ]]

export TEST_SC_VERSION=0.10.0
if LUCIDOS_NO_TOOL_DOWNLOAD=1 shellcheck_resolve >"$tmp/out" 2>"$tmp/err"; then
    echo 'Accepted an outdated ShellCheck' >&2
    exit 1
fi
grep -q 'ShellCheck.*required' "$tmp/err"

# Functions intercept host detection and every download in this fixture.
uname() { case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; esac; }
curl() { printf 'bad archive' > "$4"; }
if LUCIDOS_NO_TOOL_DOWNLOAD=0 shellcheck_resolve >"$tmp/out" 2>"$tmp/err"; then
    echo 'Accepted an invalid archive' >&2
    exit 1
fi
grep -q 'checksum mismatch' "$tmp/err"
[[ -z "$(find "$XDG_CACHE_HOME" -type f)" ]]
