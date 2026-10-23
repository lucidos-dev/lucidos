#!/bin/bash
# flock_test_support.sh: FIFO channels and a lock holder process for tests.
#
# A holder is a perl process this test spawned. It takes a lock, optionally
# publishes its own pid, reports ready, then holds until released. Every pid it
# yields is the test's own child, so the ADR 0025 kill shims stay satisfied.
#
# Synchronization is by FIFO, never by sleeping. Each FIFO is opened read-write
# once, up front. A blocking open of a FIFO is what bash 3.2 abandons when a
# SIGCHLD interrupts it, and an abandoned reader leaves its writer stuck.

_HOLDER_SEQ=0

# Create a FIFO and hold it open read-write on <fd>. Later opens of it, from any
# process, never block. Read with `read -r line <&<fd>`.
channel_open() { # <fd> <path>
    mkfifo "$2"
    eval "exec $1<>\"\$2\""
}

channel_close() { # <fd>
    eval "exec $1>&-"
}

# Start a holder of <lock> in mode <sh|ex>, and return once it holds the lock.
# Sets HOLDER_PID and HOLDER_RELEASE. <pidfile>, when given, gets its pid.
start_lock_holder() { # <scratch-dir> <lock> <sh|ex> [pidfile]
    local dir="$1" lock="$2" mode="$3" pidfile="${4:-}" ready
    _HOLDER_SEQ=$((_HOLDER_SEQ + 1))
    ready="$dir/holder-$_HOLDER_SEQ.ready"
    HOLDER_RELEASE="$dir/holder-$_HOLDER_SEQ.release"
    channel_open 4 "$ready"
    mkfifo "$HOLDER_RELEASE"
    perl -MFcntl=:flock -e '
        my ($lock, $mode, $pidfile, $ready, $release) = @ARGV;
        open(my $g, "+<", $release) or die "open $release: $!";
        open(my $fh, ">>", $lock) or die "open $lock: $!";
        flock($fh, $mode eq "sh" ? LOCK_SH : LOCK_EX) or die "flock: $!";
        if ($pidfile ne "") {
            open(my $p, ">", $pidfile) or die "open $pidfile: $!";
            print $p $$;
            close $p;
        }
        open(my $r, ">", $ready) or die;
        print $r "locked\n";
        close $r;
        my $line = <$g>;
    ' "$lock" "$mode" "$pidfile" "$ready" "$HOLDER_RELEASE" 4>&- &
    # shellcheck disable=SC2034 # read by the sourcing test
    HOLDER_PID=$!
    local _
    read -r _ <&4
    channel_close 4
}

# Release a holder and reap it. The holder opened its release FIFO read-write
# before it reported ready, so this open never blocks.
release_lock_holder() { # <pid> <release-fifo>
    echo go > "$2"
    wait "$1" 2>/dev/null || true
}
