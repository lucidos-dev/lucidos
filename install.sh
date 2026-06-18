#!/usr/bin/env bash
#
# install.sh — one-click installer for Lucidos.
#
#   curl -fsSL https://raw.githubusercontent.com/lucidos-dev/lucidos/main/install.sh | sh
#
# Takes a reasonably clean macOS or Linux machine to a running Lucidos:
#   1. bootstraps the toolchain (Rust, Node, Docker, build deps),
#   2. clones (or updates) the repo,
#   3. starts PostgreSQL + the engine + the frontend via scripts/web-dev.sh,
#   4. prints the local URL to open.
#
# It is idempotent and safe to re-run. See the "One-click install" section of
# README.md for the user-facing summary and the supported environment variables.
#
# IMPORTANT — first run compiles from source. Lucidos currently ships no
# prebuilt binaries or container images, so the very first launch builds the
# Rust engine from source. On a clean machine that build alone takes well past
# five minutes (typically 10–20+ on a laptop). Subsequent runs reuse the build
# and are fast. This is called out honestly rather than hidden.
#
# ── bash re-exec guard ──────────────────────────────────────────────────────
# The installer uses bashisms (`set -o pipefail`, arrays). When started by a
# non-bash POSIX shell — e.g. `curl … | sh` on a Debian host whose /bin/sh is
# dash — re-exec under bash. On macOS /bin/sh IS bash, so the common `| sh`
# path never re-execs there. This block must parse under plain POSIX sh, so it
# uses no bashisms itself.
LUCIDOS_INSTALL_URL="${LUCIDOS_INSTALL_URL:-https://raw.githubusercontent.com/lucidos-dev/lucidos/main/install.sh}"
if [ -z "${BASH_VERSION:-}" ]; then
    if command -v bash >/dev/null 2>&1; then
        if [ -f "$0" ] && [ -r "$0" ]; then
            # Started as `sh install.sh` — re-run the file under bash.
            exec bash "$0" "$@"
        else
            # Piped (`curl … | sh`) — no file to re-run, so re-fetch under bash.
            # Capture first and fail loudly if the fetch came back empty, rather
            # than exec'ing an empty `bash -c ""` (a silent no-op exit 0).
            _lucidos_payload="$(curl -fsSL "$LUCIDOS_INSTALL_URL")" || _lucidos_payload=""
            if [ -z "$_lucidos_payload" ]; then
                echo "ERROR: could not re-fetch the installer from $LUCIDOS_INSTALL_URL to run it under bash." >&2
                echo "       Re-run explicitly under bash:  curl -fsSL $LUCIDOS_INSTALL_URL | bash" >&2
                exit 1
            fi
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
LUCIDOS_REPO_URL="${LUCIDOS_REPO_URL:-https://github.com/lucidos-dev/lucidos.git}"
LUCIDOS_REF="${LUCIDOS_REF:-}"                                   # branch/tag/sha; empty = repo default
LUCIDOS_HOME="${LUCIDOS_HOME:-$HOME/lucidos}"                    # where the repo is cloned
LUCIDOS_WORKSPACE="${LUCIDOS_WORKSPACE:-$HOME/workspaces/lucidos}" # the workspace (data) directory
LUCIDOS_RELEASE_BUILD="${LUCIDOS_RELEASE_BUILD:-}"              # set to 1 for a release engine build (slower build, faster runtime)
LUCIDOS_SKIP_DEPS="${LUCIDOS_SKIP_DEPS:-}"                      # set to 1 to skip dependency bootstrap

# Provider config — if supplied, persisted to <workspace>/data/.env so the
# engine boots fully configured. With none of these set the installer boots in
# `mock` mode (no external calls) so the UI still comes up; configure a real
# provider in Settings → Providers afterwards.
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

# ── platform detection ──────────────────────────────────────────────────────
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
        local i
        for i in $(seq 1 120); do
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

# ── provider configuration ──────────────────────────────────────────────────
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
        info "then restart:  cd $LUCIDOS_HOME && ./scripts/stop.sh -w $LUCIDOS_WORKSPACE && ./scripts/web-dev.sh -w $LUCIDOS_WORKSPACE"
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

# ── launch ──────────────────────────────────────────────────────────────────
launch() {
    step "Starting Lucidos (PostgreSQL + engine + frontend)"
    printf '\n%s%sFirst run builds the engine from source — this can take 10–20+ minutes on a clean\nmachine. Subsequent runs reuse the build and start in seconds.%s\n\n' \
        "$C_DIM" "$C_BOLD" "$C_RESET"

    local build_flag="-b"
    [ -n "$LUCIDOS_RELEASE_BUILD" ] && build_flag="-b -r"

    # In mock mode, pass LUCIDOS_MODEL=mock for THIS launch only (not persisted),
    # so the stack comes up without credentials. Once a real provider is
    # configured a plain restart picks it up.
    cd "$LUCIDOS_HOME"
    if [ "$PROVIDER_MODE" = "mock" ]; then
        LUCIDOS_MODEL=mock ./scripts/web-dev.sh -w "$LUCIDOS_WORKSPACE" $build_flag \
            || die "scripts/web-dev.sh failed. Check the output above and the engine log under $LUCIDOS_WORKSPACE/.lucidos/engine.log"
    else
        ./scripts/web-dev.sh -w "$LUCIDOS_WORKSPACE" $build_flag \
            || die "scripts/web-dev.sh failed. Check the output above and the engine log under $LUCIDOS_WORKSPACE/.lucidos/engine.log"
    fi
}

# ── final banner ────────────────────────────────────────────────────────────
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
    printf '  Restart: cd %s && ./scripts/web-dev.sh -w %s\n' "$LUCIDOS_HOME" "$LUCIDOS_WORKSPACE"
    printf '  Logs:    tail -f %s/.lucidos/engine.log\n' "$LUCIDOS_WORKSPACE"
    printf '%s========================================%s\n\n' "$C_GREEN" "$C_RESET"
}

# ── main ────────────────────────────────────────────────────────────────────
main() {
    printf '%s%sLucidos installer%s\n' "$C_BOLD" "$C_BLUE" "$C_RESET"
    printf '%srepo=%s ref=%s home=%s workspace=%s%s\n\n' \
        "$C_DIM" "$LUCIDOS_REPO_URL" "${LUCIDOS_REF:-<default>}" "$LUCIDOS_HOME" "$LUCIDOS_WORKSPACE" "$C_RESET"

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

main "$@"
