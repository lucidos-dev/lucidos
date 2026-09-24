# shellcheck shell=bash
# The host memory watch: records a runaway process while it is still alive, and
# kills one that has outgrown the host's physical RAM.
#
# A `grep | head` once grew to 261 GB of compressed memory on a 48 GB Mac, filled
# the VM compressor and froze the host overnight. The jetsam reports kept only a
# name, a pid and an age, so nobody could say what launched it. This watch keeps
# the missing half: the full command line, the working directory and the parent
# chain, written while the process runs. Plan:
# docs/plans/2026-09-24-host-memory-watch-and-in-chunk-stop.md.
#
# One tick is one `top` sample, whose MEM column is the physical footprint and
# counts compressed pages. RSS does not, and that grep was almost all compressed.
# `ps` and `lsof` run only for a process already over the log threshold.
#
# THE KILL THRESHOLD IS PHYSICAL RAM. A footprint larger than RAM is reachable
# only through the compressor, so no healthy process gets there. It never kills
# pid 0 or 1, a protected pid (is_protected_host_pid, ADR 0025), or a process of
# another user. The record is on disk before the first signal.

# Knobs (all optional):
#   MEMORY_WATCH_LOG_PCT      footprint that is recorded, as a share of RAM (default 20)
#   MEMORY_WATCH_KILL_PCT     footprint that is killed, as a share of RAM (default 100,
#                             0 turns killing off, 1 to 49 fall back to the default)
#   MEMORY_WATCH_LOG_MAX_KB   size at which the log rotates, keeping one old file (default 10240)
#   MEMORY_WATCH_DIR          where the log and state live (default ~/.lucidos/memory-watch)

# The longest command line kept in a record. A coding agent's argv carries a
# system prompt of about 22 KB, which would bury the log.
MEMORY_WATCH_COMMAND_MAX=400

# ── measurement seams (overridable in tests) ────────────────────────────

_mw_top() {
    top -l 1 -o mem -n 30 -stats pid,mem 2>/dev/null
}

# One process's footprint, for the check right before a kill.
_mw_top_pid() {
    top -l 1 -pid "$1" -stats pid,mem 2>/dev/null
}

# One tab-separated row: pid, ppid, pgid, uid, start time, command. Empty when
# the process has gone.
_mw_proc_info() {
    ps -o pid=,ppid=,pgid=,uid=,lstart=,command= -p "$1" 2>/dev/null | awk '
        {
            printf "%s\t%s\t%s\t%s\t%s %s %s %s %s\t", $1, $2, $3, $4, $5, $6, $7, $8, $9
            for (i = 1; i <= 9; i++) $i = ""
            sub(/^ +/, "")
            print
        }'
}

_mw_cwd() {
    lsof -a -d cwd -p "$1" -Fn 2>/dev/null | sed -n 's/^n//p' | sed -n 1p
}

_mw_physmem_bytes() {
    sysctl -n hw.memsize 2>/dev/null
}

_mw_wait() {
    sleep "$1"
}

_mw_now() {
    date '+%Y-%m-%dT%H:%M:%S%z'
}

# ── knobs ───────────────────────────────────────────────────────────────

_mw_dir() {
    printf '%s' "${MEMORY_WATCH_DIR:-$HOME/.lucidos/memory-watch}"
}

_mw_log_pct() {
    local v="${MEMORY_WATCH_LOG_PCT:-20}"
    case "$v" in '' | *[!0-9]* | 0) v=20 ;; esac
    [ "$v" -le 100 ] || v=20
    printf '%s' "$v"
}

# Off, or at least half of RAM. A share under half is far more likely a typo than
# a decision to kill ordinary large processes.
_mw_kill_pct() {
    local v="${MEMORY_WATCH_KILL_PCT:-100}"
    case "$v" in '' | *[!0-9]*) v=100 ;; esac
    if [ "$v" -ne 0 ] && [ "$v" -lt 50 ]; then v=100; fi
    printf '%s' "$v"
}

_mw_log_max_kb() {
    local v="${MEMORY_WATCH_LOG_MAX_KB:-10240}"
    case "$v" in '' | *[!0-9]* | 0) v=10240 ;; esac
    printf '%s' "$v"
}

# ── the pieces ──────────────────────────────────────────────────────────

# `top` output to "pid bytes" lines. MEM reads like 7101M, 657M+ or 1.5G-; the
# trailing sign is top's delta marker. Anything that does not parse is dropped.
_mw_parse_top() {
    awk '
        $1 == "PID" { go = 1; next }
        go && $1 ~ /^[0-9]+$/ {
            m = $2
            sub(/[+-]$/, "", m)
            u = substr(m, length(m))
            n = substr(m, 1, length(m) - 1)
            if (n !~ /^[0-9.]+$/) next
            mult = 0
            if (u == "B") mult = 1
            if (u == "K") mult = 1024
            if (u == "M") mult = 1048576
            if (u == "G") mult = 1073741824
            if (u == "T") mult = 1099511627776
            if (mult == 0) next
            printf "%s %.0f\n", $1, n * mult
        }'
}

_mw_gb() {
    awk -v b="$1" 'BEGIN { printf "%.2f", b / 1073741824 }'
}

# The executable's name, from the start of a command line.
_mw_comm() {
    local first="${1%% *}"
    printf '%s' "${first##*/}"
}

# "600 bash <- 580 node <- 1 launchd": the chain from parent pid $1 up to launchd.
_mw_parent_chain() {
    local ppid="$1" chain="" cmd depth=0 row
    while [ -n "$ppid" ] && [ "$depth" -lt 30 ]; do
        row="$(_mw_proc_info "$ppid")"
        if [ -z "$row" ]; then
            chain="${chain:+$chain <- }$ppid (gone)"
            break
        fi
        cmd="$(printf '%s' "$row" | cut -f6)"
        chain="${chain:+$chain <- }$ppid $(_mw_comm "$cmd")"
        [ "$ppid" -gt 1 ] || break
        ppid="$(printf '%s' "$row" | cut -f2)"
        depth=$((depth + 1))
    done
    printf '%s' "$chain"
}

_mw_rotate() {
    local log="$1" size
    [ -f "$log" ] || return 0
    size="$(wc -c < "$log" | tr -d ' ')"
    if [ "$size" -gt $(($(_mw_log_max_kb) * 1024)) ]; then
        mv -f "$log" "$log.1"
    fi
}

_mw_record() {
    local pid="$1" bytes="$2" log_t="$3" ppid="$4" pgid="$5" uid="$6" lstart="$7" cmd="$8"
    echo "$(_mw_now) OVER pid=$pid footprint=$(_mw_gb "$bytes") GB threshold=$(_mw_gb "$log_t") GB"
    echo "  started: $lstart"
    echo "  ppid: $ppid  pgid: $pgid  uid: $uid"
    echo "  command: ${cmd:0:$MEMORY_WATCH_COMMAND_MAX}"
    echo "  cwd: $(_mw_cwd "$pid")"
    echo "  parents: $(_mw_parent_chain "$ppid")"
}

_mw_kill() {
    local pid="$1" uid="$2" bytes="$3" kill_t="$4" lstart="$5" now
    # `command -v`, because a standalone lib test does not source ports.sh.
    if command -v is_protected_host_pid >/dev/null 2>&1 && is_protected_host_pid "$pid"; then
        echo "$(_mw_now) not killed: pid $pid is protected"
        return 0
    fi
    if [ "$uid" != "$(id -u)" ]; then
        echo "$(_mw_now) not killed: pid $pid belongs to uid $uid"
        return 0
    fi
    # The tick's reading is a moment old, and the pid may have been reused since.
    if [ "$(_mw_proc_info "$pid" | cut -f5)" != "$lstart" ]; then
        echo "$(_mw_now) not killed: pid $pid is no longer the process that was measured"
        return 0
    fi
    now="$(_mw_top_pid "$pid" | _mw_parse_top | awk -v p="$pid" '$1 == p { print $2 }')"
    if [ -z "$now" ] || [ "$now" -lt "$kill_t" ]; then
        echo "$(_mw_now) not killed: pid $pid is no longer past the kill threshold"
        return 0
    fi
    bytes="$now"
    echo "$(_mw_now) KILL pid=$pid footprint=$(_mw_gb "$bytes") GB is past the $(_mw_gb "$kill_t") GB kill threshold"
    kill -TERM "$pid" 2>/dev/null || return 0
    _mw_wait 5
    # Same check before the SIGKILL: the pid may have exited and been reused.
    if [ "$(_mw_proc_info "$pid" | cut -f5)" = "$lstart" ]; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
    return 0
}

# ── one tick ────────────────────────────────────────────────────────────

memory_watch_once() {
    local dir log rows ram log_t kill_pct kill_t seen seen_new
    local pid bytes info ppid pgid uid lstart cmd key
    dir="$(_mw_dir)"
    mkdir -p "$dir" 2>/dev/null || return 0
    log="$dir/memory-watch.log"
    seen="$dir/seen"
    seen_new="$dir/seen.new"
    _mw_rotate "$log"

    rows="$(_mw_top | _mw_parse_top)"
    ram="$(_mw_physmem_bytes)"
    case "$ram" in '' | *[!0-9]*) rows="" ;; esac
    if [ -z "$rows" ]; then
        echo "$(_mw_now) note: top gave no reading, nothing was checked" >> "$log"
        return 0
    fi
    log_t=$((ram * $(_mw_log_pct) / 100))
    kill_pct="$(_mw_kill_pct)"
    kill_t=$((ram * kill_pct / 100))

    # The seen set is rewritten every tick, so it holds only what is over the
    # threshold now and cannot grow.
    : > "$seen_new"
    while read -r pid bytes <&3; do
        [ "$pid" -gt 1 ] || continue
        [ "$bytes" -ge "$log_t" ] || continue
        info="$(_mw_proc_info "$pid")"
        [ -n "$info" ] || continue
        IFS=$'\t' read -r _ ppid pgid uid lstart cmd <<EOF
$info
EOF
        key="$pid $lstart"
        printf '%s\n' "$key" >> "$seen_new"
        if grep -qxF -- "$key" "$seen" 2>/dev/null; then
            echo "$(_mw_now) STILL pid=$pid footprint=$(_mw_gb "$bytes") GB" >> "$log"
        else
            _mw_record "$pid" "$bytes" "$log_t" "$ppid" "$pgid" "$uid" "$lstart" "$cmd" >> "$log"
        fi
        if [ "$kill_pct" -gt 0 ] && [ "$bytes" -ge "$kill_t" ]; then
            _mw_kill "$pid" "$uid" "$bytes" "$kill_t" "$lstart" >> "$log"
        fi
    done 3<<EOF
$rows
EOF
    mv -f "$seen_new" "$seen"
}

# ── the launchd agent ───────────────────────────────────────────────────
# The agent runs the checkout's own scripts/memory-watch.sh, so it must never be
# installed from a coding-agent worktree. A worktree is thrown away when its
# session ends, and the agent would keep pointing at it (ADR 0021).

MEMORY_WATCH_LABEL="dev.lucidos.memory-watch"
MEMORY_WATCH_INTERVAL_S=30

_mw_launchctl() {
    launchctl "$@"
}

# $1 made safe inside an XML element.
# sed rather than ${v//&/...}: bash 5.2 reads a bare & in that replacement as
# the match, and bash 3.2 keeps quotes around it literally.
_mw_xml() {
    printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

# The agent's plist, for the checkout at $1. The knobs in force at install time
# travel in EnvironmentVariables, since launchd passes the agent nothing else.
memory_watch_plist() {
    local project dir
    project="$(_mw_xml "$1")"
    dir="$(_mw_xml "$(_mw_dir)")"
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$MEMORY_WATCH_LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>$project/scripts/memory-watch.sh</string>
        <string>--once</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>MEMORY_WATCH_DIR</key>
        <string>$dir</string>
        <key>MEMORY_WATCH_LOG_PCT</key>
        <string>$(_mw_log_pct)</string>
        <key>MEMORY_WATCH_KILL_PCT</key>
        <string>$(_mw_kill_pct)</string>
        <key>MEMORY_WATCH_LOG_MAX_KB</key>
        <string>$(_mw_log_max_kb)</string>
    </dict>
    <key>StartInterval</key>
    <integer>$MEMORY_WATCH_INTERVAL_S</integer>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$dir/launchd.log</string>
    <key>StandardErrorPath</key>
    <string>$dir/launchd.log</string>
</dict>
</plist>
PLIST
}

# Install or replace the agent for the checkout at $1, in the agents folder $2.
memory_watch_install() {
    local project="$1" agents="$2" plist uid
    case "$project" in
        */.lucidos/worktrees/*)
            echo "Refusing: $project is inside a coding-agent worktree, which is deleted when its session ends." >&2
            echo "Run this from your own checkout instead." >&2
            return 1
            ;;
    esac
    plist="$agents/$MEMORY_WATCH_LABEL.plist"
    uid="$(id -u)"
    mkdir -p "$agents" "$(_mw_dir)" || return 1
    _mw_launchctl bootout "gui/$uid/$MEMORY_WATCH_LABEL" >/dev/null 2>&1 || true
    # bootout can return before launchd has let go of the service, and a
    # bootstrap then fails. Wait up to five seconds for it to go.
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        _mw_launchctl print "gui/$uid/$MEMORY_WATCH_LABEL" >/dev/null 2>&1 || break
        _mw_wait 0.5
    done
    memory_watch_plist "$project" > "$plist" || return 1
    if ! _mw_launchctl bootstrap "gui/$uid" "$plist"; then
        echo "launchctl could not load $plist." >&2
        return 1
    fi
    echo "Installed $MEMORY_WATCH_LABEL: $project/scripts/memory-watch.sh every ${MEMORY_WATCH_INTERVAL_S}s."
    echo "Log: $(_mw_dir)/memory-watch.log"
}

memory_watch_uninstall() {
    local agents="$1"
    _mw_launchctl bootout "gui/$(id -u)/$MEMORY_WATCH_LABEL" >/dev/null 2>&1 || true
    rm -f "$agents/$MEMORY_WATCH_LABEL.plist"
    echo "Removed $MEMORY_WATCH_LABEL."
}

memory_watch_status() {
    local agents="$1" log
    log="$(_mw_dir)/memory-watch.log"
    if [ -f "$agents/$MEMORY_WATCH_LABEL.plist" ]; then
        echo "Installed: $agents/$MEMORY_WATCH_LABEL.plist"
    else
        echo "Not installed."
    fi
    if _mw_launchctl print "gui/$(id -u)/$MEMORY_WATCH_LABEL" >/dev/null 2>&1; then
        echo "Loaded in launchd."
    else
        echo "Not loaded in launchd."
    fi
    if [ -f "$log" ]; then
        echo "Last lines of $log:"
        tail -5 "$log"
    fi
}
