#!/bin/bash
# proc_env.sh: read one variable out of a process's own environment.
#
# `ps -E -p <pid> -o command=` prints argv first, then NAME=value pairs, all on
# one line. That is the only way to learn what a running Lucidos process was
# launched with, and two callers need it:
#
#   scripts/lib/workspace.sh       LUCIDOS_GATEWAY_PORT, so stop.sh asks the
#                                  gateway that owns the workspace
#   scripts/lib/preflight_reclaim.sh   LUCIDOS_WORKSPACE and
#                                  LUCIDOS_WORKSPACE_ID, so a reclaim never
#                                  builds a path from an assumed layout
#
# Both had a copy of the parse. One is enough, and the space-in-a-path rule
# below is exactly the kind of detail that would have drifted between two.
#
# The PROCESS LISTING is deliberately NOT here. `preflight_reclaim.sh` overrides
# its own `ps` call as a test seam, and a seam its tests cannot reach is no seam
# (ADR 0025). `proc_env_value` parses a line somebody else read.
#
# `proc_env_has_entry` is the one reader that runs `ps` itself, because it must
# see argv and the environment separately. It is a function, so a test that
# needs synthetic environments overrides it after sourcing this file.

# One variable's value out of a `ps -E` line. Prints nothing when it is absent.
#
#   proc_env_value "<ps -E line>" LUCIDOS_WORKSPACE
#
# The value runs to the next NAME= token rather than to the next space, because
# a real workspace path holds one: the packaged app lives under "Library/
# Application Support". Matching on a leading space means a variable whose name
# ends in another's name (FOO= inside BARFOO=) is not mistaken for it.
proc_env_value() {
    printf '%s' "$1" | awk -v name="$2" '
        {
            needle = " " name "="
            line = " " $0
            i = index(line, needle)
            if (i == 0) exit
            rest = substr(line, i + length(needle))
            if (match(rest, / [A-Za-z_][A-Za-z0-9_]*=/))
                rest = substr(rest, 1, RSTART - 1)
            print rest
        }
    '
}

# Does pid $1's environment hold the exact entry $2 (NAME=VALUE)? The e2e sweeps
# decide a browser is theirs this way (ADR 0251). The kernel records the
# environment at exec, and no prompt text can write into another process's.
#
# Never argv, unlike `proc_env_value`. Linux gives the environment alone, NUL
# separated. macOS `ps -E` prints argv THEN the environment, so the argv line is
# stripped off as a prefix first; a line that does not start with it answers no.
# Every failure answers no, so an unreadable environment never authorizes a kill.
# macOS hides the environment of Apple platform binaries, which answer no too.
proc_env_has_entry() {
    local pid="$1" pair="$2" argv full
    [ -n "$pair" ] || return 1
    if [ -d "/proc/$pid" ]; then
        tr '\0' '\n' 2>/dev/null < "/proc/$pid/environ" | grep -qxF -- "$pair"
        return
    fi
    argv="$(ps -ww -p "$pid" -o command= 2>/dev/null)" || return 1
    full="$(ps -E -ww -p "$pid" -o command= 2>/dev/null)" || return 1
    [ -n "$argv" ] || return 1
    case "$full" in
        "$argv"*) ;;
        *) return 1 ;;
    esac
    case "${full#"$argv"} " in
        *" $pair "*) return 0 ;;
    esac
    return 1
}
