#!/bin/bash
# Docker daemon state, and the report a launcher prints when it is down.
#
# Sourced by BOTH scripts/lib/preflight.sh (launch time, before anything is
# built or started) and scripts/lib/workspace.sh (provision time, right before
# the first `docker` call), so the two cannot disagree about what "down" means
# or word it differently.
#
# ── The probe is the exit status of `docker version`, never an error string ──
# This mirrors `docker_daemon_state` in crates/lucidos-gateway/src/postgres.rs
# ON PURPOSE: that classifier is what decides whether a workspace's provisioning
# failure is retried (transient, the daemon is still starting) or latched
# (terminal, there is no docker CLI at all), and a shell half that classified
# differently would tell the user one thing while the gateway did another. Keep
# the two in step.
#
# `docker version --format {{.Server.Version}}` is the probe because its exit
# STATUS answers the question: the server half fails when the daemon is
# unreachable, so a zero exit means the daemon answered. `docker inspect` cannot
# be used for this: it exits 1 identically for "no such container" and "the
# daemon is down", which is what made the 2026-08-03 login race surface as a
# confusing `docker run failed: failed to connect to the docker API at
# unix:///...`. `docker info` (what preflight used before) does answer correctly,
# but it is a second vocabulary for one condition, so it is gone.
#
# The daemon's own error TEXT is read too, but only to quote in the report, never
# to classify. That keeps the "no message matching" rule intact while letting the
# report say `permission denied` instead of guessing a remedy for it.

# Include guard: both sourcing libraries may be loaded by the same script.
if [ -n "${LUCIDOS_DOCKER_SH_LOADED:-}" ]; then
    return 0
fi
LUCIDOS_DOCKER_SH_LOADED=1

# How long `ensure_docker_daemon` waits for a just-launched Docker Desktop to
# answer. Docker Desktop routinely takes 30-60s from a cold login, so a short
# ceiling would report a failure the user is seconds away from not having.
DOCKER_START_TIMEOUT_S="${DOCKER_START_TIMEOUT_S:-120}"

# ── Seams ───────────────────────────────────────────────────────────────────
# The three functions that actually touch the host. A test overrides these and
# nothing else, so no test can start Docker Desktop or reach a real daemon.

# Is a `docker` executable on this shell's PATH? The structural stand-in for the
# gateway's spawn-error `NotFound` arm.
_docker_cli_present() {
    command -v docker >/dev/null 2>&1
}

# Which OS are we advising? A seam, not a convenience: the remedies below differ
# per platform and the whole point of the report is that they are CORRECT, so the
# non-Darwin arms have to be reachable from a test running on a Mac.
_docker_host_os() {
    uname
}

# Ask the daemon for its version. Only the exit status is read.
_docker_version_probe() {
    docker version --format '{{.Server.Version}}' >/dev/null 2>&1
}

# The same call's stderr, for quoting in the report. Never classified on.
# The brace group is the unambiguous spelling of "stderr only": fd2 is pointed at
# the caller's stdout first, then the command's own stdout is dropped inside. The
# terser `2>&1 >/dev/null` does the same thing but is indistinguishable from the
# common mistake, and ShellCheck flags it (SC2069).
_docker_version_stderr() {
    { docker version --format '{{.Server.Version}}' >/dev/null; } 2>&1
}

# ── Classification ──────────────────────────────────────────────────────────

# Pure: (cli-present status, probe status) in, state word out. Split from the
# seams so the whole decision table is unit-testable without a host.
#
# $1 = 0 when a docker CLI is on PATH, non-zero otherwise
# $2 = exit status of the version probe (ignored when the CLI is absent)
#
# Prints one of:
#   ready        the daemon answered
#   unreachable  a CLI is present but the daemon did not answer (may clear)
#   missing      no docker CLI on PATH (waiting cannot fix this)
classify_docker_probe() {
    local cli_status="$1" probe_status="$2"
    if [ "$cli_status" -ne 0 ]; then
        printf 'missing\n'
        return 0
    fi
    if [ "$probe_status" -eq 0 ]; then
        printf 'ready\n'
        return 0
    fi
    printf 'unreachable\n'
}

# The live state of the Docker daemon, as one of the three words above.
docker_daemon_state() {
    local cli_status probe_status
    _docker_cli_present
    cli_status=$?
    if [ "$cli_status" -ne 0 ]; then
        classify_docker_probe "$cli_status" 1
        return 0
    fi
    _docker_version_probe
    probe_status=$?
    classify_docker_probe "$cli_status" "$probe_status"
}

# ── Reporting ───────────────────────────────────────────────────────────────

# The first meaningful line of the daemon's own complaint, trimmed to fit the
# report. Empty when it said nothing useful. Report-only, by construction: the
# state word is already decided before this runs.
docker_probe_detail() {
    local line
    line="$(_docker_version_stderr | grep -v '^[[:space:]]*$' | head -1)"
    # The CLI prefixes its own noise on some versions; keep the sentence.
    line="${line#error during connect: }"
    line="${line#Error response from daemon: }"
    [ ${#line} -gt 120 ] && line="${line:0:117}..."
    printf '%s\n' "$line"
}

# Wrap a value in quotes only when pasting it back would otherwise split it. The
# report's re-run line exists to be copied, and a workspace path with a space in
# it would produce a command that silently means something else. Not a general
# shell quoter, and does not pretend to be: it covers whitespace, which is the
# only thing a workspace path realistically carries.
_docker_paste_safe() {
    case "$1" in
        *[[:space:]]*) printf '"%s"\n' "$1" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

# The command that would re-run the caller, for the report's "then re-run" line.
# `parse_dev_args` has already populated WORKSPACE (raw, pre-resolution) and
# BUILD by the time any launcher reaches the preflight, so this reproduces what
# the user actually typed rather than a generic placeholder they have to adapt.
_docker_rerun_command() {
    local script="${SCRIPT_NAME:-web-dev.sh}" cmd
    cmd="./scripts/$script"
    [ -n "${WORKSPACE:-}" ] && cmd="$cmd -w $(_docker_paste_safe "${WORKSPACE}")"
    [ -n "${BUILD:-}" ] && cmd="$cmd -b"
    [ -n "${RELEASE:-}" ] && cmd="$cmd -r"
    printf '%s\n' "$cmd"
}

# One `label: value` row of the report, on a fixed column so every row lines up
# however many the branch below emits.
_docker_report_row() {
    printf '  %-16s %s\n' "$1" "$2"
}

# One visually delimited block naming the condition, why it blocks everything,
# and the commands that resolve it. Deliberately NOT two quiet lines in the
# middle of a launch: the previous wording scrolled past unnoticed and the run
# read as "the workspaces just didn't start".
#
# **The remedies are platform-correct and are not guesses.** Two rules:
#
#   * They match what `install.sh`'s `ensure_docker_installed` /
#     `ensure_docker_running` already tell a from-source installer, because that
#     is the SAME user hitting the SAME condition one step earlier: the `--dev`
#     install path runs those and then launches `run.sh`, which reaches here.
#     Two surfaces contradicting each other about how to start Docker is worse
#     than either one alone. (The duplication is structural, not laziness:
#     `install.sh` is fetched standalone by `curl | sh` before any checkout
#     exists, so it cannot source this file.)
#   * An `unreachable` daemon is NOT assumed to be a stopped service. The state
#     is deliberately classified without deciding WHY (see the header), and on
#     Linux "permission denied" (the user is not in the `docker` group, which
#     `install.sh` diagnoses explicitly) is as common as a stopped one. Printing
#     `systemctl start docker` as THE fix would send half of those users to run
#     a command that changes nothing. So both candidates are offered, and the
#     quoted `Docker said:` line is what tells them which one they have.
#
# $1 = state word from docker_daemon_state (`unreachable` or `missing`)
# Written to stderr, so it survives a caller piping stdout somewhere.
docker_down_report() {
    local state="$1" rule rerun detail="" os
    rule="  ────────────────────────────────────────────────────────────"
    rerun="$(_docker_rerun_command)"
    os="$(_docker_host_os)"
    [ "$state" = "unreachable" ] && detail="$(docker_probe_detail)"
    {
        echo ""
        echo "$rule"
        if [ "$state" = "missing" ]; then
            echo "  Docker is not installed, so no workspace can start."
        else
            echo "  Docker is not running, so no workspace can start."
        fi
        echo ""
        echo "  Lucidos keeps every workspace's data in PostgreSQL, which runs"
        echo "  in a Docker container. Nothing starts until Docker is up."
        if [ -n "$detail" ]; then
            echo ""
            _docker_report_row "Docker said:" "$detail"
        fi
        echo ""
        if [ "$state" = "missing" ]; then
            if [ "$os" = "Darwin" ]; then
                _docker_report_row "Install Docker:" "brew install --cask docker"
            else
                _docker_report_row "Install Docker:" "https://docs.docker.com/engine/install/"
            fi
        elif [ "$os" = "Darwin" ]; then
            _docker_report_row "Start Docker:" "open -a Docker"
        else
            _docker_report_row "If stopped:" "sudo systemctl start docker"
            _docker_report_row "If denied:" "add yourself to the 'docker' group, then log back in"
        fi
        _docker_report_row "Then re-run:" "$rerun"
        echo "$rule"
        echo ""
    } >&2
}

# ── The entry point launchers call ──────────────────────────────────────────

# Prompt y/n defaulting to YES. Sibling to `_confirm` in preflight.sh, which
# defaults to NO because it guards *installing* something. This one guards
# starting an application the user already installed, so the safe default is the
# helpful one. Non-interactive returns 1, same as `_confirm`, so a launcher
# spawned by the restart API never blocks on a keystroke.
_confirm_default_yes() {
    local prompt="$1" yn
    if ! [ -t 0 ]; then
        return 1
    fi
    read -r -p "$prompt " yn
    [[ ! "$yn" =~ ^[Nn]$ ]]
}

# Wait for a starting daemon to answer, printing a progress line so the wait
# reads as progress rather than a hang. Returns 0 once ready, 1 on timeout.
# $1 = seconds to wait (defaults to DOCKER_START_TIMEOUT_S)
wait_for_docker_daemon() {
    local budget="${1:-$DOCKER_START_TIMEOUT_S}" waited=0
    printf '  Waiting for Docker'
    while [ "$waited" -lt "$budget" ]; do
        if [ "$(docker_daemon_state)" = "ready" ]; then
            printf ' ready!\n'
            return 0
        fi
        printf '.'
        sleep 1
        waited=$((waited + 1))
    done
    printf '\n'
    return 1
}

# Guarantee a reachable Docker daemon, or exit 1 with the report.
#
# On macOS an unreachable daemon is OFFERED the remedy this script already knows
# (`open -a Docker`) rather than being handed back to the user with instructions:
# it is the one prereq condition with a one-word fix, and it was also the only
# one check_prereqs refused to act on. Defaulting to yes (unlike the install
# prompts) is deliberate: this starts an application the user already installed,
# it does not install anything.
#
# Every other outcome (declined, non-interactive, `open` failed, timed out, no
# CLI, non-Darwin) prints the report and exits, so a launch never proceeds into
# a `docker run` that cannot work.
ensure_docker_daemon() {
    local state
    state="$(docker_daemon_state)"
    [ "$state" = "ready" ] && return 0

    # `-t 0` gates the LEAD-IN as well as the prompt, not just the prompt: a
    # non-interactive launch (the restart API spawns one) would otherwise print a
    # sentence introducing a question nobody is going to be asked, immediately
    # above a block that states the same thing.
    if [ "$state" = "unreachable" ] && [ "$(_docker_host_os)" = "Darwin" ] && [ -t 0 ]; then
        echo ""
        echo "  Docker is installed but the daemon isn't running."
        if _confirm_default_yes "  Start Docker Desktop? [Y/n]"; then
            if open -a Docker 2>/dev/null; then
                wait_for_docker_daemon && return 0
                # We launched it and it still is not answering, so the report's
                # "open -a Docker" line alone would just tell the user to redo
                # what they watched fail. Name the case that actually produces
                # this, in the same words install.sh uses for it.
                echo "  Docker Desktop did not become ready in ${DOCKER_START_TIMEOUT_S}s." >&2
                echo "  On a first run, finish its setup (accept the terms) and try again." >&2
            else
                echo "  Could not launch Docker Desktop (is it installed as an app?)." >&2
            fi
        fi
    fi

    docker_down_report "$state"
    exit 1
}

# Report-only sibling for callers already past the launch preflight
# (`setup_postgres`), where the user is not sitting at a prompt for this script
# and a second offer to start Docker would be noise. Returns non-zero when the
# daemon is not reachable so the caller can fail with its own context, after the
# named cause has been printed instead of a raw `docker run failed:` string.
report_docker_daemon_if_down() {
    local state
    state="$(docker_daemon_state)"
    [ "$state" = "ready" ] && return 0
    docker_down_report "$state"
    return 1
}
