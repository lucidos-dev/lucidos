#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=scripts/lib/dev_prerequisites.sh
source scripts/lib/dev_prerequisites.sh

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export XDG_CACHE_HOME="$tmp/cache"
mkdir -p "$tmp/crates/lucidos-app" "$tmp/browser/platform"
dp_platform() { echo apt; }
shellcheck_resolve() { echo 'ERROR: ShellCheck is required' >&2; return 1; }
uname() { echo Linux; }
python3() { [[ "$*" == "-c import venv, ensurepip" ]] && return 1; return 2; }
npx() {
    case "$*" in
        '--no-install playwright install --dry-run --only-shell chromium')
            printf 'Chrome Headless Shell (playwright chromium-headless-shell v1)\n  Install location: %s/browser\n' "$tmp"
            ;;
        '--no-install playwright install --only-shell chromium')
            if [[ ${install_fails:-} == 1 ]]; then return 1; fi
            touch "$tmp/browser/platform/chrome-headless-shell"
            chmod +x "$tmp/browser/platform/chrome-headless-shell"
            ;;
        *) return 2 ;;
    esac
}
ldd() {
    [[ "$1" == "$tmp/browser/platform/chrome-headless-shell" ]] || return 2
    printf 'libnspr4.so => not found\nlibnss3.so => not found\n'
}
if dev_prerequisites "$tmp" lint-static,ts,engine >"$tmp/out" 2>"$tmp/err"; then
    echo 'Missing prerequisites must fail' >&2
    exit 1
fi
grep -q 'ShellCheck is required' "$tmp/err"
grep -q 'python3-venv' "$tmp/err"
grep -q 'libnspr4.so' "$tmp/err"
grep -q 'libnss3.so' "$tmp/err"
grep -q 'sudo npx playwright install-deps chromium' "$tmp/err"
rm "$tmp/browser/platform/chrome-headless-shell"
if install_fails=1 dev_prerequisites "$tmp" ts 2>"$tmp/err"; then
    echo 'A missing browser must fail' >&2
    exit 1
fi
grep -q 'Chromium headless shell is missing' "$tmp/err"
dev_prerequisites "$tmp" vite
if dev_prerequisites "$tmp" agents 2>"$tmp/err"; then
    echo 'The agents suite requires Python venv support' >&2
    exit 1
fi
grep -q 'python3-venv' "$tmp/err"
if dev_prerequisites "$tmp" confinement 2>"$tmp/err"; then
    echo 'The confinement suite requires Python venv support' >&2
    exit 1
fi
grep -q 'python3-venv' "$tmp/err"
