#!/usr/bin/env bash
#
# proc_tree.sh: walk a process tree by parent pid, never by command line
# (ADR 0025). Sourced by host_memory_guard.sh and harden-suites.sh.

# Read "pid ppid" lines on stdin and print $1 and its descendants, root first,
# one per line. The visited set keeps a malformed feed with a cycle from
# looping. Callers pass the feed, so a test can hand in a synthetic one.
proc_tree_walk() { # <root-pid>
    awk -v root="$1" '
        { kids[$2] = kids[$2] " " $1 }
        END {
            queue = root
            while (queue != "") {
                n = split(queue, q, " ")
                queue = ""
                for (i = 1; i <= n; i++) {
                    if (q[i] in seen) continue
                    seen[q[i]] = 1
                    print q[i]
                    m = split(kids[q[i]], k, " ")
                    for (j = 1; j <= m; j++) queue = queue " " k[j]
                }
            }
        }'
}
