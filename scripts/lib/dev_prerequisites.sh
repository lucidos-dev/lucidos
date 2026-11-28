#!/usr/bin/env bash
# shellcheck source=scripts/lib/shellcheck_pin.sh
source "$(dirname "${BASH_SOURCE[0]}")/shellcheck_pin.sh"

dp_platform() {
    if [[ $(uname -s) == Darwin ]]; then
        echo brew
        return
    fi
    local distro
    distro="$(awk -F= '/^ID(_LIKE)?=/ {gsub(/"/, "", $2); print $2}' /etc/os-release 2>/dev/null)" || distro=
    case " ${distro//$'\n'/ } " in
        *" alpine "*) echo alpine ;;
        *" debian "* | *" ubuntu "*) command -v apt >/dev/null && echo apt ;;
        *" fedora "* | *" rhel "*) command -v dnf >/dev/null && echo dnf ;;
        *" arch "*) command -v pacman >/dev/null && echo pacman ;;
    esac
    return 0
}

dp_python() {
    python3 -c 'import venv, ensurepip' >/dev/null 2>&1 && return 0
    echo 'ERROR: Python 3 with venv and ensurepip is required.' >&2
    case "$1" in
        apt) echo 'Run sudo apt install python3-venv (or python3.X-venv for your Python version).' >&2 ;;
        dnf) echo 'Run sudo dnf install python3, or sudo dnf reinstall python3.' >&2 ;;
        pacman) echo 'Run sudo pacman -S python.' >&2 ;;
        brew) echo 'Run brew install python, or brew reinstall python.' >&2 ;;
        *) echo 'Install Python 3 with its venv and ensurepip modules.' >&2 ;;
    esac
    return 1
}

dp_shell_path() {
    local location binary
    location="$(npx --no-install playwright install --dry-run --only-shell chromium |
        awk '/playwright chromium-headless-shell v/ {shell=1} shell && /Install location:/ {
            sub(/^[[:space:]]*Install location:[[:space:]]*/, ""); print; exit
        }')" || return 1
    [[ -n "$location" ]] || return 1
    for binary in "$location"/*/chrome-headless-shell "$location"/*/headless_shell; do
        if [[ -x "$binary" ]]; then printf '%s\n' "$binary"; return; fi
    done
    return 1
}

dp_chromium_fix() {
    local platform=$1 library package packages=()
    shift
    case "$platform" in
        apt) echo 'Run cd crates/lucidos-app && sudo npx playwright install-deps chromium.' ;;
        dnf)
            for library in "$@"; do
                case "$library" in
                    libnspr4.so|libplc4.so|libplds4.so) package=nspr ;;
                    libnss*.so|libsmime3.so) package=nss ;;
                    libasound.so.*) package=alsa-lib ;;
                    *) package="*/$library" ;;
                esac
                packages+=("$package")
            done
            printf 'Run sudo dnf install'
            printf ' %q' "${packages[@]}"
            printf '\n'
            ;;
        pacman)
            for library in "$@"; do
                case "$library" in
                    libnspr4.so|libplc4.so|libplds4.so) package=nspr ;;
                    libnss*.so|libsmime3.so) package=nss ;;
                    libasound.so.*) package=alsa-lib ;;
                    libatk*.so.*) package=at-spi2-core ;;
                    libX11.so.*) package=libx11 ;;
                    libXcomposite.so.*) package=libxcomposite ;;
                    libXdamage.so.*) package=libxdamage ;;
                    libXext.so.*) package=libxext ;;
                    libXfixes.so.*) package=libxfixes ;;
                    libXrandr.so.*) package=libxrandr ;;
                    libxcb.so.*) package=libxcb ;;
                    libgbm.so.*) package=mesa ;;
                    libcairo.so.*) package=cairo ;;
                    libpango*.so.*) package=pango ;;
                    libglib*.so.*|libgio*.so.*|libgobject*.so.*) package=glib2 ;;
                    libcups.so.*) package=libcups ;;
                    libdbus*.so.*) package=dbus ;;
                    *) printf 'Find the package with pacman -F %q.\n' "$library"; continue ;;
                esac
                packages+=("$package")
            done
            if [[ -n "${packages[*]:-}" ]]; then
                printf 'Run sudo pacman -S'
                printf ' %q' "${packages[@]}"
                printf '\n'
            fi
            ;;
        *) echo 'Install the packages that provide the missing libraries listed above.' ;;
    esac >&2
}

dp_chromium() (
    local platform=$2 binary links missing library libraries=()
    if [[ "$platform" == alpine ]] || compgen -G '/lib/ld-musl-*.so.1' >/dev/null; then
        echo 'ERROR: Playwright Chromium requires a supported glibc Linux host.' >&2
        return 1
    fi
    cd "$1/crates/lucidos-app" || return 1
    if ! binary="$(dp_shell_path)" &&
        ! { npx --no-install playwright install --only-shell chromium && binary="$(dp_shell_path)"; }; then
        echo 'ERROR: Chromium headless shell is missing. Run cd crates/lucidos-app && npx playwright install --only-shell chromium.' >&2
        return 1
    fi
    [[ $(uname -s) == Linux ]] || return 0
    links="$(ldd "$binary")" || {
        echo 'ERROR: Could not inspect Chromium shared libraries. Check that ldd works on this host.' >&2
        return 1
    }
    missing="$(awk '$2 == "=>" && $3 == "not" && $4 == "found" {print $1}' <<< "$links" | sort -u)"
    [[ -n "$missing" ]] || return 0
    echo 'ERROR: Chromium is missing shared libraries:' >&2
    while IFS= read -r library; do
        printf '  %s\n' "$library" >&2
        libraries+=("$library")
    done <<< "$missing"
    dp_chromium_fix "$platform" "${libraries[@]}"
    return 1
)

dev_prerequisites() {
    local root=$1 suites=",$2," platform failed=0
    platform="$(dp_platform)"
    case "$suites" in
        *,lint-static,*) shellcheck_resolve >/dev/null || failed=1 ;;
    esac
    case "$suites" in
        *,ts,* | *,vitest,*) dp_chromium "$root" "$platform" || failed=1 ;;
    esac
    case "$suites" in
        *,engine,* | *,engine-touched,* | *,engine-filtered,* | *,agents,* | *,confinement,*) dp_python "$platform" || failed=1 ;;
    esac
    return "$failed"
}
