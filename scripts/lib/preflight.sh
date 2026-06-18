#!/bin/bash
# Preflight: verify required tools are installed, offer to install missing ones.
# Sourced by web-dev.sh and tauri-dev.sh. Call check_prereqs after parse_dev_args.
#
# UX: silent on the happy path. If a tool is missing we ask "Install <tool>? [y/N]"
# without printing the install command — install on Y, exit on N for required tools.

# Run an install command and re-check that the tool is now on PATH afterwards.
# Distinguishes "install command failed" from "installed but not on PATH" so the
# user gets the right next step. `set -eo pipefail` in the subshell makes
# `curl | sh` style installers fail loudly instead of silently exiting 0.
_run_install() {
    local tool="$1" cmd="$2" script="${SCRIPT_NAME:-this script}"
    if ! bash -c "set -eo pipefail; $cmd"; then
        echo "  Install command failed (see output above)." >&2
        return 1
    fi
    # Some installers (rustup, brew on Apple Silicon) put binaries in dirs
    # not yet in this shell's PATH; source common shims so the verify works.
    [ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
    [ -x "/opt/homebrew/bin/brew" ] && eval "$(/opt/homebrew/bin/brew shellenv)"
    [ -x "/usr/local/bin/brew" ] && eval "$(/usr/local/bin/brew shellenv)"
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "  '$tool' installed but not on PATH. Open a new shell and re-run $script." >&2
        return 1
    fi
}

# Prompt y/n; default N. Returns 0 for yes, 1 for no.
_confirm() {
    local prompt="$1" yn
    if ! [ -t 0 ]; then
        return 1
    fi
    read -r -p "$prompt " yn
    [[ "$yn" =~ ^[Yy]$ ]]
}

# Check tool; if missing, prompt and install. Exit 1 if required and user declines.
# $1 tool, $2 description, $3 install command, $4 "required" or "recommended"
_check_or_install() {
    local tool="$1" why="$2" install_cmd="$3" level="$4"
    if command -v "$tool" >/dev/null 2>&1; then
        return 0
    fi
    echo ""
    echo "  Missing: $tool — $why"
    if _confirm "  Install $tool? [y/N]"; then
        if _run_install "$tool" "$install_cmd"; then
            echo "  OK: $tool installed."
            return 0
        fi
        # _run_install printed the specific failure reason
        exit 1
    fi
    if [ "$level" = "required" ]; then
        echo "  Cannot proceed without '$tool'. Exiting." >&2
        exit 1
    fi
    echo "  Skipping (recommended only)."
}

check_prereqs() {
    if [[ "$OSTYPE" != "darwin"* ]]; then
        # Linux/Windows: no auto-install, but warn on missing required tools.
        for t in cargo sccache cmake docker node; do
            command -v "$t" >/dev/null 2>&1 || echo "Warning: '$t' not found in PATH." >&2
        done
        return 0
    fi

    # Homebrew bootstraps everything else.
    _check_or_install brew    "Homebrew package manager" \
        '/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"' required

    # Required — engine/frontend won't build/run without these.
    _check_or_install cargo   "Rust toolchain (engine build)" \
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y" required
    _check_or_install sccache "Rust compile cache" \
        "brew install sccache" required
    _check_or_install cmake   "Required to build native Rust dependencies" \
        "brew install cmake" required
    _check_or_install docker  "PostgreSQL+pgvector container runtime" \
        "brew install --cask docker && open -a Docker" required
    _check_or_install node    "Frontend / Vite dev server" \
        "brew install node" required

    # Recommended — used by helper scripts.
    _check_or_install jq   "Used by helper scripts for JSON parsing" \
        "brew install jq" recommended
    _check_or_install psql "Used by helper scripts for DB introspection" \
        "brew install libpq && brew link --force libpq" recommended

    # Docker daemon must be running before setup_postgres can connect.
    if ! docker info >/dev/null 2>&1; then
        local script="${SCRIPT_NAME:-this script}"
        echo ""
        echo "  Docker is installed but the daemon isn't running."
        echo "  Open Docker Desktop and wait for it to start, then re-run $script." >&2
        exit 1
    fi
}

# Tauri CLI (`cargo tauri`) — only tauri-dev.sh needs it, so it lives outside
# check_prereqs (web-dev.sh shares that and must not force a CLI compile).
# The installed binary is `cargo-tauri`, so detect that, not `tauri`. The repo
# is on Tauri v2 (see crates/lucidos-app/Cargo.toml), so pin the CLI to the v2
# major. Call after check_prereqs so cargo is guaranteed present.
check_tauri_cli() {
    if [[ "$OSTYPE" != "darwin"* ]]; then
        command -v cargo-tauri >/dev/null 2>&1 || \
            echo "Warning: 'cargo-tauri' not found — run: cargo install tauri-cli --version '^2.0' --locked" >&2
        return 0
    fi
    _check_or_install cargo-tauri "Tauri v2 CLI (opens the desktop window)" \
        "cargo install tauri-cli --version '^2.0' --locked" required
}
