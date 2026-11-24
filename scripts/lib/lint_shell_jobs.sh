#!/usr/bin/env bash
#
# lint_shell_jobs.sh: how many ShellCheck processes scripts/lint-shell.sh runs
# at once. Sourced by that script and by lint_shell_test.sh. It defines
# functions, and makes `ps` usable inside an agent's sandbox.
#
# The heaviest script takes one ShellCheck process to 2.6 GB, and eight scripts
# pass 1 GB. They are few, so six jobs at once peaked at 7 GB together. Each
# job is granted LINT_SHELL_GB_PER_JOB of spare memory, which covers that.
# Cores cap the count, and so does LINT_SHELL_MAX_JOBS.
#
# A ShellCheck takes most of its memory at the end of its run, so available
# memory alone hides what the running ones will still take. Spare memory is
# therefore what is available, less the unclaimed grant of every ShellCheck on
# the host. Sessions see each other through the process list.

# shellcheck source=scripts/lib/host_memory_guard.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/host_memory_guard.sh"
# shellcheck source=scripts/lib/host_load_guard.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/host_load_guard.sh"
# shellcheck source=scripts/lib/ps_tools.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/ps_tools.sh"
ps_tools_env

LINT_SHELL_GB_PER_JOB=2
LINT_SHELL_MAX_JOBS=8

# A further batch starts only while this much is spare. It is above the
# heaviest process's peak.
LINT_SHELL_GB_TO_START=3

# Every process on this host, as `<resident-kb> <parent-pid> <executable>`
# lines.
_lint_shell_process_list() {
    ps -Aww -o rss= -o ppid= -o comm=
}

# Print the spare memory in GB, given <available-gb> and a process list on
# stdin. <own-batch-pids> names the batches the caller has started, space
# separated. Each one counts once, whether or not the list shows its
# ShellCheck yet. Every other ShellCheck counts once too. Prints nothing when
# an input cannot be read.
lint_shell_spare_gb_for() { # <available-gb> <own-batch-pids>
    awk -v avail="$1" -v own="$2" -v per="$LINT_SHELL_GB_PER_JOB" '
        BEGIN {
            owned = split(own, pids, " ")
            for (i = 1; i <= owned; i++) {
                if (pids[i] !~ /^[0-9]+$/) unreadable = 1
                mine[pids[i]] = 1
            }
        }
        NF >= 3 && $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ {
            listed = 1
            rss_gb = $1 / 1048576
            parent = $2
            sub(/^[ \t]*[0-9]+[ \t]+[0-9]+[ \t]+/, "")
            sub(/[ \t]+$/, "")
            sub(/.*\//, "")
            if ($0 != "shellcheck") next
            if (!(parent in mine)) others++
            held += (rss_gb < per ? rss_gb : per)
        }
        END {
            if (!listed || unreadable || avail !~ /^[0-9]+(\.[0-9]+)?$/) exit
            printf "%.2f\n", avail - ((others + owned) * per - held)
        }'
}

# The spare memory of this host now, in GB. A process list that fails part
# way is not read at all.
lint_shell_spare_gb() { # <own-batch-pids>
    local list
    list="$(_lint_shell_process_list 2> /dev/null)" || return 0
    printf '%s\n' "$list" | lint_shell_spare_gb_for "$(_host_mem_read_available_gb)" "$1"
}

# Jobs for a host with <ncpu> cores and <spare-gb> of memory. A value that
# cannot be read counts as too little, so the answer is one job.
lint_shell_jobs_for() { # <ncpu> <spare-gb>
    awk -v ncpu="$1" -v spare="$2" -v per="$LINT_SHELL_GB_PER_JOB" -v max="$LINT_SHELL_MAX_JOBS" 'BEGIN {
        if (ncpu !~ /^[0-9]+$/ || spare !~ /^[0-9]+(\.[0-9]+)?$/) { print 1; exit }
        jobs = int(spare / per)
        if (jobs > ncpu) jobs = ncpu
        if (jobs > max) jobs = max
        if (jobs < 1) jobs = 1
        print jobs
    }'
}

# Jobs for this host, from its core count and its spare memory now.
lint_shell_jobs() {
    local spare
    spare="$(lint_shell_spare_gb "")"
    [ -n "$spare" ] || echo "lint-shell: cannot read spare memory, so ShellCheck runs one batch at a time." >&2
    lint_shell_jobs_for "$(_host_load_read_ncpu)" "$spare"
}

# Whether <spare-gb> leaves room to start one more batch. A value that cannot
# be read, or is below zero, counts as no room.
lint_shell_room_for() { # <spare-gb>
    awk -v spare="$1" -v need="$LINT_SHELL_GB_TO_START" 'BEGIN {
        exit !(spare ~ /^[0-9]+(\.[0-9]+)?$/ && spare + 0 >= need)
    }'
}

# Whether this host has room to start one more batch beside the caller's own.
lint_shell_has_room() { # <own-batch-pids>
    lint_shell_room_for "$(lint_shell_spare_gb "$1")"
}
