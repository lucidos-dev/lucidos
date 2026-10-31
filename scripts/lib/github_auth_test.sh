#!/usr/bin/env bash
# Offline tests: no real gh, credentials, or release actions.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/home"
export HOME="$WORK/home" PATH="$WORK/bin:$PATH" AUTH_LOG="$WORK/auth.log"
unset GH_TOKEN GH_ACCOUNT_GITHUB_COM GH_CONFIG_DIR
cat > "$WORK/bin/gh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" "config=${GH_CONFIG_DIR-unset}" >> "$AUTH_LOG"
[ "$*" = 'auth token -h github.com -u release-bot' ] || exit 99
printf '%s' "${FAKE_TOKEN-fetched-token}"
exit "${FAKE_STATUS-0}"
STUB
chmod +x "$WORK/bin/gh"
# shellcheck source=scripts/lib/github_auth.sh
source "$SCRIPT_DIR/github_auth.sh"
GH_TOKEN=provided-token
github_auth
[ "$GH_TOKEN" = provided-token ]
[ "$(bash -c 'printf %s "$GH_TOKEN"')" = provided-token ]
[ ! -e "$AUTH_LOG" ]
unset GH_TOKEN
if github_auth > "$WORK/error" 2>&1; then
    echo 'missing account accepted' >&2; exit 1
fi
grep -q GH_ACCOUNT_GITHUB_COM "$WORK/error"
[ ! -e "$AUTH_LOG" ]
export GH_ACCOUNT_GITHUB_COM=release-bot
mkdir -p "$HOME/.config/gh-lucidos"
github_auth
[ "$GH_TOKEN" = fetched-token ]
[ "$GH_CONFIG_DIR" = "$HOME/.config/gh-lucidos" ]
[ "$(wc -l < "$AUTH_LOG" | tr -d ' ')" = 2 ]
github_auth
[ "$(wc -l < "$AUTH_LOG" | tr -d ' ')" = 2 ]
unset GH_TOKEN
export GH_CONFIG_DIR="$WORK/custom"
github_auth
grep -q "config=$WORK/custom" "$AUTH_LOG"
unset GH_TOKEN
export GH_CONFIG_DIR=''
github_auth
[ "$GH_CONFIG_DIR" = '' ]
unset GH_TOKEN GH_CONFIG_DIR
rmdir "$HOME/.config/gh-lucidos"
github_auth
[ "${GH_CONFIG_DIR+x}" != x ]
unset GH_TOKEN
export FAKE_TOKEN=''
if github_auth > "$WORK/error" 2>&1; then
    echo 'empty token accepted' >&2; exit 1
fi
grep -q GH_ACCOUNT_GITHUB_COM "$WORK/error"
export FAKE_TOKEN=ignored FAKE_STATUS=1
if github_auth > "$WORK/error" 2>&1; then
    echo 'failed lookup accepted' >&2; exit 1
fi
unset GH_TOKEN GH_ACCOUNT_GITHUB_COM
# Each mode must fail at auth, before parsing/validating release inputs.
if [ -f "$ROOT/scripts/release.sh" ]; then
for mode in --verify-build --push-rc --publish-verified --publish-draft --attach-notarized; do
    if bash "$ROOT/scripts/release.sh" "$mode" > "$WORK/error" 2>&1; then
        echo "$mode accepted missing account" >&2; exit 1
    fi
    grep -q GH_ACCOUNT_GITHUB_COM "$WORK/error"
done
fi
for entry in release-to-lucidos.sh build-dmg.sh; do
    [ -f "$ROOT/scripts/$entry" ] || continue
    if bash "$ROOT/scripts/$entry" --release-attach > "$WORK/error" 2>&1; then
        echo "$entry accepted missing account" >&2; exit 1
    fi
    grep -q GH_ACCOUNT_GITHUB_COM "$WORK/error"
done
echo 'GitHub authentication tests passed'
