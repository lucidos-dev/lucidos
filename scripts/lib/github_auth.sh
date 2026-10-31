#!/usr/bin/env bash
# Resolve once in the entry point; exported GH_TOKEN pins every gh subprocess
# (including sourced helpers and child build scripts) to that identity. Callers:
# the release scripts and the e2e GitHub driver (scripts/e2e-github.sh).
github_auth() {
    if [ "${GH_CONFIG_DIR+x}" != x ] && [ -d "${HOME}/.config/gh-lucidos" ]; then
        export GH_CONFIG_DIR="${HOME}/.config/gh-lucidos"
    fi
    if [ -n "${GH_TOKEN:-}" ]; then
        export GH_TOKEN
        return 0
    fi
    if [ -z "${GH_ACCOUNT_GITHUB_COM:-}" ]; then
        echo 'ERROR: GitHub access requires GH_TOKEN or GH_ACCOUNT_GITHUB_COM; refusing gh active-account fallback.' >&2
        return 1
    fi
    local token
    if ! token="$(gh auth token -h github.com -u "$GH_ACCOUNT_GITHUB_COM")" || [ -z "$token" ]; then
        echo 'ERROR: GitHub access could not fetch a token for GH_ACCOUNT_GITHUB_COM; set GH_TOKEN or check that account and GH_CONFIG_DIR.' >&2
        return 1
    fi
    export GH_TOKEN="$token"
}
