#!/bin/bash
# flock.sh: hold a flock from the shell (ADR 0438).
#
# macOS ships no flock(1), so perl takes the lock. It locks a descriptor the
# shell opened, and flock binds to the open file description, so the lock stays
# held after perl exits. It is released when the last descriptor sharing that
# description closes: this shell's, and any child's that inherited it.
#
# So a long-lived child spawned while a lock is open must close that
# descriptor (`7>&-`), or it keeps the lock for its whole life.
#
# Readers trust a pidfile only while its lock is held (live_pid). Probe only a
# lock whose owner acquires blocking: a probe briefly holds a shared lock, and
# that makes an owner's non-blocking claim fail.
#
# Tested by flock_test.sh.

_flock_need_perl() {
    command -v perl >/dev/null 2>&1 && return 0
    echo "ERROR: taking a lock needs perl, which is not on PATH." >&2
    return 2
}

# Lock descriptor $1, shared or exclusive ($2), blocking or not ($3).
# Exits 0 when locked, 1 when another process holds it, 2 on any other error.
_flock_perl() { # <fd> <sh|ex> <nb|block>
    perl -MFcntl=:flock -e '
        my ($fd, $mode, $wait) = @ARGV;
        open(my $fh, ">&=", $fd) or exit 2;
        my $op = $mode eq "sh" ? LOCK_SH : LOCK_EX;
        $op |= LOCK_NB if $wait eq "nb";
        flock($fh, $op) and exit 0;
        exit($!{EWOULDBLOCK} ? 1 : 2);
    ' "$1" "$2" "$3"
}

_flock_fd_open() { # <fd> <path>
    mkdir -p "$(dirname "$2")" || return 2
    eval "exec $1>>\"\$2\"" || return 2
}

# Called once when flock_open finds the lock held, just before it waits.
# Overridden by tests as a synchronization point.
_flock_waiting() { # <path>
    echo "Waiting for $1, held by another process..." >&2
}

# Open <path> on descriptor <fd> and lock it, waiting for the holder.
# Returns 0 once locked. On failure the descriptor is closed again.
flock_open() { # <fd> <path> <sh|ex>
    local fd="$1" path="$2" mode="$3" rc=0
    _flock_need_perl || return 2
    _flock_fd_open "$fd" "$path" || return 2
    _flock_perl "$fd" "$mode" nb || rc=$?
    if [ "$rc" -eq 1 ]; then
        _flock_waiting "$path"
        rc=0
        _flock_perl "$fd" "$mode" block || rc=$?
    fi
    [ "$rc" -eq 0 ] && return 0
    flock_close "$fd"
    return "$rc"
}

# Like flock_open, but never waits. Returns 1 when another process holds it.
flock_try() { # <fd> <path> <sh|ex>
    local fd="$1" path="$2" mode="$3" rc=0
    _flock_need_perl || return 2
    _flock_fd_open "$fd" "$path" || return 2
    _flock_perl "$fd" "$mode" nb || rc=$?
    [ "$rc" -eq 0 ] && return 0
    flock_close "$fd"
    return "$rc"
}

flock_close() { # <fd>
    eval "exec $1>&-"
}

# Returns 0 when some process holds the lock at <path>, 1 when nobody does
# (a missing file included), and 2 when the probe itself failed.
lock_is_held() { # <path>
    [ -e "$1" ] || return 1
    _flock_need_perl || return 2
    perl -MFcntl=:flock -e '
        open(my $fh, "<", $ARGV[0]) or exit 2;
        flock($fh, LOCK_SH | LOCK_NB) and exit 1;
        exit($!{EWOULDBLOCK} ? 0 : 2);
    ' "$1"
}

# The pid in <pidfile>, printed only while the lock at <lock> is held. Prints
# nothing for no holder, an unreadable file, or a pid of 1 or less.
live_pid() { # <lock> <pidfile>
    lock_is_held "$1" || return 0
    local pid
    pid="$(cat "$2" 2>/dev/null || true)"
    case "$pid" in '' | *[!0-9]*) return 0 ;; esac
    if [ "$pid" -gt 1 ]; then echo "$pid"; fi
}

# The pid in <pidfile> when that process is alive and its argv[0] is <binary>,
# else nothing. Selection is on argv[0], never the whole command line (ADR
# 0025), so a process that merely mentions <binary> never matches.
pidfile_pid_running() { # <pidfile> <binary>
    local pid
    pid="$(cat "$1" 2>/dev/null || true)"
    if pid_runs "$pid" "$2"; then echo "$pid"; fi
}

# True when <pid> is a live process whose argv[0] is <binary>.
pid_runs() { # <pid> <binary>
    local argv0
    case "$1" in '' | *[!0-9]*) return 1 ;; esac
    [ "$1" -gt 1 ] || return 1
    argv0="$(ps -o command= -p "$1" 2>/dev/null)" || return 1
    argv0="${argv0%% *}"
    [ "${argv0##*/}" = "$2" ]
}

# The paths match lucidos_owner_lock::engine. A gateway test pins them
# (the_shell_reads_the_pidfiles_the_owners_write).
engine_lock_path() { echo "$1/.lucidos/engine.lock"; }
engine_pid_path() { echo "$1/.lucidos/engine.pid"; }
engine_successor_lock_path() { echo "$1/.lucidos/engine.successor.lock"; }
engine_successor_pid_path() { echo "$1/.lucidos/engine.successor.pid"; }

# The pid of the engine that owns <workspace> right now, or nothing.
#
# No lock file means no lock-aware engine has ever run here, so the pidfile
# still follows the old contract: a launcher wrote it. Its pid counts then only
# while that process is alive and its argv[0] is lucidos-engine. Temporary
# measure: docs/temporary-measures.md § "Pre-lock engine pidfile fallback".
engine_live_pid() { # <workspace>
    local lock
    lock="$(engine_lock_path "$1")"
    if [ -e "$lock" ]; then
        live_pid "$lock" "$(engine_pid_path "$1")"
    else
        pidfile_pid_running "$(engine_pid_path "$1")" lucidos-engine
    fi
}

# The pid of an engine waiting to own <workspace>, while it waits. A stop that
# lands mid-respawn signals it too, or it boots once the owner is gone.
engine_waiting_pid() { # <workspace>
    live_pid "$(engine_successor_lock_path "$1")" "$(engine_successor_pid_path "$1")"
}

# The pids to signal to stop <workspace>'s engine: its owner, and a successor
# waiting mid-respawn. The lock check and the signal are two steps, and the OS
# reuses pids, so each must still run lucidos-engine, as the gateway and the
# desktop app also check.
engine_pids_to_signal() { # <workspace>
    local pid
    for pid in $(engine_live_pid "$1") $(engine_waiting_pid "$1"); do
        if pid_runs "$pid" lucidos-engine; then echo "$pid"; fi
    done
}
