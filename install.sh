#!/usr/bin/env bash
#
# install.sh — one-click installer for Lucidos.
#
#   curl -fsSL https://lucidos.dev/install.sh | sh
#
# DEFAULT (download) path — the curl|sh experience: no Docker, no Rust/Node, no
# clone, no compile. It detects your platform, DOWNLOADS the prebuilt headless
# runtime tarball (engine + gateway + frontend + relocatable PostgreSQL 18 +
# pgvector + sdk + system-knowhow), verifies its sha256 checksum, extracts it
# under an install
# prefix, and launches the bundled gateway (which provisions the embedded Postgres
# and spawns the engine — the same runtime model as the macOS .app). First run is
# seconds, not minutes.
#
#   The tarballs are built by .github/workflows/release-tarballs.yml on the v*
#   tag push and attached to the Release while it is still a DRAFT, which is
#   only published once all four are on it. So every visible release carries
#   them: there is no window in which a brand-new version 404s. Alternatives if
#   a download fails anyway:
#     • --version <older>       install the previous release's tarball
#     • --dev / --source        build from source (clones + compiles)
#     • --from-tarball <path>   install a tarball you built with scripts/build-headless.sh
#
# Modes:
#   (default)              download the prebuilt tarball for the detected platform and run it
#   --dev | --source       build from source: bootstrap toolchain, clone, cargo build, run.sh
#   --from-tarball <path>  install a LOCAL tarball (offline) and run it
#
# See `install.sh --help` for the full flag/env contract, and the "One-click
# install" section of README.md for the user-facing summary.
#
# ── bash re-exec guard ──────────────────────────────────────────────────────
# The installer uses bashisms (`set -o pipefail`, arrays). When started by a
# non-bash POSIX shell — e.g. `curl … | sh` on a Debian host whose /bin/sh is
# dash — re-exec under bash. On macOS /bin/sh IS bash, so the common `| sh`
# path never re-execs there. This block must parse under plain POSIX sh, so it
# uses no bashisms itself.
# The baked fallback version lives ABOVE the re-exec guard on purpose: the guard
# exports it so a re-fetched copy cannot resolve a DIFFERENT version than the
# copy the user actually piped in (see the piped branch below). release.sh
# rewrites this line in the same step that bumps RELEASE; install_test.sh and
# version_sources_test.sh assert the two match.
LUCIDOS_DEFAULT_VERSION="0.33.0"
# Where a PIPED dash run re-fetches itself from. A mirror that serves this script
# under its own domain (lucidos.dev) rewrites this line at publish time so the
# re-fetch pulls THE SAME copy, not whatever github main happens to hold.
LUCIDOS_INSTALL_URL="${LUCIDOS_INSTALL_URL:-https://raw.githubusercontent.com/lucidos-dev/lucidos/main/install.sh}"
if [ -z "${BASH_VERSION:-}" ]; then
    if command -v bash >/dev/null 2>&1; then
        if [ -f "$0" ] && [ -r "$0" ]; then
            # Started as `sh install.sh` — re-run the file under bash. The
            # adjacent RELEASE file (if any) still wins, so nothing is pinned.
            exec bash "$0" "$@"
        else
            # Piped (`curl … | sh`) — no file to re-run, so re-fetch under bash.
            #
            # Pin the version BEFORE re-fetching. Without this the re-fetched
            # copy re-resolves its own baked default, so a user who piped a
            # 0.16.0 installer from lucidos.dev silently installed whatever
            # github main was baked at (0.14.0 in the wild). Only the version is
            # carried over — an explicit LUCIDOS_VERSION / --version still wins.
            LUCIDOS_VERSION="${LUCIDOS_VERSION:-$LUCIDOS_DEFAULT_VERSION}"
            export LUCIDOS_VERSION
            # Capture first and fail loudly if the fetch came back empty, rather
            # than exec'ing an empty `bash -c ""` (a silent no-op exit 0).
            _lucidos_payload="$(curl -fsSL "$LUCIDOS_INSTALL_URL")" || _lucidos_payload=""
            if [ -z "$_lucidos_payload" ]; then
                echo "ERROR: could not re-fetch the installer from $LUCIDOS_INSTALL_URL to run it under bash." >&2
                echo "       Re-run explicitly under bash:  curl -fsSL $LUCIDOS_INSTALL_URL | bash" >&2
                exit 1
            fi
            # Shebang sniff BEFORE the payload reaches `exec bash -c`. Same
            # soft-404 hazard the helper-lib fetch guards against (see
            # _source_libs): an origin that answers an unknown path with its
            # landing page and a 200 makes `curl -fsSL` succeed and the
            # non-empty test pass, and bash then executes HTML. Assert the
            # shebang rather than reject a leading '<': fail-closed, and it is
            # the same test the front-door CI rung applies to what the origin
            # serves. Must stay POSIX sh, like the rest of this guard.
            case "$_lucidos_payload" in
                '#!'*) : ;;
                *)
                    echo "ERROR: $LUCIDOS_INSTALL_URL did not return a shell script." >&2
                    echo "       The origin likely served its 404/SPA fallback page with a 200 status." >&2
                    echo "       Re-run against a known-good origin, or download install.sh and run it directly." >&2
                    exit 1 ;;
            esac
            exec bash -c "$_lucidos_payload" bash "$@"
        fi
    else
        echo "ERROR: this installer requires bash, which was not found on PATH." >&2
        echo "       Install bash and re-run, or run with: curl -fsSL <url>/install.sh | bash" >&2
        exit 1
    fi
fi

set -euo pipefail

# ── configuration (all overridable via environment) ─────────────────────────
# Download-path config (the default mode). LUCIDOS_DEFAULT_VERSION and
# LUCIDOS_INSTALL_URL are set above the re-exec guard.
LUCIDOS_VERSION="${LUCIDOS_VERSION:-}"                          # version to download; empty = resolve (RELEASE file when run from a checkout, else the baked default)
LUCIDOS_RELEASE_BASE_URL="${LUCIDOS_RELEASE_BASE_URL:-}"       # base URL holding lucidos-<version>-<triple>.tar.gz + .sha256; empty = the GitHub Releases default
LUCIDOS_PREFIX="${LUCIDOS_PREFIX:-$HOME/.lucidos}"             # install home; the runtime extracts to $LUCIDOS_PREFIX/runtime/<stem>/ (SHARED across instances)
LUCIDOS_INSTANCE_EXPLICIT="${LUCIDOS_INSTANCE:+1}"         # non-empty if the user chose an instance via env LUCIDOS_INSTANCE (or --name, set in parse_args)
LUCIDOS_INSTANCE="${LUCIDOS_INSTANCE:-default}"             # instance slug (--name); its data lives at $LUCIDOS_PREFIX/<slug>/
LUCIDOS_GATEWAY_DATA="${LUCIDOS_GATEWAY_DATA:-}"            # override the instance data dir (registry + embedded PG + fastembed + logs); empty = $LUCIDOS_PREFIX/<slug>
LUCIDOS_PORT_EXPLICIT="${LUCIDOS_PORT:+1}"                  # non-empty if the user PINNED a port via env LUCIDOS_PORT (or --port, set in parse_args)
LUCIDOS_PORT="${LUCIDOS_PORT:-5252}"                         # gateway port — a mutable PROPERTY of the instance (5252 = the packaged gateway's default)
LUCIDOS_FORCE="${LUCIDOS_FORCE:-}"                            # set to 1 to re-download/re-extract even if the runtime is already present
LUCIDOS_NO_LAUNCH="${LUCIDOS_NO_LAUNCH:-}"                    # set to 1 to install without launching (start it later)
LUCIDOS_NO_SERVICE="${LUCIDOS_NO_SERVICE:-}"                 # set to 1 (or --no-service) to launch in the FOREGROUND instead of registering a user service
LUCIDOS_HEALTH_TIMEOUT="${LUCIDOS_HEALTH_TIMEOUT:-120}"     # seconds to wait for the registered gateway's health endpoint before failing loud
LUCIDOS_FROM_SOURCE="${LUCIDOS_FROM_SOURCE:-}"               # set to 1 (or pass --dev/--source) to build from source instead of downloading
LUCIDOS_FROM_TARBALL="${LUCIDOS_FROM_TARBALL:-}"            # path to a LOCAL tarball to install (or pass --from-tarball <path>)
LUCIDOS_UNINSTALL=""                                          # set by --uninstall: stop + unregister an instance (delegates to uninstall.sh)
LUCIDOS_LIST=""                                               # set by --list: list installed instances (delegates to uninstall.sh)
LUCIDOS_ALL="${LUCIDOS_ALL:-}"                               # with --uninstall: act on ALL instances (forwarded to uninstall.sh as --all)
LUCIDOS_PURGE="${LUCIDOS_PURGE:-}"                           # with --uninstall: also delete the instance data (and, with --all, the shared runtime)
LUCIDOS_LIB_BASE_URL="${LUCIDOS_LIB_BASE_URL:-}"            # base URL for the helper libs when piped (curl|sh); empty = derived from LUCIDOS_INSTALL_URL
LUCIDOS_UNINSTALL_URL="${LUCIDOS_UNINSTALL_URL:-}"          # URL for uninstall.sh when piped (curl|sh); empty = derived from LUCIDOS_INSTALL_URL
LUCIDOS_TLS_CERT="${LUCIDOS_TLS_CERT:-}"                    # opt-in TLS (--tls-cert): cert path; with --tls-key the gateway serves https
LUCIDOS_TLS_KEY="${LUCIDOS_TLS_KEY:-}"                      # opt-in TLS (--tls-key): private-key path; both-or-neither
LUCIDOS_BIND="${LUCIDOS_BIND:-}"                            # optional (--bind): all | loopback | <IP> — written to the machine-global ~/.lucidos/network.toml

# Source-build (--dev) config — preserves the legacy behavior verbatim.
LUCIDOS_REPO_URL="${LUCIDOS_REPO_URL:-https://github.com/lucidos-dev/lucidos.git}"
LUCIDOS_REF="${LUCIDOS_REF:-}"                                   # branch/tag/sha; empty = repo default
LUCIDOS_HOME="${LUCIDOS_HOME:-$HOME/lucidos}"                    # --dev only: where the repo is cloned (distinct from LUCIDOS_PREFIX)
LUCIDOS_WORKSPACE="${LUCIDOS_WORKSPACE:-$HOME/workspaces/lucidos}" # --dev only: the workspace (data) directory
LUCIDOS_DEBUG_BUILD="${LUCIDOS_DEBUG_BUILD:-}"                  # --dev only: set to 1 for a faster (debug) engine build; default is a release build
LUCIDOS_SKIP_DEPS="${LUCIDOS_SKIP_DEPS:-}"                      # --dev only: set to 1 to skip dependency bootstrap

# The pure helper libs install.sh shares with the build scripts + its unit test.
# Sourced from the checkout when present, else fetched (see source_install_libs).
LUCIDOS_LIBS="stage_runtime.sh headless_tarball.sh install_common.sh"

# Provider config — if supplied, made visible to the launched runtime so the
# engine boots configured. With none set, the gateway boots WITHOUT a provider
# (clear onboarding state); configure one in Settings → Providers afterwards. The
# --dev path additionally persists these to <workspace>/data/.env.
#   OPENAI_API_KEY   — GPT models via OpenAI
#   VERTEX_PROJECT_ID + (optional) VERTEX_REGION — Claude/Gemini via Vertex AI
OPENAI_API_KEY="${OPENAI_API_KEY:-}"
VERTEX_PROJECT_ID="${VERTEX_PROJECT_ID:-}"
VERTEX_REGION="${VERTEX_REGION:-}"

# ── output helpers ──────────────────────────────────────────────────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_BOLD="$(printf '\033[1m')"; C_DIM="$(printf '\033[2m')"
    C_BLUE="$(printf '\033[34m')"; C_GREEN="$(printf '\033[32m')"
    C_YELLOW="$(printf '\033[33m')"; C_RED="$(printf '\033[31m')"; C_RESET="$(printf '\033[0m')"
else
    C_BOLD=""; C_DIM=""; C_BLUE=""; C_GREEN=""; C_YELLOW=""; C_RED=""; C_RESET=""
fi

step() { printf '%s==>%s %s%s%s\n' "$C_BLUE" "$C_RESET" "$C_BOLD" "$*" "$C_RESET"; }
info() { printf '    %s\n' "$*"; }
ok()   { printf '%s  ✓%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
warn() { printf '%s  !%s %s\n' "$C_YELLOW" "$C_RESET" "$*" >&2; }
die()  { printf '\n%sERROR:%s %s\n' "$C_RED" "$C_RESET" "$*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

# ── helper library sourcing (download / from-tarball modes) ──────────────────
# The directory install.sh was invoked from, if it is a real file (a checkout or
# `bash install.sh`). Empty when piped (`curl … | sh`).
installer_self_dir() {
    local src=""
    if [ -n "${BASH_SOURCE:-}" ] && [ -f "${BASH_SOURCE[0]:-}" ]; then
        src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    fi
    printf '%s' "$src"
}

# _source_libs <name…> — source each named lib from the checkout's scripts/lib
# when present (a real file run, i.e. a checkout), else fetch the tiny pure files
# from the same ref the installer came from. ONE source of truth with the build
# scripts (no divergent os/arch map, no inlined copy that can drift).
_source_libs() {
    local self_dir lib_dir name base first
    self_dir="$(installer_self_dir)"
    if [ -n "$self_dir" ] && [ -f "$self_dir/scripts/lib/stage_runtime.sh" ]; then
        lib_dir="$self_dir/scripts/lib"
    else
        # Piped install (no checkout): fetch the helper libs from the same ref as
        # install.sh itself. These are tiny, pure, public-mirror-safe files.
        base="${LUCIDOS_LIB_BASE_URL:-${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib}"
        lib_dir="$(mktemp -d)"
        step "Fetching installer helper libraries"
        info "$base"
        for name in "$@"; do
            curl -fsSL "$base/$name" -o "$lib_dir/$name" \
                || die "Could not fetch helper lib '$name' from $base. Run install.sh from a checkout of the repo, or use --dev to build from source."
            [ -s "$lib_dir/$name" ] \
                || die "Fetched helper lib '$name' from $base is empty. Run install.sh from a checkout, or use --dev to build from source."
            # Content sniff BEFORE the file reaches `.` (source). Neither check
            # above can see a SOFT-404: an origin that answers an unknown path
            # with its landing page and a 200 status makes `curl -fsSL` succeed
            # and the non-empty test pass, and the installer then executes HTML
            # as shell. That is exactly the 2026-07-29 clean-machine failure
            # (ubuntu:22.04, `curl -fsSL lucidos.dev/install.sh | sh`): the
            # Cloudflare Pages SPA fallback served the landing page for
            # scripts/lib/*.sh, and bash died on `<!DOCTYPE html>`. Defence in
            # depth — do NOT drop this because the publisher now uploads the
            # libs; any wrong or hijacked origin can still soft-404. Reject when
            # the first non-blank line opens a tag (<!DOCTYPE, <html, <?xml),
            # and reject rather than warn: sourcing an unknown payload is the
            # failure we are preventing.
            first="$(awk 'NF { sub(/^[[:space:]]+/, ""); print; exit }' "$lib_dir/$name")"
            case "$first" in
                '<'*) die "Helper lib '$name' from $base is HTML, not shell.
       The origin likely returned its 404/SPA fallback page with a 200 status.
       Re-run from a checkout of the repo, or use --dev to build from source." ;;
            esac
        done
    fi
    for name in "$@"; do
        # shellcheck disable=SC1090
        . "$lib_dir/$name" || die "Could not source helper lib '$name' from $lib_dir."
    done
}

# The triple/stem/URL helpers (download + from-tarball stem checks).
source_install_libs() {
    # shellcheck disable=SC2086
    _source_libs $LUCIDOS_LIBS
}

# The service templating/detection helpers (sourced lazily, only when we are
# actually about to launch/register — keeps the offline from-tarball verify/
# extract path from fetching anything it doesn't need). install_common.sh rides
# along for finish_install's preflight (install_ca_bundle_candidates): on the
# download path it was already sourced (re-sourcing pure helpers is a no-op),
# and on the from-tarball path this is where it first loads.
source_service_lib() { _source_libs service.sh install_common.sh; }

# ── checksum verification (download / from-tarball; OS-aware, fail-closed) ────
ensure_checksum_tool() {
    have shasum || have sha256sum || die "Need 'shasum' (macOS) or 'sha256sum' (Linux) for mandatory checksum verification. Install one and re-run."
}

# compute_sha256 <file> — print the lowercase hex SHA-256 of <file>. Non-zero if
# no tool is available.
compute_sha256() {
    if have shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif have sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    else
        return 2
    fi
}

# verify_checksum_against_sidecar <file> <sidecar> — recompute sha256(<file>) and
# compare it to the hex in <sidecar> (its first whitespace-separated field). This
# is filename-independent (works even if the tarball was renamed) and OS-aware.
# Returns non-zero on a mismatch or when no hashing tool is available.
verify_checksum_against_sidecar() {
    local file="$1" sidecar="$2" want got
    want="$(awk '{print $1; exit}' "$sidecar" 2>/dev/null || true)"
    [ -n "$want" ] || return 2
    got="$(compute_sha256 "$file")" || return 2
    [ "$got" = "$want" ]
}

# ── runtime extraction + activation (download / from-tarball) ────────────────
# extract_tarball <tarball> <stem> — extract the headless tarball into
# $LUCIDOS_PREFIX/runtime/. Its sole top-level dir IS <stem>, so it lands at
# $LUCIDOS_PREFIX/runtime/<stem>. Idempotent: a present runtime is left alone
# unless LUCIDOS_FORCE is set. Prints the runtime dir on stdout.
extract_tarball() {
    local tarball="$1" stem="$2"
    # `:?` because --prefix accepts an empty argument, and every rm -rf
    # below is derived from this path.
    local runtime_parent="${LUCIDOS_PREFIX:?}/runtime"
    local runtime_dir="$runtime_parent/$stem"

    if [ -x "$runtime_dir/lucidos-gateway" ] && [ -z "$LUCIDOS_FORCE" ]; then
        ok "Runtime already installed at $runtime_dir (use --force to re-extract)." >&2
        printf '%s\n' "$runtime_dir"
        return 0
    fi

    step "Extracting runtime → $runtime_dir" >&2
    mkdir -p "$runtime_parent" || die "Could not create $runtime_parent"

    # Extract beside the live tree and swap, never delete then extract. This
    # runtime is SHARED: every registered instance's service runs
    # `<prefix>/runtime/current/<binary>` and `current` points here, so a
    # `--force` re-install used to remove those binaries for the whole duration
    # of the untar. A KeepAlive respawn inside that window fails on a missing
    # file, and every other instance loses its on-disk image.
    # Both names carry this shell's pid, so they are unique to this run and need
    # no pre-clean. Do NOT add a `.staging-$stem.*` sweep here to reclaim a
    # killed run's leftovers: that glob is scoped to the stem, not the pid, so a
    # second concurrent install deletes THIS run's `.previous-` rollback copy.
    # The restore below is then skipped, and the shared runtime is left missing,
    # which is the outcome this whole staging dance exists to prevent.
    local staging="$runtime_parent/.staging-$stem.$$"
    local previous="$runtime_parent/.previous-$stem.$$"
    mkdir -p "$staging" || die "Could not create $staging"
    tar -xzf "$tarball" -C "$staging" || {
        rm -rf "$staging"
        die "Failed to extract $tarball into $staging"
    }
    [ -x "$staging/$stem/lucidos-gateway" ] || {
        rm -rf "$staging"
        die "Extracted runtime is missing the gateway binary at $stem/lucidos-gateway. The tarball is not a valid Lucidos runtime."
    }
    if [ -e "$runtime_dir" ] && ! mv "$runtime_dir" "$previous"; then
        rm -rf "$staging"
        die "Could not move the existing runtime aside at $runtime_dir"
    fi
    if ! mv "$staging/$stem" "$runtime_dir"; then
        # Put the old runtime back: a failed re-install must not leave the
        # shared path missing for the instances still pointing at it.
        [ -e "$previous" ] && mv "$previous" "$runtime_dir"
        rm -rf "$staging"
        die "Could not activate the extracted runtime at $runtime_dir"
    fi
    rm -rf "$staging" "$previous"
    ok "Extracted $stem" >&2
    printf '%s\n' "$runtime_dir"
}

# link_current <runtime-dir> — (re)point $LUCIDOS_PREFIX/runtime/current at the
# active runtime, a stable path for step 4's service files. `ln -sfn` replaces an
# existing symlink atomically on both BSD (macOS) and GNU (Linux) ln.
link_current() {
    local runtime_dir="$1" link="$LUCIDOS_PREFIX/runtime/current"
    ln -sfn "$runtime_dir" "$link" 2>/dev/null || ln -sf "$runtime_dir" "$link" 2>/dev/null || true
}

# ── download install (default mode) ─────────────────────────────────────────
run_download_install() {
    have curl || die "curl is required for the download path. Install curl, or use --dev to build from source."
    have tar  || die "tar is required."
    ensure_checksum_tool
    source_install_libs

    local triple version base_url stem url sha_url tmp runtime_dir
    triple="$(stage_runtime_host_triple)" \
        || die "Unsupported platform $(uname -s)/$(uname -m). Lucidos publishes macOS (arm64/x86_64) and Linux (x86_64/aarch64) tarballs."
    version="$(install_resolve_version "$LUCIDOS_VERSION" "$(installer_self_dir)/RELEASE" "$LUCIDOS_DEFAULT_VERSION")"
    if [ -n "$LUCIDOS_RELEASE_BASE_URL" ]; then
        base_url="$LUCIDOS_RELEASE_BASE_URL"
    else
        base_url="$(install_default_base_url "$version")"
    fi
    stem="$(headless_tarball_stem "$version" "$triple")"
    url="$(install_tarball_url "$base_url" "$version" "$triple")"
    sha_url="$(install_checksum_url "$base_url" "$version" "$triple")"
    runtime_dir="$(install_runtime_dir "$LUCIDOS_PREFIX" "$version" "$triple")"

    step "Installing Lucidos $version ($triple)"
    info "prefix:  $LUCIDOS_PREFIX"
    info "runtime: $runtime_dir"

    if [ -x "$runtime_dir/lucidos-gateway" ] && [ -z "$LUCIDOS_FORCE" ]; then
        ok "Runtime already installed (use --force to re-download)."
        link_current "$runtime_dir"
        finish_install "$runtime_dir"
        return 0
    fi

    tmp="$(mktemp -d)"
    step "Downloading $stem.tar.gz"
    info "$url"
    if ! curl -fSL -o "$tmp/$stem.tar.gz" "$url" 2>/dev/null; then
        rm -rf "$tmp"
        download_failed "$url"
    fi
    # Checksum is MANDATORY on the download path — fail closed if the sidecar is
    # missing OR the hash does not match.
    if ! curl -fSL -o "$tmp/$stem.tar.gz.sha256" "$sha_url" 2>/dev/null; then
        rm -rf "$tmp"
        die "Downloaded the tarball but could NOT fetch its checksum sidecar:
       $sha_url
       Checksum verification is mandatory on the download path; aborting. Use
       --from-tarball <path> for a local artifact you trust, or --dev to build."
    fi
    step "Verifying checksum"
    if ! verify_checksum_against_sidecar "$tmp/$stem.tar.gz" "$tmp/$stem.tar.gz.sha256"; then
        rm -rf "$tmp"
        die "Checksum verification FAILED for $stem.tar.gz — refusing to install a corrupt or tampered download."
    fi
    ok "Checksum verified"

    runtime_dir="$(extract_tarball "$tmp/$stem.tar.gz" "$stem")"
    rm -rf "$tmp"
    link_current "$runtime_dir"
    finish_install "$runtime_dir"
}

download_failed() {
    local url="$1"
    die "Download failed: $url

       Likely causes:
         • No tarball was published for this platform. The published triples are
           macOS arm64/x86_64 and Linux x86_64/aarch64. A release is held as a
           draft until all four are attached, so this means it was published
           deliberately incomplete or the asset was removed.
         • The network / a proxy blocked github.com.

       Working alternatives:
         • Install the previous release (or retry in a few minutes):
             curl -fsSL $LUCIDOS_INSTALL_URL | sh -s -- --version <older-version>
         • Build from source:
             curl -fsSL $LUCIDOS_INSTALL_URL | sh -s -- --dev
         • Install a tarball you built with scripts/build-headless.sh:
             ./install.sh --from-tarball /path/to/lucidos-<version>-<triple>.tar.gz

       Or point LUCIDOS_RELEASE_BASE_URL / LUCIDOS_VERSION at a published artifact."
}

# ── local tarball install (--from-tarball) ──────────────────────────────────
run_from_tarball_install() {
    local tarball="$LUCIDOS_FROM_TARBALL" stem runtime_dir sidecar
    have tar || die "tar is required."
    [ -f "$tarball" ] || die "--from-tarball: file not found: $tarball"

    # The tarball's sole top-level dir IS the stem (lucidos-<version>-<triple>).
    # `head -1` closes the pipe early; on a large tarball that SIGPIPEs `tar`
    # (exit 141), which under `set -euo pipefail` would abort the whole installer.
    # `|| true` keeps the captured first line while swallowing that signal — the
    # empty-check below still catches a genuinely unreadable tarball.
    stem="$(tar -tzf "$tarball" 2>/dev/null | head -1 | sed 's,/.*,,')" || true
    [ -n "$stem" ] || die "--from-tarball: '$tarball' is not a readable gzip tarball."
    case "$stem" in
        lucidos-*) ;;
        *) die "--from-tarball: top-level dir '$stem' is not a lucidos-<version>-<triple> runtime tree." ;;
    esac

    step "Installing Lucidos from $tarball"
    info "prefix:  $LUCIDOS_PREFIX"
    info "runtime: $LUCIDOS_PREFIX/runtime/$stem"

    # Verify the adjacent sidecar when present (fail closed on mismatch); warn if
    # absent (a local artifact the user explicitly pointed at).
    sidecar="$tarball.sha256"
    if [ -f "$sidecar" ]; then
        ensure_checksum_tool
        step "Verifying checksum"
        if ! verify_checksum_against_sidecar "$tarball" "$sidecar"; then
            die "Checksum verification FAILED for $tarball (sidecar $sidecar) — refusing to install."
        fi
        ok "Checksum verified"
    else
        warn "No checksum sidecar ($sidecar) next to the tarball — installing WITHOUT verification."
    fi

    runtime_dir="$(extract_tarball "$tarball" "$stem")"
    link_current "$runtime_dir"
    finish_install "$runtime_dir"
}

# ── post-extract validation (download / from-tarball) ────────────────────────
# verify_runtime_executes <runtime-dir> — run the extracted gateway once
# (`--build-id` prints the baked id and exits before any runtime/port/PG touch)
# to prove the prebuilt binaries actually execute on THIS machine. Catches, at
# install time instead of as an opaque service crash-loop later: a glibc older
# than the tarball's build floor ("version 'GLIBC_2.xx' not found"), a wrong-arch
# tarball, and a missing dynamic loader.
verify_runtime_executes() {
    local runtime_dir="$1" out libc=""
    if out="$("$runtime_dir/lucidos-gateway" --build-id 2>&1)"; then
        ok "Runtime binaries execute on this machine (gateway build $out)"
        return 0
    fi
    libc="$( (ldd --version 2>/dev/null || true) | head -1)"
    die "The installed runtime binaries do not run on this machine:
       \$ $runtime_dir/lucidos-gateway --build-id
       ${out:-<no output>}
${libc:+       This system reports: $libc
}       Lucidos Linux tarballs are built against glibc 2.35 (the Ubuntu 22.04
       floor): Ubuntu 22.04+, Debian 12+, Fedora 36+, RHEL/Rocky/Alma 10+.
       RHEL 9 and its rebuilds are BELOW it (EL9 pins glibc 2.34 for its whole
       lifecycle). Upgrade the OS, or build from source instead:
         curl -fsSL $LUCIDOS_INSTALL_URL | sh -s -- --dev"
}

# preflight_runtime_deps — warn (never fail) about host deps the RUNTIME needs
# that a minimal server image most often lacks: `git` (the engine shells out for
# every git operation — coding-agent threads, repository features) and a system
# CA bundle (rustls reads the system store for outbound TLS: LLM providers, the
# embedding-model download, web push). The install itself proceeds either way.
preflight_runtime_deps() {
    have git || warn "git not found on PATH — coding-agent threads and repository features will not work until you install git."
    if [ "$(uname -s)" = "Linux" ] && ! system_ca_bundle_present; then
        warn "No system CA bundle found — outbound TLS (LLM providers, the memory-model download, web push) will fail. Install your distro's ca-certificates package."
    fi
    return 0
}

# system_ca_bundle_present — true if any CA bundle rustls-native-certs reads
# exists (the candidate list lives in install_common.sh — one source of truth),
# or the user points at one explicitly via SSL_CERT_FILE.
system_ca_bundle_present() {
    local ca
    [ -n "${SSL_CERT_FILE:-}" ] && [ -f "$SSL_CERT_FILE" ] && return 0
    while IFS= read -r ca; do
        [ -f "$ca" ] && return 0
    done <<EOF
$(install_ca_bundle_candidates)
EOF
    return 1
}

# ── launch / register the extracted runtime ──────────────────────────────────
# finish_install <runtime-dir> — the post-extract step. Validates the runtime
# executes here, warns about missing host runtime deps, then resolves the
# instance (slug → data dir; port). With --no-launch, just print how to start.
# Otherwise decide between registering a user SERVICE (the default — always-on,
# survives terminal-close + reboot) and a FOREGROUND launch (--no-service, or
# graceful degrade when no service manager is available).
finish_install() {
    local runtime_dir="$1" manager decision
    source_service_lib
    service_is_instance_name "$LUCIDOS_INSTANCE" \
        || die "--name must be a slug of lowercase letters/digits/dashes and not a reserved name (got '$LUCIDOS_INSTANCE')."
    validate_remote_access_flags
    macos_clt_preflight
    verify_runtime_executes "$runtime_dir"
    preflight_runtime_deps

    if [ -n "$LUCIDOS_NO_LAUNCH" ]; then
        # No probing on --no-launch; just resolve the data dir for the message.
        # --bind / --tls-* are validated above but applied only on a real
        # launch/register — --no-launch must not touch machine-global config.
        [ -n "$LUCIDOS_GATEWAY_DATA" ] || LUCIDOS_GATEWAY_DATA="$(service_instance_data_dir "$LUCIDOS_PREFIX" "$LUCIDOS_INSTANCE")"
        print_installed "$runtime_dir"
        return 0
    fi

    apply_bind_config
    manager="$(service_detect_manager)"
    decision="$(service_compose_decision "$LUCIDOS_NO_SERVICE" "$manager")"
    resolve_instance

    if [ "$decision" = "service" ]; then
        register_service "$runtime_dir" "$manager"
    else
        if [ -n "$LUCIDOS_NO_SERVICE" ]; then
            info "Service registration skipped (--no-service) — launching in the foreground."
        elif [ "$manager" = "none" ]; then
            warn "No supported user service manager (launchd / systemd --user) detected — launching in the foreground instead."
            info "Re-run on a machine with launchd (macOS) or systemd --user (Linux) to install the always-on service."
        fi
        launch_runtime "$runtime_dir"
    fi
}

# ── remote access (--bind / --tls-cert / --tls-key) ──────────────────────────
# validate_remote_access_flags — fail fast on a half-configured/unreadable TLS
# pair (validate_tls_config, below with the other TLS helpers) or a bind value
# the gateway would silently degrade to loopback (net_config::parse_bind_value
# warns + falls back — the installer refuses instead, because a user passing
# --bind believes they exposed it).
validate_remote_access_flags() {
    validate_tls_config
    if [ -n "$LUCIDOS_BIND" ]; then
        service_is_bind_value "$LUCIDOS_BIND" \
            || die "--bind must be 'all', 'loopback', or a literal IP address (got '$LUCIDOS_BIND')."
    fi
}

# apply_bind_config — write --bind into the machine-global ~/.lucidos/
# network.toml (the SAME durable knob the picker's Settings → Network access
# edits — never unit env, which would permanently shadow the file since env
# beats it in the resolution order). Machine-global on purpose: warn when
# changing an existing different value, since every gateway on this machine
# picks it up at its next restart.
apply_bind_config() {
    [ -n "$LUCIDOS_BIND" ] || return 0
    local path existing="" old
    path="$(service_network_toml_path "$HOME")"
    if [ -f "$path" ]; then
        existing="$(cat "$path" 2>/dev/null || true)"
        old="$(service_network_toml_bind "$existing")"
        if [ -n "$old" ] && [ "$old" != "$LUCIDOS_BIND" ]; then
            warn "Updating machine-global $path bind: '$old' → '$LUCIDOS_BIND' (every Lucidos gateway on this machine picks it up at its next restart)."
        fi
    fi
    service_write_network_toml "$HOME" "$LUCIDOS_BIND" || die "Could not write $path"
    ok "Gateway network bind set to '$LUCIDOS_BIND' ($path)"
}

# print_remote_access_hints — how to reach this instance from other devices,
# scheme- and bind-aware. Push notifications (and the PWA) need a SECURE
# origin: https or localhost — plain http://<host> can't register them, so the
# hints lead with the two zero-config secure paths.
print_remote_access_hints() {
    local scheme; scheme="$(install_url_scheme)"
    printf '\n'
    if [ -n "$LUCIDOS_BIND" ] && [ "$LUCIDOS_BIND" != "loopback" ]; then
        printf '  Remote:   bound to %s — open %s://<this-host>:%s/ from your devices.\n' "$LUCIDOS_BIND" "$scheme" "$LUCIDOS_PORT"
        if [ "$scheme" = "http" ]; then
            printf '            %sNote:%s browsers grant push notifications + PWA install only on a\n' "$C_YELLOW" "$C_RESET"
            printf '            secure origin (https or localhost) — use an SSH tunnel, tailscale\n'
            printf '            serve, or re-run with --tls-cert/--tls-key for full remote function.\n'
        fi
    else
        printf '  Remote:   listens on localhost only (secure default). To use it from other devices:\n'
        printf '    ssh -L %s:localhost:%s <this-host>   # full app incl. push, zero config\n' "$LUCIDOS_PORT" "$LUCIDOS_PORT"
        printf '    tailscale serve --bg %s              # trusted HTTPS on your tailnet\n' "$LUCIDOS_PORT"
        printf '    re-run with --bind all [--tls-cert <crt> --tls-key <key>]\n'
    fi
}

# resolve_instance — resolve LUCIDOS_PORT (the instance's mutable port property)
# and LUCIDOS_GATEWAY_DATA (the instance data dir) for slug
# $LUCIDOS_INSTANCE. Idempotent: an existing instance with no pinned port reuses
# its recorded port; a pinned --port sets/changes it (fail-closed if a foreigner
# holds it); a brand-new instance auto-picks the first free port from the default.
resolve_instance() {
    local data portfile want="$LUCIDOS_PORT" p picked=""
    [ -n "$LUCIDOS_GATEWAY_DATA" ] || LUCIDOS_GATEWAY_DATA="$(service_instance_data_dir "$LUCIDOS_PREFIX" "$LUCIDOS_INSTANCE")"
    data="$LUCIDOS_GATEWAY_DATA"
    portfile="$(service_instance_port_file "$data")"

    if [ -n "$LUCIDOS_PORT_EXPLICIT" ]; then
        service_is_port_number "$want" || die "--port must be a number 1..65535 (got '$want')."
        # Setting/changing this instance's port. OK if free, or if WE already hold
        # it (this instance's recorded port == want); else a foreigner has it.
        if service_port_in_use "$want" && ! port_is_ours "$portfile" "$want"; then
            die "Port $want is already in use by another process (the Lucidos .app, a dev
       gateway, or something else) and is not this instance's port. Choose a free
       --port, or use --no-service."
        fi
        LUCIDOS_PORT="$want"
    elif [ -f "$portfile" ]; then
        # Existing instance, no pinned port → keep the port it already uses.
        LUCIDOS_PORT="$(tr -d '[:space:]' < "$portfile" 2>/dev/null || true)"
        service_is_port_number "$LUCIDOS_PORT" || LUCIDOS_PORT="$want"
    else
        # Brand-new instance → first free port from the default upward.
        while IFS= read -r p; do
            if ! service_port_in_use "$p"; then picked="$p"; break; fi
        done <<EOF
$(service_port_candidates "$want" 64)
EOF
        [ -n "$picked" ] || die "Could not find a free port in $want..$((want+63)). Pass --port explicitly."
        [ "$picked" = "$want" ] || warn "Port $want is in use — selected free port $picked for instance '$LUCIDOS_INSTANCE'."
        LUCIDOS_PORT="$picked"
    fi
}

# port_is_ours <portfile> <port> — true if this instance already records <port>
# as its own (so a re-register on the same port isn't a foreign collision).
port_is_ours() {
    local portfile="$1" port="$2"
    [ -f "$portfile" ] && [ "$(tr -d '[:space:]' < "$portfile" 2>/dev/null || true)" = "$port" ]
}

# record_instance_port <data>: mark this instance as INSTALLED by recording its
# current port at <data>/port. That marker is the whole of instance discovery.
# service_list_instance_names lists exactly the <prefix>/*/ dirs carrying one, so
# it is what makes an instance visible to `uninstall.sh --list` and removable by
# `--all` / `--all --purge`, and what makes a bare re-run reuse this port.
#
# BOTH launch shapes must write it. Registering a service and launching in the
# foreground are two ways to finish the same install, and an install that leaves
# a data dir behind has to be uninstallable either way. Only the ORDERING differs
# between the two call sites, and each has its own reason: see them.
record_instance_port() {
    printf '%s\n' "$LUCIDOS_PORT" > "$(service_instance_port_file "$1")" 2>/dev/null || true
}

# launch_runtime <runtime-dir> — run the bundled gateway in the FOREGROUND with
# the same env crates/lucidos-app/src/desktop.rs::spawn_gateway sets (sourced from
# the shared service_runtime_env_pairs — one source of truth). Uses the SHARED
# runtime; the gateway provisions the embedded Postgres cluster and spawns the
# engine; its with_pg_libpath sets LD_LIBRARY_PATH/DYLD_LIBRARY_PATH from
# LUCIDOS_PG_LIB_DIR for every PG subprocess, so we only point the PG dirs.
launch_runtime() {
    local runtime_dir="$1" data="$LUCIDOS_GATEWAY_DATA" pair
    mkdir -p "$data" "$data/fastembed" || die "Could not create the gateway data dir $data"

    # Record the instance port BEFORE the exec below, which never returns. The
    # service path writes the same marker AFTER its unit, deliberately; here
    # there is no registration that can fail, and the data dir already exists, so
    # the only place left to write it is before we hand the process over.
    # Without this a degraded/foreground install was invisible to
    # `uninstall.sh --list` and unremovable by `--all --purge`, which returns
    # early when no instance carries a marker: the data dir AND the shared
    # runtime survived a full purge.
    record_instance_port "$data"

    # Make supplied provider creds visible to the gateway (engines inherit them).
    [ -n "$OPENAI_API_KEY" ]    && export OPENAI_API_KEY
    [ -n "$VERTEX_PROJECT_ID" ] && export VERTEX_PROJECT_ID
    [ -n "$VERTEX_REGION" ]     && export VERTEX_REGION

    print_running "$runtime_dir"

    # Give the gateway a writable CWD (it caches the embedding model under
    # FASTEMBED_CACHE_DIR and writes its registry under LUCIDOS_GATEWAY_DATA).
    cd "$data"
    local -a envargs=()
    while IFS= read -r pair; do
        [ -n "$pair" ] && envargs+=("$pair")
    done <<EOF
$(append_tls_env "$(service_runtime_env_pairs "$runtime_dir" "$data" "$LUCIDOS_PORT")")
EOF
    exec env "${envargs[@]}" "$(service_runtime_program "$runtime_dir")"
}

# ── service registration (default) ───────────────────────────────────────────
# register_service <runtime-dir> <manager> — write + load the user service unit
# for instance $LUCIDOS_INSTANCE (launchd LaunchAgent on macOS, systemd --user
# unit on Linux), then health-check it. The service points at the SHARED
# <prefix>/runtime/current symlink, so a later install just re-links current and
# every instance's unit keeps working.
register_service() {
    local runtime_dir="$1" manager="$2"
    local data="$LUCIDOS_GATEWAY_DATA" prefix="$LUCIDOS_PREFIX" slug="$LUCIDOS_INSTANCE"
    mkdir -p "$data" "$data/fastembed" "$(service_log_dir "$data")" \
        || die "Could not create the instance data/log dirs at $data"

    local runtime_root gateway_bin env_block mode
    runtime_root="$(service_runtime_root "$prefix")"
    gateway_bin="$(service_runtime_program "$runtime_root")"
    env_block="$(service_runtime_env_pairs "$runtime_root" "$data" "$LUCIDOS_PORT")"
    env_block="$(append_tls_env "$env_block")"
    env_block="$(append_provider_creds "$env_block")"
    # Bake provider secrets only at mode 600 (the always-on service must carry
    # them); with none supplied the gateway boots into the no-provider onboarding
    # state, exactly like the .app, and the unit stays world-readable (644).
    if [ -n "$OPENAI_API_KEY" ] || [ -n "$VERTEX_PROJECT_ID" ] || [ -n "$VERTEX_REGION" ]; then
        mode=600
    else
        mode=644
    fi

    step "Registering instance '$slug' as a user service ($manager, port $LUCIDOS_PORT)"
    case "$manager" in
        launchd)      register_launchd "$gateway_bin" "$data" "$env_block" "$mode" "$slug" ;;
        systemd-user) register_systemd "$gateway_bin" "$data" "$env_block" "$mode" "$slug" ;;
        *) die "register_service: unknown manager '$manager'" ;;
    esac
    # Record the instance's current port (marks it for listing; a bare re-run
    # reuses it). Written after the unit so a failed registration leaves no marker.
    record_instance_port "$data"

    if have curl; then
        step "Waiting for the gateway to come up"
        if service_health_wait "$LUCIDOS_PORT" "$LUCIDOS_HEALTH_TIMEOUT" 1 "$(install_url_scheme)"; then
            ok "Gateway healthy on port $LUCIDOS_PORT"
        else
            die "The service was registered but the gateway did not answer
       $(install_url_scheme)://localhost:$LUCIDOS_PORT/ within ${LUCIDOS_HEALTH_TIMEOUT}s. It will keep retrying
       (KeepAlive / Restart=always). Check the logs:
$(service_log_hint "$manager" "$data" "$slug")
       then re-open the URL, or run $(uninstall_hint "$slug") to remove it."
        fi
    else
        info "curl not found — skipping the post-register health check."
    fi
    print_service_running "$runtime_dir" "$manager"
}

# append_provider_creds <env-block> — append any supplied provider creds to the
# canonical env block so the always-on service boots configured. Ends with printf
# (returns 0) so an absent cred can't trip `set -e` on the command substitution.
append_provider_creds() {
    local block="$1"
    [ -n "$OPENAI_API_KEY" ]    && block="$block
OPENAI_API_KEY=$OPENAI_API_KEY"
    [ -n "$VERTEX_PROJECT_ID" ] && block="$block
VERTEX_PROJECT_ID=$VERTEX_PROJECT_ID"
    [ -n "$VERTEX_REGION" ]     && block="$block
VERTEX_REGION=$VERTEX_REGION"
    printf '%s' "$block"
}

# ── opt-in TLS (--tls-cert/--tls-key) ────────────────────────────────────────
# The packaged gateway serves plain http by default, which is a secure context
# ONLY on localhost — a phone/laptop reaching the instance over the network gets
# no service worker, no PWA install, and no web push. Supplying a cert + key
# (e.g. from `tailscale cert`, mkcert, or a real CA) makes the gateway serve
# https so remote devices get the full experience. Remote reachability itself is
# separate: the gateway binds loopback-only until changed via --bind (which
# writes the machine-global network.toml) or Settings → Access → Network access.

# tls_enabled — true iff both TLS paths were supplied (validated below).
tls_enabled() { [ -n "$LUCIDOS_TLS_CERT" ] && [ -n "$LUCIDOS_TLS_KEY" ]; }

# validate_tls_config — both-or-neither, and both files must exist AND be
# readable. Fail closed: a half-configured TLS gateway would refuse every
# connection far less legibly.
validate_tls_config() {
    [ -n "$LUCIDOS_TLS_CERT$LUCIDOS_TLS_KEY" ] || return 0
    tls_enabled || die "--tls-cert and --tls-key must be supplied together (got only one)."
    [ -f "$LUCIDOS_TLS_CERT" ] || die "--tls-cert: file not found: $LUCIDOS_TLS_CERT"
    [ -f "$LUCIDOS_TLS_KEY" ]  || die "--tls-key: file not found: $LUCIDOS_TLS_KEY"
    [ -r "$LUCIDOS_TLS_CERT" ] || die "--tls-cert: file not readable: $LUCIDOS_TLS_CERT"
    [ -r "$LUCIDOS_TLS_KEY" ]  || die "--tls-key: file not readable: $LUCIDOS_TLS_KEY"
}

# install_url_scheme — https when TLS is configured, else http. For the health
# probe + every printed URL.
install_url_scheme() { if tls_enabled; then printf 'https'; else printf 'http'; fi; }

# append_tls_env <env-block> — append the TLS pairs when configured (mirrors
# append_provider_creds; ends with printf so `set -e` is safe). Like the
# provider creds, TLS is re-baked from this run's flags: re-run WITH the flags
# when re-registering, or the service reverts to plain http.
append_tls_env() {
    local block="$1"
    if tls_enabled; then
        block="$block
$(service_tls_env_pairs "$LUCIDOS_TLS_CERT" "$LUCIDOS_TLS_KEY")"
    fi
    printf '%s' "$block"
}

# ── macOS Command Line Tools preflight (download / from-tarball paths) ───────
# Chat works without CLT, but coding agents / Apply / run_python shell out to
# git and python3, whose /usr/bin shims error until CLT is installed. Warn —
# never die — so a fresh Mac gets the hint at install time instead of a cryptic
# failure on first use. (--dev has its own toolchain bootstrap and never runs
# this.)
macos_clt_preflight() {
    [ "$(uname -s)" = "Darwin" ] || return 0
    xcode-select -p >/dev/null 2>&1 && return 0
    warn "Xcode Command Line Tools not detected — chat will work, but coding agents,"
    warn "git operations, and Python scripts will fail until they are installed."
    info "Install them with:  xcode-select --install"
}

# register_launchd <gateway-bin> <data> <env-block> <mode> <slug> — write the
# LaunchAgent plist and (re)load it idempotently.
register_launchd() {
    local gateway_bin="$1" data="$2" env_block="$3" mode="$4" slug="$5"
    local label plist out_log err_log content uid
    label="$(service_launchd_label "$slug")"
    plist="$(service_launchd_plist_path "$HOME" "$slug")"
    out_log="$(service_log_dir "$data")/gateway.out.log"
    err_log="$(service_log_dir "$data")/gateway.err.log"
    content="$(service_launchd_plist "$label" "$gateway_bin" "$data" "$out_log" "$err_log" "$env_block")"
    service_write_file "$plist" "$content" "$mode" \
        || die "Could not write the LaunchAgent plist at $plist"
    info "plist: $plist"
    uid="$(id -u)"
    # A re-run over a running instance must fully unload the old job first, so
    # this can now fail because the OLD agent would not leave the domain. That is
    # reported rather than papered over: bootstrapping on top of it would leave
    # the previous plist running while claiming the new one had loaded.
    service_launchd_load "$uid" "$plist" "$label" \
        || die "launchctl could not load the service ($label)${SERVICE_LAUNCHD_ERR:+: $SERVICE_LAUNCHD_ERR}.
       Try: launchctl bootout gui/$uid/$label ; launchctl bootstrap gui/$uid \"$plist\""
    ok "LaunchAgent $label loaded (RunAtLoad + KeepAlive)"
}

# register_systemd <gateway-bin> <data> <env-block> <mode> <slug> — write the
# systemd --user unit, enable + start it, and enable user lingering (best-effort)
# so it survives logout/reboot.
register_systemd() {
    local gateway_bin="$1" data="$2" env_block="$3" mode="$4" slug="$5"
    local unit_name unit_path content
    unit_name="$(service_systemd_unit_name "$slug")"
    unit_path="$(service_systemd_unit_path "$HOME" "$slug" "${XDG_CONFIG_HOME:-}")"
    content="$(service_systemd_unit "$gateway_bin" "$data" "$env_block")"
    service_write_file "$unit_path" "$content" "$mode" \
        || die "Could not write the systemd unit at $unit_path"
    info "unit: $unit_path"
    service_systemd_load "$unit_path" "$unit_name" \
        || die "systemctl --user could not start $unit_name. Check: systemctl --user status $unit_name"
    ok "systemd unit $unit_name enabled + started"
    # Survive logout/reboot. May require privileges; announce + best-effort, never
    # fail the install over it.
    if loginctl enable-linger "$USER" >/dev/null 2>&1; then
        ok "Enabled user lingering (the service runs across logout/reboot)"
    else
        warn "Could not enable user lingering — the service may stop on logout."
        info "To make it survive logout/reboot, run:  sudo loginctl enable-linger $USER"
    fi
}

# service_log_hint <manager> <data> <slug> — where to look when a service won't
# come up.
service_log_hint() {
    case "$1" in
        launchd)      printf '         %s/gateway.err.log' "$(service_log_dir "$2")" ;;
        systemd-user) printf '         journalctl --user -u %s -e' "$(service_systemd_unit_name "$3")" ;;
    esac
}

print_running() {
    local runtime_dir="$1" url
    url="$(install_url_scheme)://localhost:$LUCIDOS_PORT"
    printf '\n%s========================================%s\n' "$C_GREEN" "$C_RESET"
    printf '%s  Starting Lucidos 🚀%s\n' "$C_BOLD" "$C_RESET"
    printf '========================================\n'
    printf '  %sOpen:%s     %s%s/%s\n' "$C_BOLD" "$C_RESET" "$C_BOLD$C_BLUE" "$url" "$C_RESET"
    printf '  Instance: %s  (port %s)\n' "$LUCIDOS_INSTANCE" "$LUCIDOS_PORT"
    printf '  Runtime:  %s\n' "$runtime_dir"
    printf '  Data:     %s\n' "$LUCIDOS_GATEWAY_DATA"
    print_remote_access_hints
    printf '\n'
    printf '  The gateway runs in the FOREGROUND below — press Ctrl-C to stop.\n'
    printf '  To run it as an always-on background service instead, re-run without\n'
    printf '  --no-service (needs launchd on macOS or systemd --user on Linux).\n'
    printf '%s========================================%s\n\n' "$C_GREEN" "$C_RESET"
}

print_service_running() {
    local runtime_dir="$1" manager="$2" slug="$LUCIDOS_INSTANCE" url
    url="$(install_url_scheme)://localhost:$LUCIDOS_PORT"
    printf '\n%s========================================%s\n' "$C_GREEN" "$C_RESET"
    printf '%s  Lucidos is running as a service 🚀%s\n' "$C_BOLD" "$C_RESET"
    printf '========================================\n'
    printf '  %sOpen:%s     %s%s/%s\n' "$C_BOLD" "$C_RESET" "$C_BOLD$C_BLUE" "$url" "$C_RESET"
    printf '  Instance: %s%s%s  (port %s)\n' "$C_BOLD" "$slug" "$C_RESET" "$LUCIDOS_PORT"
    printf '  Runtime:  %s  (shared)\n' "$runtime_dir"
    printf '  Data:     %s\n' "$LUCIDOS_GATEWAY_DATA"
    case "$manager" in
        launchd)
            printf '  Service:  launchd agent %s%s%s\n' "$C_BOLD" "$(service_launchd_label "$slug")" "$C_RESET"
            printf '            %s\n' "$(service_launchd_plist_path "$HOME" "$slug")"
            printf '  Logs:     %s/gateway.{out,err}.log\n' "$(service_log_dir "$LUCIDOS_GATEWAY_DATA")"
            ;;
        systemd-user)
            printf '  Service:  systemd --user unit %s%s%s\n' "$C_BOLD" "$(service_systemd_unit_name "$slug")" "$C_RESET"
            printf '            %s\n' "$(service_systemd_unit_path "$HOME" "$slug" "${XDG_CONFIG_HOME:-}")"
            printf '  Logs:     journalctl --user -u %s -f\n' "$(service_systemd_unit_name "$slug")"
            ;;
    esac
    print_remote_access_hints
    printf '\n'
    printf '  It runs in the background now and starts at login.\n'
    printf '  Stop + remove it:  %s\n' "$(uninstall_hint "$slug")"
    printf '                     (add --purge to also delete data; it prompts for nothing)\n'
    printf '  Change its port:   re-run with --name %s --port <new-port>\n' "$slug"
    printf '%s========================================%s\n\n' "$C_GREEN" "$C_RESET"
}

print_installed() {
    local runtime_dir="$1"
    printf '\n%s========================================%s\n' "$C_GREEN" "$C_RESET"
    printf '%s  Lucidos installed ✓%s\n' "$C_BOLD" "$C_RESET"
    printf '========================================\n'
    printf '  Instance: %s\n' "$LUCIDOS_INSTANCE"
    printf '  Runtime:  %s  (shared)\n' "$runtime_dir"
    printf '  Data:     %s\n' "$LUCIDOS_GATEWAY_DATA"
    printf '\n'
    printf '  Re-run the installer without --no-launch to start it — by default it\n'
    printf '  registers an always-on user service (launchd / systemd --user); add\n'
    printf '  --no-service to run it in the foreground instead. Or launch directly:\n'
    printf '    LUCIDOS_API_PORT=%s LUCIDOS_GATEWAY_DATA=%s \\\n' "$LUCIDOS_PORT" "$LUCIDOS_GATEWAY_DATA"
    printf '    LUCIDOS_GATEWAY_PG_BACKEND=embedded \\\n'
    printf '    LUCIDOS_PG_BIN_DIR=%s/postgres/bin LUCIDOS_PG_LIB_DIR=%s/postgres/lib \\\n' "$runtime_dir" "$runtime_dir"
    printf '    LUCIDOS_ENGINE_BIN=%s/lucidos-engine \\\n' "$runtime_dir"
    printf '    LUCIDOS_STATIC_DIR=%s/frontend LUCIDOS_SDK_DIR=%s/sdk \\\n' "$runtime_dir" "$runtime_dir"
    printf '    %s/lucidos-gateway\n' "$runtime_dir"
    printf '  Then open: %s%s://localhost:%s/%s\n' "$C_BLUE" "$(install_url_scheme)" "$LUCIDOS_PORT" "$C_RESET"
    printf '%s========================================%s\n\n' "$C_GREEN" "$C_RESET"
}

# uninstall_hint <slug>: the command that actually removes <slug> ON THIS RUN.
# `./uninstall.sh` is only real when install.sh ran from a checkout. The default
# audience pipes the installer (`curl … | sh`), and the runtime tarball lays down
# no uninstaller, so printing the `./` form to them names a file they do not have
# anywhere on disk. Branch on the same self-dir probe dispatch_uninstall uses.
uninstall_hint() {
    local slug="$1" self_dir
    self_dir="$(installer_self_dir)"
    if [ -n "$self_dir" ] && [ -f "$self_dir/uninstall.sh" ]; then
        printf './uninstall.sh --name %s' "$slug"
    else
        printf 'uninstall.sh --name %s  (download it from %s)' \
            "$slug" "${LUCIDOS_REPO_URL%.git}"
    fi
}

# ── uninstall / list dispatch (--uninstall / --list) ─────────────────────────
# install.sh --uninstall / --list are thin front doors for uninstall.sh (so the
# uninstall + listing orchestration lives in ONE place). They exec the sibling
# uninstall.sh from a checkout, else fetch it from the same ref as the installer.
# The instance is forwarded as --name; --port is forwarded only when pinned (so a
# bare --uninstall lets uninstall.sh resolve the sole/selected instance).
dispatch_uninstall() {
    local self_dir url payload
    local -a fwd=(--prefix "$LUCIDOS_PREFIX")
    # Uninstall is slug-keyed (the port is not the identity), so --port is not
    # forwarded — instances are addressed by --name / --all.
    [ -n "$LUCIDOS_LIST" ]     && fwd+=(--list)
    [ -n "$LUCIDOS_ALL" ]      && fwd+=(--all)
    [ -n "$LUCIDOS_INSTANCE_EXPLICIT" ] && fwd+=(--name "$LUCIDOS_INSTANCE")
    [ -n "$LUCIDOS_PURGE" ]    && fwd+=(--purge)
    self_dir="$(installer_self_dir)"
    if [ -n "$self_dir" ] && [ -f "$self_dir/uninstall.sh" ]; then
        exec bash "$self_dir/uninstall.sh" "${fwd[@]}"
    fi
    url="${LUCIDOS_UNINSTALL_URL:-${LUCIDOS_INSTALL_URL%/install.sh}/uninstall.sh}"
    step "Fetching the uninstaller"
    info "$url"
    payload="$(curl -fsSL "$url")" || die "Could not fetch the uninstaller from $url"
    [ -n "$payload" ] || die "Fetched uninstaller from $url is empty."
    # Shebang sniff before `exec bash -c`, for the same reason _source_libs
    # sniffs a fetched helper lib: `curl -fsSL` plus a non-empty test cannot see
    # a SOFT-404, and this line hands unknown remote content straight to bash.
    # It is not hypothetical here: as of the 2026-07-30 docs audit the published
    # front door serves install.sh and scripts/lib/*.sh but NOT uninstall.sh, so
    # <origin>/uninstall.sh returns the landing page at status 200 and a piped
    # `install.sh --uninstall` executed that HTML. Fail loud and name the
    # fallback instead. Drop this only if you also want a hijacked or
    # misconfigured origin to reach `exec`.
    case "$payload" in
        '#!'*) : ;;
        *) die "$url did not return a shell script.
       The origin likely served its 404/SPA fallback page with a 200 status.
       Run the uninstaller directly from a checkout of the repo instead:
         ./uninstall.sh ${fwd[*]}" ;;
    esac
    exec bash -c "$payload" bash "${fwd[@]}"
}

# ════════════════════════════════════════════════════════════════════════════
# ── SOURCE BUILD (--dev / --source) — preserved legacy behavior ─────────────
# ════════════════════════════════════════════════════════════════════════════
# This branch reproduces the original installer exactly: it bootstraps the
# toolchain (Rust, Node, Docker, cmake), clones/updates the repo, configures the
# provider into <workspace>/data/.env, and builds + launches from source via
# scripts/run.sh. It is for contributors / from-source installs.

# ── platform detection (source build) ───────────────────────────────────────
OS=""; ARCH=""
detect_platform() {
    local uname_s uname_m
    uname_s="$(uname -s)"
    uname_m="$(uname -m)"
    case "$uname_s" in
        Darwin) OS="macos" ;;
        Linux)  OS="linux" ;;
        *) die "Unsupported OS: $uname_s. Lucidos installs on macOS and Linux." ;;
    esac
    case "$uname_m" in
        x86_64|amd64) ARCH="x86_64" ;;
        arm64|aarch64) ARCH="arm64" ;;
        *) die "Unsupported architecture: $uname_m (need x86_64 or arm64)." ;;
    esac
    step "Detected $OS / $ARCH"
}

# ── linux package-manager abstraction ───────────────────────────────────────
PKG=""           # apt|dnf|yum|pacman|zypper
SUDO=""          # "sudo" when not root and sudo exists
detect_pkg_mgr() {
    if [ "$(id -u)" -ne 0 ]; then
        if have sudo; then SUDO="sudo"; else SUDO=""; fi
    fi
    for c in apt-get dnf yum pacman zypper; do
        if have "$c"; then PKG="$c"; break; fi
    done
}

pkg_install() {
    # Best-effort install of one or more distro packages. Args = package names.
    [ -n "$PKG" ] || { warn "No supported package manager found; install manually: $*"; return 1; }
    case "$PKG" in
        apt-get) $SUDO apt-get update -y && $SUDO apt-get install -y "$@" ;;
        dnf)     $SUDO dnf install -y "$@" ;;
        yum)     $SUDO yum install -y "$@" ;;
        pacman)  $SUDO pacman -Sy --noconfirm "$@" ;;
        zypper)  $SUDO zypper install -y "$@" ;;
    esac
}

# ── base tools (curl, git) ──────────────────────────────────────────────────
ensure_base_tools() {
    step "Checking base tools (curl, git)"
    if ! have curl; then
        if [ "$OS" = "linux" ]; then pkg_install curl || true; fi
        have curl || die "curl is required but could not be installed. Install curl and re-run."
    fi
    if ! have git; then
        if [ "$OS" = "macos" ]; then
            # Triggers the Xcode Command Line Tools install (provides git + a C toolchain).
            warn "git not found — triggering Xcode Command Line Tools install (a GUI dialog may appear)."
            xcode-select --install 2>/dev/null || true
            die "Re-run this installer once the Command Line Tools finish installing."
        else
            pkg_install git || true
        fi
        have git || die "git is required but could not be installed. Install git and re-run."
    fi
    ok "curl and git present"
}

# ── homebrew (macOS) ────────────────────────────────────────────────────────
ensure_brew() {
    if have brew; then return 0; fi
    step "Installing Homebrew (package manager for macOS)"
    NONINTERACTIVE=1 /bin/bash -c \
        "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    # Put brew on PATH for the rest of this run (Apple Silicon vs Intel prefix).
    if [ -x /opt/homebrew/bin/brew ]; then eval "$(/opt/homebrew/bin/brew shellenv)"; fi
    if [ -x /usr/local/bin/brew ]; then eval "$(/usr/local/bin/brew shellenv)"; fi
    have brew || die "Homebrew installed but 'brew' is not on PATH. Open a new shell and re-run."
}

brew_install() {
    # Install a formula if its command isn't already present. $1=cmd $2=formula
    local cmd="$1" formula="${2:-$1}"
    if have "$cmd"; then return 0; fi
    info "brew install $formula"
    brew install "$formula"
}

# ── rust toolchain ──────────────────────────────────────────────────────────
ensure_rust() {
    if have cargo; then ok "Rust toolchain present ($(cargo --version 2>/dev/null || echo cargo))"; return 0; fi
    step "Installing Rust toolchain (rustup)"
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    have cargo || die "Rust installed but 'cargo' is not on PATH. Open a new shell and re-run."
    ok "Rust toolchain installed"
}

# ── node ────────────────────────────────────────────────────────────────────
ensure_node() {
    if have node && have npm; then ok "Node.js present ($(node --version 2>/dev/null))"; return 0; fi
    step "Installing Node.js"
    if [ "$OS" = "macos" ]; then
        brew_install node
    else
        pkg_install nodejs npm || pkg_install nodejs || true
    fi
    have node || die "Node.js is required but could not be installed automatically. Install Node 18+ and re-run."
    # Warn (don't fail) on an old major — the frontend build wants a modern Node.
    local major
    major="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
    if [ "$major" -lt 18 ] 2>/dev/null; then
        warn "Node $(node --version) is older than v18; the frontend build may fail. Consider nvm or NodeSource for a newer Node."
    fi
    ok "Node.js installed"
}

# ── build deps (cmake; sccache when easy) ───────────────────────────────────
ensure_build_deps() {
    step "Checking native build dependencies"
    if [ "$OS" = "macos" ]; then
        brew_install cmake
        brew_install sccache || true   # speeds up rebuilds; optional (see below)
        have jq || brew_install jq || true
    else
        # A C/C++ toolchain + cmake + openssl headers are needed to build the
        # engine's native crates.
        case "$PKG" in
            apt-get) pkg_install build-essential cmake pkg-config libssl-dev jq || true ;;
            dnf|yum) pkg_install gcc gcc-c++ make cmake pkgconfig openssl-devel jq || true ;;
            pacman)  pkg_install base-devel cmake pkgconf openssl jq || true ;;
            zypper)  pkg_install -t pattern devel_basis 2>/dev/null || true; pkg_install cmake pkg-config libopenssl-devel jq || true ;;
            *) warn "Install a C toolchain, cmake, pkg-config and OpenSSL dev headers manually." ;;
        esac
        # sccache rarely ships in distro repos and building it from source is
        # slow; skip it on Linux. The build falls back to an uncached compile
        # (see ensure_sccache_or_disable).
    fi
    have cmake || die "cmake is required to build the engine but is not installed. Install cmake and re-run."
    ok "Native build dependencies present"
}

# .cargo/config.toml sets `rustc-wrapper = "sccache"`. If sccache is not on
# PATH, cargo would fail with `process didn't exit successfully: sccache`.
# Export RUSTC_WRAPPER="" to override the config and do a plain (uncached)
# build. When sccache IS present we leave it alone so rebuilds stay cached.
ensure_sccache_or_disable() {
    if have sccache; then
        ok "sccache present — Rust builds will be cached"
    else
        export RUSTC_WRAPPER=""
        info "sccache not found — building without the compile cache (RUSTC_WRAPPER disabled)."
    fi
}

# ── docker ──────────────────────────────────────────────────────────────────
ensure_docker_installed() {
    if have docker; then return 0; fi
    step "Installing Docker (runs PostgreSQL + pgvector)"
    if [ "$OS" = "macos" ]; then
        brew install --cask docker || die "Could not install Docker Desktop via Homebrew. Install it from https://www.docker.com/products/docker-desktop and re-run."
        info "Launching Docker Desktop…"
        open -a Docker || true
    else
        # Docker's official convenience script supports the major distros.
        if [ -n "$PKG" ]; then
            info "Installing Docker Engine via get.docker.com (requires sudo)…"
            curl -fsSL https://get.docker.com | $SUDO sh || \
                die "Docker install failed. Install Docker Engine manually: https://docs.docker.com/engine/install/ then re-run."
        else
            die "Docker is required but no package manager was found. Install Docker manually: https://docs.docker.com/engine/install/"
        fi
    fi
    have docker || die "Docker installed but 'docker' is not on PATH. Open a new shell (or restart) and re-run."
}

ensure_docker_running() {
    step "Verifying the Docker daemon is running"
    if docker info >/dev/null 2>&1; then ok "Docker daemon is running"; return 0; fi

    if [ "$OS" = "macos" ]; then
        info "Waiting for Docker Desktop to start (up to 120s)…"
        open -a Docker >/dev/null 2>&1 || true
        for _ in $(seq 1 120); do
            if docker info >/dev/null 2>&1; then ok "Docker daemon is running"; return 0; fi
            printf '.'; sleep 1
        done
        printf '\n'
        die "Docker Desktop did not become ready. Open Docker Desktop, finish first-run setup (accept terms), wait for the whale icon to settle, then re-run this installer."
    fi

    # Linux: try to start the service.
    if have systemctl; then
        info "Starting the docker service (requires sudo)…"
        $SUDO systemctl enable --now docker >/dev/null 2>&1 || true
    fi
    if docker info >/dev/null 2>&1; then ok "Docker daemon is running"; return 0; fi

    # Still failing — most often a permissions issue (user not in the docker group).
    if $SUDO docker info >/dev/null 2>&1; then
        warn "Docker works with sudo but not as your user — you are not in the 'docker' group."
        info "Adding you to the docker group (requires sudo)…"
        $SUDO usermod -aG docker "$USER" 2>/dev/null || true
        die "Log out and back in (or run: newgrp docker) so the group change takes effect, then re-run this installer. Lucidos must use Docker without sudo."
    fi
    die "The Docker daemon is not running. Start it (e.g. 'sudo systemctl start docker') and re-run."
}

# ── repo ────────────────────────────────────────────────────────────────────
fetch_repo() {
    # If the installer is being run from inside an existing checkout, use it.
    local self_dir=""
    if [ -n "${BASH_SOURCE:-}" ] && [ -f "${BASH_SOURCE[0]:-}" ]; then
        self_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    fi
    if [ -n "$self_dir" ] && [ -f "$self_dir/scripts/web-dev.sh" ] && [ -d "$self_dir/.git" ]; then
        LUCIDOS_HOME="$self_dir"
        step "Using existing checkout at $LUCIDOS_HOME"
        return 0
    fi

    if [ -d "$LUCIDOS_HOME/.git" ]; then
        step "Updating existing checkout at $LUCIDOS_HOME"
        git -C "$LUCIDOS_HOME" fetch --tags --prune origin || warn "git fetch failed; using the existing checkout as-is."
        if [ -n "$LUCIDOS_REF" ]; then
            git -C "$LUCIDOS_HOME" checkout "$LUCIDOS_REF" || die "Could not checkout ref '$LUCIDOS_REF'."
            git -C "$LUCIDOS_HOME" pull --ff-only 2>/dev/null || true
        else
            git -C "$LUCIDOS_HOME" pull --ff-only 2>/dev/null || warn "Could not fast-forward; using the existing checkout as-is."
        fi
    else
        if [ -e "$LUCIDOS_HOME" ] && [ -n "$(ls -A "$LUCIDOS_HOME" 2>/dev/null || true)" ]; then
            die "$LUCIDOS_HOME exists and is not a Lucidos checkout. Set LUCIDOS_HOME to an empty/new path and re-run."
        fi
        step "Cloning Lucidos into $LUCIDOS_HOME"
        git clone "$LUCIDOS_REPO_URL" "$LUCIDOS_HOME" || die "git clone failed from $LUCIDOS_REPO_URL"
        if [ -n "$LUCIDOS_REF" ]; then
            git -C "$LUCIDOS_HOME" checkout "$LUCIDOS_REF" || die "Could not checkout ref '$LUCIDOS_REF'."
        fi
    fi
    ok "Repository ready at $LUCIDOS_HOME"
}

# ── provider configuration (source build) ───────────────────────────────────
PROVIDER_MODE="mock"   # mock|openai|vertex|existing
configure_provider() {
    step "Configuring the LLM provider"
    mkdir -p "$LUCIDOS_WORKSPACE/data"
    local env_file="$LUCIDOS_WORKSPACE/data/.env"

    local block=""
    if [ -n "$OPENAI_API_KEY" ]; then
        PROVIDER_MODE="openai"
        block="OPENAI_API_KEY=$OPENAI_API_KEY"
        ok "Using OpenAI (OPENAI_API_KEY supplied) — persisted to data/.env"
    elif [ -n "$VERTEX_PROJECT_ID" ]; then
        PROVIDER_MODE="vertex"
        block="VERTEX_PROJECT_ID=$VERTEX_PROJECT_ID"
        [ -n "$VERTEX_REGION" ] && block="$block
VERTEX_REGION=$VERTEX_REGION"
        ok "Using Vertex AI (VERTEX_PROJECT_ID=$VERTEX_PROJECT_ID) — persisted to data/.env"
        info "Vertex also needs Google credentials (e.g. 'gcloud auth application-default login')."
    elif [ -f "$env_file" ] && grep -qE '^[[:space:]]*(OPENAI_API_KEY|VERTEX_PROJECT_ID|ANTHROPIC_API_KEY)=' "$env_file"; then
        # No creds supplied this run, but data/.env already configures a provider.
        # Leave it untouched — a re-run must never de-configure the workspace.
        PROVIDER_MODE="existing"
        ok "Found an existing provider in data/.env — keeping it."
    else
        PROVIDER_MODE="mock"
        warn "No provider credentials supplied — booting in 'mock' mode so the UI comes up."
        info "Chat will return stub responses until you configure a provider in Settings → Providers,"
        info "then restart:  cd $LUCIDOS_HOME && ./scripts/stop.sh -w $LUCIDOS_WORKSPACE && ./scripts/run.sh -w $LUCIDOS_WORKSPACE"
    fi

    # Only rewrite data/.env when we have a fresh managed block to install. Strip
    # any prior managed block first so re-runs don't duplicate it. When no creds
    # are supplied we never touch the file, so an existing config survives.
    if [ -n "$block" ]; then
        if [ -f "$env_file" ]; then
            awk '
                /^# >>> lucidos install\.sh \(managed\) >>>$/ { skip=1; next }
                /^# <<< lucidos install\.sh \(managed\) <<<$/ { skip=0; next }
                !skip { print }
            ' "$env_file" > "$env_file.tmp" && mv "$env_file.tmp" "$env_file"
        fi
        {
            printf '# >>> lucidos install.sh (managed) >>>\n'
            printf '%s\n' "$block"
            printf '# <<< lucidos install.sh (managed) <<<\n'
        } >> "$env_file"
    fi

    # data/.env holds secrets — keep it private whenever it exists (covers the
    # re-run-in-mock-mode case where we wrote no block this time).
    if [ -f "$env_file" ]; then chmod 600 "$env_file" 2>/dev/null || true; fi
}

# ── launch (source build) ───────────────────────────────────────────────────
launch() {
    step "Starting Lucidos (PostgreSQL + engine + frontend)"
    printf '\n%s%sFirst run builds the engine from source (a release build by default) — this can\ntake 10–20+ minutes on a clean machine. Subsequent runs reuse the build and\nstart in seconds. Set LUCIDOS_DEBUG_BUILD=1 for a faster (debug) build.%s\n\n' \
        "$C_DIM" "$C_BOLD" "$C_RESET"

    # scripts/run.sh is the user-facing launcher: it brings up the same stack as
    # the developer script (scripts/web-dev.sh) but defaults to a release engine
    # build and a one-shot frontend build (no rebuild-on-change watcher left
    # running). LUCIDOS_DEBUG_BUILD (set in the environment) flows through to it
    # to opt into a faster debug build instead.
    #
    # In mock mode, pass LUCIDOS_MODEL=mock for THIS launch only (not persisted),
    # so the stack comes up without credentials. Once a real provider is
    # configured a plain restart picks it up.
    cd "$LUCIDOS_HOME"
    if [ "$PROVIDER_MODE" = "mock" ]; then
        LUCIDOS_MODEL=mock ./scripts/run.sh -w "$LUCIDOS_WORKSPACE" \
            || die "scripts/run.sh failed. Check the output above and the engine log under $LUCIDOS_WORKSPACE/.lucidos/engine.log"
    else
        ./scripts/run.sh -w "$LUCIDOS_WORKSPACE" \
            || die "scripts/run.sh failed. Check the output above and the engine log under $LUCIDOS_WORKSPACE/.lucidos/engine.log"
    fi
}

# ── final banner (source build) ─────────────────────────────────────────────
print_done() {
    # Resolve the user-facing URL from the workspace ports file the dev script wrote.
    local ports_file="$LUCIDOS_WORKSPACE/.lucidos/ports"
    local url="http://localhost:5173" proto="http" port="5173"
    if [ -f "$ports_file" ]; then
        # shellcheck disable=SC1090
        port="$(awk -F= '/^VITE_PORT=/{print $2}' "$ports_file" | tail -1)"
        proto="$(awk -F= '/^PROTO=/{print $2}' "$ports_file" | tail -1)"
        [ -n "$proto" ] || proto="http"
        [ -n "$port" ] || port="5173"
        url="$proto://localhost:$port"
    fi

    printf '\n%s========================================%s\n' "$C_GREEN" "$C_RESET"
    printf '%s  Lucidos is running 🎉%s\n' "$C_BOLD" "$C_RESET"
    printf '========================================\n'
    printf '  %sOpen:%s       %s%s%s\n' "$C_BOLD" "$C_RESET" "$C_BOLD$C_BLUE" "$url" "$C_RESET"
    printf '  Workspace:  %s\n' "$LUCIDOS_WORKSPACE"
    printf '  Repo:       %s\n' "$LUCIDOS_HOME"
    if [ "$PROVIDER_MODE" = "mock" ]; then
        printf '  Provider:   %smock%s — configure a real one in Settings → Providers, then restart\n' "$C_YELLOW" "$C_RESET"
    elif [ "$PROVIDER_MODE" = "existing" ]; then
        printf '  Provider:   configured (data/.env)\n'
    else
        printf '  Provider:   %s\n' "$PROVIDER_MODE"
    fi
    printf '\n'
    printf '  Stop:    cd %s && ./scripts/stop.sh -w %s\n' "$LUCIDOS_HOME" "$LUCIDOS_WORKSPACE"
    printf '  Restart: cd %s && ./scripts/run.sh -w %s\n' "$LUCIDOS_HOME" "$LUCIDOS_WORKSPACE"
    printf '  Logs:    tail -f %s/.lucidos/engine.log\n' "$LUCIDOS_WORKSPACE"
    printf '%s========================================%s\n\n' "$C_GREEN" "$C_RESET"
}

run_dev_install() {
    step "Source build (--dev): bootstrapping toolchain and building from source"
    detect_platform
    [ "$OS" = "linux" ] && detect_pkg_mgr

    if [ -n "$LUCIDOS_SKIP_DEPS" ]; then
        warn "LUCIDOS_SKIP_DEPS set — skipping dependency bootstrap (assuming Rust, Node, Docker, cmake are present)."
        for t in git cargo node npm docker cmake; do
            have "$t" || die "Required tool '$t' is missing and LUCIDOS_SKIP_DEPS is set. Install it or unset LUCIDOS_SKIP_DEPS."
        done
    else
        # macOS: install Homebrew first — its Command Line Tools provide git and
        # a C compiler that the git/build checks below depend on.
        [ "$OS" = "macos" ] && ensure_brew
        ensure_base_tools
        ensure_rust
        ensure_node
        ensure_build_deps
        ensure_docker_installed
    fi

    ensure_sccache_or_disable
    ensure_docker_running
    fetch_repo
    configure_provider
    launch
    print_done
}

# ── help ────────────────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Lucidos installer

Usage:
  curl -fsSL <url>/install.sh | sh                 # download + run as a service (default)
  ./install.sh                                     # download + run (from a checkout)
  ./install.sh --no-service                        # download + run in the foreground
  ./install.sh --dev                               # build from source instead
  ./install.sh --from-tarball <path>               # install a local tarball
  ./install.sh --uninstall [--purge]               # stop + remove the service

Modes:
  (default)             download the prebuilt headless tarball for your platform,
                        verify its sha256, extract it, and register the bundled
                        gateway as an always-on USER service (launchd LaunchAgent
                        on macOS, systemd --user unit on Linux) so it survives
                        terminal-close + reboot and restarts on failure.
  --no-service          after extract, run the gateway in the FOREGROUND instead
                        of registering a service (Ctrl-C to stop). Also the
                        automatic fallback when no user service manager is present.
  --dev, --source       build from source: bootstrap toolchain (Rust/Node/Docker/
                        cmake), clone the repo, compile, and launch via run.sh.
                        ALWAYS foreground — --dev never registers a service.
  --from-tarball PATH   install a LOCAL tarball (offline; e.g. one produced by
                        scripts/build-headless.sh); registers the service too
                        (unless --no-service).
  --uninstall           stop + unregister an instance and remove its plist/unit
                        (delegates to uninstall.sh). Keeps your data unless --purge.
  --list                list installed instances + their ports (delegates to
                        uninstall.sh).

Multiple gateways (instances): each --name <slug> is an independent gateway that
shares the one downloaded runtime but has its own data dir (<prefix>/<slug>/),
service id (com.lucidos.gateway.<slug>), and port. The PORT is a mutable property
— re-run with --name <slug> --port <new> to move an instance to a new port. So a
terminal install coexists with a dev gateway and the packaged .app.

Flags:
  --name SLUG           instance name (default: 'default'). Lowercase letters/
                        digits/dashes; 'gateway'/'runtime'/'current'/'logs' are reserved.
  --version V           version to download (default: RELEASE when run from a
                        checkout, else $LUCIDOS_DEFAULT_VERSION)
  --base-url URL        base URL holding lucidos-<version>-<triple>.tar.gz + .sha256
                        (default: the GitHub Releases path for v<version>)
  --prefix DIR          install prefix (default: \$HOME/.lucidos). The SHARED runtime
                        extracts to <prefix>/runtime/lucidos-<version>-<triple>/.
  --port P              the instance's gateway port (default: $LUCIDOS_PORT; auto-picks a
                        free port for a NEW instance if it's taken). Pinning it sets/
                        changes this instance's port.
  --bind B              gateway network bind: all | loopback | <IP>. Written to the
                        machine-global ~/.lucidos/network.toml (the same knob the
                        picker's Settings → Network access edits). Default: loopback
                        only — use an SSH tunnel or tailscale serve for remote access.
  --tls-cert PATH       serve https instead of http (requires --tls-key). A remote
                        device (phone/laptop) only gets a secure context — service
                        worker, PWA install, web push notifications — over https;
                        plain http limits those to localhost. Works with certs from
                        'tailscale cert', mkcert, or a real CA. Remote reachability
                        itself is separate: --bind (or Settings → Access → Network
                        access; the gateway binds loopback-only by default). Like
                        provider creds, TLS is baked from THIS run's flags — re-run
                        with them when re-registering, or the service reverts to http.
  --tls-key PATH        the private key for --tls-cert (both-or-neither).
  --force               re-download / re-extract the runtime even if already installed
  --no-launch           install but do not start (or register) the gateway
  --no-service          run in the foreground instead of registering a service
  --uninstall           stop + remove an instance (see Modes); with --all, every instance
  --list                list installed instances + ports
  --all                 with --uninstall: act on every instance
  --purge               with --uninstall: also delete the instance data (with --all,
                        also the shared runtime). IRREVERSIBLE, no confirmation:
                        the data dir holds the embedded PostgreSQL cluster (every
                        thread, message, memory and setting of every workspace)
                        and any picker-created workspace directory under it.
                        A bare --uninstall is not a dry run either: it stops the
                        gateway and removes its service, keeping only your data.
                        --list is the one command that changes nothing.
  -h, --help            this help

Environment variables:
  LUCIDOS_VERSION            same as --version
  LUCIDOS_RELEASE_BASE_URL   same as --base-url
  LUCIDOS_PREFIX             same as --prefix
  LUCIDOS_INSTANCE           same as --name
  LUCIDOS_PORT               same as --port
  LUCIDOS_FORCE=1            same as --force
  LUCIDOS_NO_LAUNCH=1        same as --no-launch
  LUCIDOS_NO_SERVICE=1       same as --no-service
  LUCIDOS_FROM_SOURCE=1      same as --dev
  LUCIDOS_GATEWAY_DATA       override the instance data dir (default: <prefix>/<slug>)
  LUCIDOS_BIND               same as --bind
  LUCIDOS_TLS_CERT           same as --tls-cert
  LUCIDOS_TLS_KEY            same as --tls-key
  LUCIDOS_LIB_BASE_URL       base URL for the helper libs when piped (curl|sh)
  OPENAI_API_KEY             provider: GPT via OpenAI (exported / baked into the service)
  VERTEX_PROJECT_ID          provider: Claude/Gemini via Vertex AI
  VERTEX_REGION              optional Vertex region
  --dev-only: LUCIDOS_REPO_URL, LUCIDOS_REF, LUCIDOS_HOME (clone dir),
              LUCIDOS_WORKSPACE, LUCIDOS_DEBUG_BUILD, LUCIDOS_SKIP_DEPS

Service: launchd agent com.lucidos.gateway.<slug> (~/Library/LaunchAgents/) or
systemd --user unit lucidos-gateway-<slug>.service (~/.config/systemd/user/). Logs:
<prefix>/<slug>/logs/ (launchd) or 'journalctl --user -u lucidos-gateway-<slug>'.
Stop + remove with ./uninstall.sh --name <slug> (or ./install.sh --uninstall);
add --purge to also delete data. Both are repo scripts and the runtime lays down
no copy, so a piped install needs uninstall.sh downloaded from the repository.

NOTE: every published GitHub Release carries the per-platform tarballs: CI
attaches them while the release is still a draft, and it is published only once
all four are on it. If a download 404s anyway, use --version <older-version> or
fall back to --dev / --from-tarball <path>.
EOF
}

# ── argument parsing ────────────────────────────────────────────────────────
parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --dev|--source)   LUCIDOS_FROM_SOURCE=1; shift ;;
            --from-tarball)   [ $# -ge 2 ] || die "--from-tarball requires a path argument"; LUCIDOS_FROM_TARBALL="$2"; shift 2 ;;
            --version)        [ $# -ge 2 ] || die "--version requires an argument";  LUCIDOS_VERSION="$2"; shift 2 ;;
            --base-url)       [ $# -ge 2 ] || die "--base-url requires an argument"; LUCIDOS_RELEASE_BASE_URL="$2"; shift 2 ;;
            --prefix)         [ $# -ge 2 ] || die "--prefix requires an argument";   LUCIDOS_PREFIX="$2"; shift 2 ;;
            --name)           [ $# -ge 2 ] || die "--name requires an argument";     LUCIDOS_INSTANCE="$2"; LUCIDOS_INSTANCE_EXPLICIT=1; shift 2 ;;
            --port)           [ $# -ge 2 ] || die "--port requires an argument";     LUCIDOS_PORT="$2"; LUCIDOS_PORT_EXPLICIT=1; shift 2 ;;
            --bind)           [ $# -ge 2 ] || die "--bind requires an argument";     LUCIDOS_BIND="$2"; shift 2 ;;
            --tls-cert)       [ $# -ge 2 ] || die "--tls-cert requires a path argument"; LUCIDOS_TLS_CERT="$2"; shift 2 ;;
            --tls-key)        [ $# -ge 2 ] || die "--tls-key requires a path argument";  LUCIDOS_TLS_KEY="$2"; shift 2 ;;
            --force)          LUCIDOS_FORCE=1; shift ;;
            --no-launch)      LUCIDOS_NO_LAUNCH=1; shift ;;
            --no-service)     LUCIDOS_NO_SERVICE=1; shift ;;
            --uninstall)      LUCIDOS_UNINSTALL=1; shift ;;
            --list)           LUCIDOS_LIST=1; shift ;;
            --all)            LUCIDOS_ALL=1; shift ;;
            --purge)          LUCIDOS_PURGE=1; shift ;;
            -h|--help)        usage; exit 0 ;;
            *) die "unknown argument: $1  (run 'install.sh --help' for usage)" ;;
        esac
    done
    # The instance data dir is resolved later (it depends on the slug); leave it
    # empty here unless the user pinned an override via LUCIDOS_GATEWAY_DATA.
}

# ── main ────────────────────────────────────────────────────────────────────
main() {
    parse_args "$@"

    printf '%s%sLucidos installer%s\n' "$C_BOLD" "$C_BLUE" "$C_RESET"

    if [ -n "$LUCIDOS_LIST" ]; then
        dispatch_uninstall   # execs uninstall.sh --list — does not return
    elif [ -n "$LUCIDOS_UNINSTALL" ]; then
        printf '%smode=uninstall prefix=%s purge=%s%s\n\n' \
            "$C_DIM" "$LUCIDOS_PREFIX" "${LUCIDOS_PURGE:+yes}" "$C_RESET"
        dispatch_uninstall   # execs uninstall.sh — does not return
    elif [ -n "$LUCIDOS_FROM_SOURCE" ]; then
        printf '%smode=source repo=%s ref=%s home=%s workspace=%s%s\n\n' \
            "$C_DIM" "$LUCIDOS_REPO_URL" "${LUCIDOS_REF:-<default>}" "$LUCIDOS_HOME" "$LUCIDOS_WORKSPACE" "$C_RESET"
        run_dev_install
    elif [ -n "$LUCIDOS_FROM_TARBALL" ]; then
        printf '%smode=from-tarball tarball=%s prefix=%s%s\n\n' \
            "$C_DIM" "$LUCIDOS_FROM_TARBALL" "$LUCIDOS_PREFIX" "$C_RESET"
        run_from_tarball_install
    else
        printf '%smode=download prefix=%s%s\n\n' "$C_DIM" "$LUCIDOS_PREFIX" "$C_RESET"
        run_download_install
    fi
}

main "$@"
