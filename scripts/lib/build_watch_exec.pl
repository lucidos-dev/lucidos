#!/usr/bin/perl
# build_watch_exec.pl: become the checkout's one build-watch (ADR 0438).
#
# Usage: perl build_watch_exec.pl <app-dir> <watch-lock> <pidfile> <start-fd>
#        <command> [args...]
#
# start_frontend_built runs this while it holds .build-watch/start.lock on
# <start-fd>, and this process inherits that descriptor. In order, it:
#
#   1. takes <watch-lock> exclusively, waiting out a dying watcher;
#   2. removes dist/, dist.staging/ and dist.prev/, which only the owner may do;
#   3. publishes its pid to <pidfile>, by rename;
#   4. closes its copy of start.lock, so the next launcher sees the watch held;
#   5. execs <command> with watch.lock still open, so the watcher holds it for
#      its whole life and the kernel drops it when the watcher dies.
use strict;
use warnings;
use Fcntl qw(:flock F_GETFD F_SETFD FD_CLOEXEC);
use File::Path qw(remove_tree);
use POSIX ();

my ($app, $lock, $pidfile, $start_fd, @cmd) = @ARGV;
die "usage: build_watch_exec.pl <app-dir> <watch-lock> <pidfile> <start-fd> <command> [args...]\n"
    unless defined $start_fd && @cmd;

open(my $watch, '>>', $lock) or die "build-watch: open $lock: $!\n";
flock($watch, LOCK_EX) or die "build-watch: lock $lock: $!\n";

remove_tree("$app/dist", "$app/dist.staging", "$app/dist.prev");

my $tmp = "$pidfile.$$.tmp";
open(my $out, '>', $tmp) or die "build-watch: write $tmp: $!\n";
print {$out} $$;
close $out or die "build-watch: write $tmp: $!\n";
rename($tmp, $pidfile) or die "build-watch: publish $pidfile: $!\n";

POSIX::close($start_fd);

# Perl opens files close-on-exec. The watcher must keep this one.
my $flags = fcntl($watch, F_GETFD, 0) or die "build-watch: fcntl: $!\n";
fcntl($watch, F_SETFD, $flags & ~FD_CLOEXEC) or die "build-watch: fcntl: $!\n";

exec { $cmd[0] } @cmd or die "build-watch: exec $cmd[0]: $!\n";
