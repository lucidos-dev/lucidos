#!/usr/bin/env bash
# ps_tools.sh: give a script a `ps` that runs inside an agent's OS sandbox.
#
# macOS refuses to exec a setuid binary from a seatbelt sandbox (ADR 0645), and
# /bin/ps is setuid root. There, ps_tools_env puts scripts/lib/bin first on PATH,
# where a stand-in reads the same facts through libproc. A `ps` already on PATH
# that runs is kept: the engine's shim for `lucidos ps` reads more fields and
# zombies. Everywhere else it changes nothing.
#
# Assignments and functions only, so any script or hermetic test can source it.

_PS_TOOLS_BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/bin" && pwd)"

# _ps_tools_path_ps_runs: whether the ps on PATH can run here. A seam for tests.
_ps_tools_path_ps_runs() {
    ps -p $$ -o pid= >/dev/null 2>&1
}

# ps_tools_env: put the stand-in first on PATH when the ps on PATH cannot run.
ps_tools_env() {
    [ "$(uname -s)" = Darwin ] || return 0
    _ps_tools_path_ps_runs && return 0
    case ":$PATH:" in
        *":$_PS_TOOLS_BIN:"*) ;;
        *) export PATH="$_PS_TOOLS_BIN:$PATH" ;;
    esac
}
