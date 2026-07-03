#!/usr/bin/env bash
# install_common.sh — the PURE, install-specific helpers install.sh shares with its
# unit test (scripts/lib/install_test.sh): how a target triple + version map to the
# download URL, how the version is resolved, and where the runtime extracts. Step 3
# of docs/plans/2026-06-30-installer-step3-download-and-run.md.
#
# These live in a lib (not inlined in install.sh) so they're unit-testable by
# sourcing, the same way stage_runtime.sh / headless_tarball.sh are — and so there
# is exactly ONE source of truth for the artifact name install.sh downloads and the
# build scripts emit. The tarball STEM comes from headless_tarball_stem and the
# TRIPLE from stage_runtime.sh's uname maps, so a download URL can never diverge
# from the artifact a release actually ships.
#
# Pure shell — no network, no filesystem writes (install_resolve_version only READS
# an optional RELEASE file). Depends on headless_tarball_stem, so the caller sources
# scripts/lib/headless_tarball.sh first (install.sh and the test both do).

# install_default_base_url <version> — the default base URL where the per-platform
# tarball + .sha256 live: the eventual GitHub Releases download path for the
# umbrella version tag. Nothing is published there yet (the CI workflow is
# artifact-only), so a default download 404s until a Release is cut — install.sh
# surfaces that and points the user at --dev / --from-tarball.
install_default_base_url() {
    printf 'https://github.com/lucidos-dev/lucidos/releases/download/v%s' "$1"
}

# install_tarball_url <base-url> <version> <triple> — the full tarball URL,
# <base>/<stem>.tar.gz, with the stem from headless_tarball_stem (so it matches the
# emitted artifact exactly). A trailing slash on <base-url> is tolerated.
install_tarball_url() {
    local base="${1%/}" version="$2" triple="$3" stem
    stem="$(headless_tarball_stem "$version" "$triple")"
    printf '%s/%s.tar.gz' "$base" "$stem"
}

# install_checksum_url <base-url> <version> <triple> — the .sha256 sidecar URL
# (the tarball URL + .sha256), matching headless_tarball_emit's sidecar name.
install_checksum_url() {
    printf '%s.sha256' "$(install_tarball_url "$1" "$2" "$3")"
}

# install_resolve_version <override> <release-file> <default> — resolve the version
# to download. Precedence: a non-empty <override> (the --version flag / LUCIDOS_VERSION)
# wins; else the trimmed contents of <release-file> when it exists and is non-empty
# (the repo-root RELEASE file, present when install.sh runs from a checkout — mirrors
# stage_runtime_version's first source); else the baked <default>. Pure: it only
# reads <release-file>.
install_resolve_version() {
    local override="$1" release_file="$2" default="$3" v=""
    if [ -n "$override" ]; then
        printf '%s' "$override"
        return 0
    fi
    if [ -n "$release_file" ] && [ -f "$release_file" ]; then
        v="$(tr -d '[:space:]' < "$release_file")"
    fi
    if [ -n "$v" ]; then
        printf '%s' "$v"
    else
        printf '%s' "$default"
    fi
}

# install_ca_bundle_candidates — the system CA-bundle files rustls-native-certs
# (the engine's outbound-TLS root source on Linux) actually reads, one per line:
# Debian/Ubuntu, Fedora/RHEL (two spellings), SUSE, then the Alpine/BSD/macOS
# fallback. install.sh's preflight warns when NONE exists (a minimal server image
# without the ca-certificates package — every outbound TLS call would fail).
# Pure: prints paths, probes nothing.
install_ca_bundle_candidates() {
    printf '%s\n' \
        /etc/ssl/certs/ca-certificates.crt \
        /etc/pki/tls/certs/ca-bundle.crt \
        /etc/pki/tls/cacert.pem \
        /etc/ssl/ca-bundle.pem \
        /etc/ssl/cert.pem
}

# install_runtime_dir <prefix> <version> <triple> — where the tarball extracts:
# <prefix>/runtime/<stem>. The stem (headless_tarball_stem) is also the tarball's
# own top-level dir, so `tar -C <prefix>/runtime` lands the 6 resources exactly here
# — which makes the idempotency check (does <runtime>/lucidos-gateway exist?) and
# step 4's uninstall a single, predictable path.
install_runtime_dir() {
    local prefix="${1%/}" version="$2" triple="$3" stem
    stem="$(headless_tarball_stem "$version" "$triple")"
    printf '%s/runtime/%s' "$prefix" "$stem"
}
