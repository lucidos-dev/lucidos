#!/bin/bash
# Host-memory stop condition for the browser e2e phase.
#
# The browser projects, mobile-webkit above all, push the macOS VM compressor
# hard. This guard reads the host between chunks and stops the run at a clean
# boundary when the host is genuinely out of memory to lend. It is the sibling of
# host_load_guard.sh (CPU saturation, exit 75) and webkit_reaper.sh (per-process
# RSS), and it owns the one thing neither of those can see.
#
# WHAT IT MEASURES, AND WHY THAT CHANGED.
#
# The stop condition used to be one number: compressor size over a fixed 12 GB.
# That is a broken instrument, and the unfiltered mobile-webkit project is what it
# broke. Compressor size measures how much idle memory macOS has squeezed, which
# tracks total host demand rather than danger. It is also cumulative across every
# process on the box and does not reset between runs, so a run was charged for
# whatever the host had already compressed before it started.
#
# The consequence was structural, not bad luck. A full unfiltered pass costs
# roughly 8 GB of compressor growth against a 12 GB absolute ceiling, so it
# completed only from a host under about 4 GB and was cut short otherwise. The
# growth rate is not even constant: two identical passes grew 0.22 and 0.46 GB per
# chunk, because the second ran on a busier host.

# A filtered slice never reached the check at all, since only the unfiltered path
# chunks. So every spec passed in slices while the unfiltered invocation produced
# exit 71 and no verdict, which is exactly the shape the nightly reported.
#
# So the ceiling is no longer the stop condition. SWAP IN USE is. macOS compresses
# before it swaps, so swap means compression has stopped keeping up, and that is
# the point where a run starts costing the host rather than the host lending to
# the run. The pile-up recorded in the e2e_lock.sh header reached 23.5 GB
# compressed AND 14 GB of swap; swap is the half that says it went wrong.
#
# The compressor is kept as a RUNAWAY BACKSTOP only, and it resolves to the LOWER
# of two ceilings. Both are ceilings, so the tighter one is the only one worth
# applying.
#
# The share of physical memory keeps a SMALL host honest. A fixed 12 GB is 75% of
# a 16 GB Mac and 25% of a 48 GB one, so one byte count cannot mean the same thing
# on both.
#
# The absolute cap keeps a LARGE host honest, and that half was missing. Half of
# 48 GB is 24 GB, which no run here has come near, so on this machine the share
# alone could never fire and the only ceiling in force was whatever the caller
# happened to export. The cap is HOST_MEMORY_COMPRESSOR_CAP_GB, 16. It is not a
# round number picked for comfort: 17.41 GB is the compressor reading that
# hard-froze this host on 2026-07-26, so 16 leaves a real margin under a reading
# already paid for. Raising it spends that margin.

# ── knobs ───────────────────────────────────────────────────────────────
#   LUCIDOS_E2E_SWAP_MAX_GB        swap in use that stops the run (default 1)
#   LUCIDOS_E2E_COMPRESSOR_MAX_GB  absolute backstop; when set it wins outright
#   LUCIDOS_E2E_COMPRESSOR_MAX_PCT share of RAM (default 50), CAPPED, see below
#
# Unset, the backstop is the lower of HOST_MEMORY_COMPRESSOR_CAP_GB and that
# share. An explicit LUCIDOS_E2E_COMPRESSOR_MAX_GB replaces BOTH, because an
# operator naming a number means that number.
#
# The PCT knob is NOT an escape from the cap. It moves the share, and the lower
# of the two still wins, so on a 48 GB host any value over 33 is a no-op. That is
# deliberate: the cap is calibrated to a freeze reading rather than to a ratio, so
# a percentage cannot argue it away. Name a number in GB to raise the ceiling.
#
# Test seams, honored before any real host read:
#   HOST_COMPRESSOR_GB_OVERRIDE
#   HOST_SWAP_USED_GB_OVERRIDE
#   HOST_PHYSMEM_GB_OVERRIDE

# A memory stop must never read as a red project, so it carries its own exit
# code. 71 is sysexits' EX_OSERR: an OS resource condition, not a test verdict.
# It sits beside the host-load guard's 75 (EX_TEMPFAIL). Playwright exits 0, 1 or
# 130, so 71 can never collide with a Playwright code.
HOST_MEMORY_STOP_EXIT=71

# The absolute half of the compressor backstop, in GB. The header says where 16
# comes from and what raising it costs. The RAM share is the other half, and the
# lower of the two is what a boundary is judged against.
HOST_MEMORY_COMPRESSOR_CAP_GB=16

# Set to the project name at the boundary that tripped a stop. The phase split
# and the project loop both read it. Neither starts more work on a host we have
# already refused to load further.
MEMORY_STOPPED=""
MEMORY_STOP_DETAIL=""

# Compressor reading at the browser phase start, and at the boundary that
# stopped the run. The final report subtracts them, so the log states what this
# run itself cost instead of asking the reader to do it.
HOST_MEMORY_BASELINE_GB=""
HOST_MEMORY_STOP_COMPRESSOR_GB=""

# ── measurement seams (overridable in tests) ────────────────────────────
# Every reader echoes nothing when it cannot measure, and the caller then fails
# open. A guard that cannot measure must never stop the suite. Same posture as
# the host-load guard and the reaper.

# Compressor size in GB, from vm_stat. The page size comes from vm_stat's own
# header rather than a constant, because it is 16 KB on Apple silicon and 4 KB
# on Intel.
_host_mem_read_compressor_gb() {
    if [ -n "${HOST_COMPRESSOR_GB_OVERRIDE:-}" ]; then
        printf '%s' "$HOST_COMPRESSOR_GB_OVERRIDE"
        return 0
    fi
    vm_stat 2>/dev/null | awk '
        /page size of/ { for (i = 1; i < NF; i++) if ($i == "of") page = $(i + 1) + 0 }
        /^Pages occupied by compressor:/ { pages = $NF + 0 }
        END { if (page > 0 && pages > 0) printf "%.2f", pages * page / 1073741824 }
    '
}

# Swap currently in use, in GB, from `sysctl vm.swapusage`. The value carries its
# own unit suffix (0.00M, 2.50G), so the unit is read off the number rather than
# assumed. A host that has never swapped reports 0.00M with no swapfile on disk,
# which is not the same as swap being unavailable: macOS grows swapfiles on
# demand, and the pile-up in the e2e_lock.sh header reached 14 GB of them.
_host_mem_read_swap_used_gb() {
    if [ -n "${HOST_SWAP_USED_GB_OVERRIDE:-}" ]; then
        printf '%s' "$HOST_SWAP_USED_GB_OVERRIDE"
        return 0
    fi
    sysctl -n vm.swapusage 2>/dev/null | awk '
        {
            for (i = 1; i <= NF; i++) {
                if ($i != "used") continue
                raw = $(i + 2)
                unit = raw
                sub(/^[0-9.]+/, "", unit)
                sub(/[A-Za-z]+$/, "", raw)
                if (unit == "G")      gb = raw + 0
                else if (unit == "M") gb = (raw + 0) / 1024
                else if (unit == "K") gb = (raw + 0) / 1048576
                else                  gb = (raw + 0) / 1073741824
                printf "%.2f", gb
                exit
            }
        }
    '
}

# Physical memory in GB, which is what the backstop is a share of.
_host_mem_read_physical_gb() {
    if [ -n "${HOST_PHYSMEM_GB_OVERRIDE:-}" ]; then
        printf '%s' "$HOST_PHYSMEM_GB_OVERRIDE"
        return 0
    fi
    local bytes
    bytes="$(sysctl -n hw.memsize 2>/dev/null)"
    case "$bytes" in ''|*[!0-9]*) return 0 ;; esac
    awk -v b="$bytes" 'BEGIN { printf "%.2f", b / 1073741824 }'
}

# ── thresholds ──────────────────────────────────────────────────────────
# Every knob falls back to its default when the override is not a usable number.
# A garbage threshold would otherwise stop the suite at the first boundary, which
# is the failure this whole file exists to remove.

# True when $1 is a plain non-negative number: digits and at most one dot.
# Numeric coercion cannot do this job on its own. awk reads `banana` as 0, and 0
# is a legitimate swap ceiling, so the two are indistinguishable by value alone.
# The garbage one then prints into the stop message as the limit it broke.
_host_mem_is_number() {
    case "$1" in
        '' | '.' | *[!0-9.]* | *.*.*) return 1 ;;
    esac
    return 0
}

_host_mem_swap_ceiling_gb() {
    local max="${LUCIDOS_E2E_SWAP_MAX_GB:-1}"
    _host_mem_is_number "$max" || max=1
    printf '%s' "$max"
}

# The backstop, in GB. An explicit LUCIDOS_E2E_COMPRESSOR_MAX_GB wins outright:
# an operator who names a number means that number, cap included.
#
# Echoes nothing when physical memory is unreadable and no absolute override is
# set, so the backstop is simply not applied. Swap still is. The cap alone would
# be a defensible answer there, but a guard that cannot measure the host must not
# start inventing thresholds for it.
_host_mem_compressor_ceiling_gb() {
    local explicit="${LUCIDOS_E2E_COMPRESSOR_MAX_GB:-}"
    if _host_mem_is_number "$explicit" && awk -v m="$explicit" 'BEGIN { exit (m + 0 > 0) ? 0 : 1 }'; then
        printf '%s' "$explicit"
        return 0
    fi
    local pct="${LUCIDOS_E2E_COMPRESSOR_MAX_PCT:-50}"
    if ! _host_mem_is_number "$pct" ||
        ! awk -v p="$pct" 'BEGIN { exit (p + 0 > 0 && p + 0 <= 100) ? 0 : 1 }'; then
        pct=50
    fi
    local phys
    phys="$(_host_mem_read_physical_gb)"
    [ -n "$phys" ] || return 0
    # The lower of the share and the cap. On a 48 GB host the cap bites first, at
    # 16.00 against a 24.00 share; on a 16 GB one the share still bites first, at
    # 8.00. That is the whole point of taking a minimum rather than picking one.
    awk -v p="$phys" -v q="$pct" -v cap="$HOST_MEMORY_COMPRESSOR_CAP_GB" \
        'BEGIN { share = p * q / 100; printf "%.2f", (share < cap) ? share : cap }'
}

# True when $1 is strictly greater than $2. Float math goes through awk, never
# bash arithmetic, which is integer only.
_host_mem_over() {
    awk -v a="$1" -v b="$2" 'BEGIN { exit (a + 0 > b + 0) ? 0 : 1 }'
}

# ── the guard ───────────────────────────────────────────────────────────

# Report the host at one chunk boundary. Returns non-zero when the run must stop,
# and records the reason for the final verdict. Every boundary prints a line, so
# an unattended run leaves the whole curve in its log rather than only the point
# where it stopped.
check_host_memory_at_boundary() {
    local where="$1"
    local gb swap ceiling swap_max
    gb="$(_host_mem_read_compressor_gb)"
    swap="$(_host_mem_read_swap_used_gb)"
    ceiling="$(_host_mem_compressor_ceiling_gb)"
    swap_max="$(_host_mem_swap_ceiling_gb)"

    if [ -z "$gb" ] && [ -z "$swap" ]; then
        echo "[e2e-mem] after $where: host memory unreadable, check skipped"
        return 0
    fi
    echo "[e2e-mem] after $where: compressor ${gb:-?} GB, swap ${swap:-?} GB"

    # Two stops, and they mean opposite things about the host. The wording says
    # which one fired, because this line is what somebody reads at 06:30 to
    # decide whether the Mac was in trouble or the run merely got greedy.
    #
    # Swap first, because it is the condition that means the host is in trouble.
    # The compressor below only means it has been busy.
    if [ -n "$swap" ] && _host_mem_over "$swap" "$swap_max"; then
        HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
        MEMORY_STOP_DETAIL="At $where the host had $swap GB of swap in use, over the $swap_max GB limit. That is measured distress."
        echo ""
        echo "[e2e-mem] STOP: $swap GB of swap is in use, over the $swap_max GB limit."
        echo "[e2e-mem] This is MEASURED DISTRESS. macOS compresses before it swaps, so"
        echo "[e2e-mem] swap means compression stopped keeping up and the host is in"
        echo "[e2e-mem] real trouble. The run must stop at this boundary."
        return 1
    fi

    if [ -n "$gb" ] && [ -n "$ceiling" ] && _host_mem_over "$gb" "$ceiling"; then
        HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
        echo ""
        echo "[e2e-mem] STOP: compressor $gb GB is over the $ceiling GB backstop."
        # "Not distress" is a claim about SWAP, so it may only be made when swap
        # was actually read. The compressor is readable on its own (vm_stat), and
        # a failing `sysctl vm.swapusage` leaves it empty, so this branch is
        # reachable and the confident wording would be a lie in it.
        if [ -n "$swap" ]; then
            MEMORY_STOP_DETAIL="At $where the compressor was $gb GB, over the $ceiling GB backstop, with swap at $swap GB. The host was not in distress."
            echo "[e2e-mem] This is the RUNAWAY BACKSTOP, NOT distress. Swap is still clear at"
            echo "[e2e-mem] $swap GB, under its $swap_max GB limit, so the host was never in"
            echo "[e2e-mem] trouble. The run is stopped for growing further than a run should"
            echo "[e2e-mem] need to, not because the Mac was struggling."
        else
            MEMORY_STOP_DETAIL="At $where the compressor was $gb GB, over the $ceiling GB backstop. Swap was unreadable, so distress could not be ruled out."
            echo "[e2e-mem] This is the RUNAWAY BACKSTOP, but swap was UNREADABLE, so distress"
            echo "[e2e-mem] cannot be ruled out here. Read the host yourself before deciding"
            echo "[e2e-mem] the Mac was fine."
        fi
        return 1
    fi
    return 0
}

# Print the host state without judging it, once, before any browser work, and
# record the compressor baseline. Stating the thresholds here is what lets an
# unattended log be read without also knowing the machine's RAM.
report_host_memory_start() {
    local gb swap ceiling swap_max
    gb="$(_host_mem_read_compressor_gb)"
    swap="$(_host_mem_read_swap_used_gb)"
    ceiling="$(_host_mem_compressor_ceiling_gb)"
    swap_max="$(_host_mem_swap_ceiling_gb)"
    HOST_MEMORY_BASELINE_GB="$gb"

    if [ -z "$gb" ]; then
        echo "[e2e-mem] browser phase start: compressor unreadable"
    else
        echo "[e2e-mem] browser phase start: compressor $gb GB, swap ${swap:-?} GB"
    fi
    echo "[e2e-mem] stops at: swap over $swap_max GB (distress), or compressor over ${ceiling:-no} GB (backstop)."
}

# Final verdict for a run a stop cut short. finish calls it, so every exit path
# says this once, last, where an unattended reader looks.
report_memory_stop() {
    [ -n "$MEMORY_STOPPED" ] || return 0
    echo ""
    echo "[e2e-mem] STOPPED ON HOST MEMORY during $MEMORY_STOPPED."
    echo "[e2e-mem] $MEMORY_STOP_DETAIL"
    if [ -n "$HOST_MEMORY_BASELINE_GB" ] && [ -n "$HOST_MEMORY_STOP_COMPRESSOR_GB" ]; then
        echo "[e2e-mem] This run grew the compressor by $(awk -v a="$HOST_MEMORY_STOP_COMPRESSOR_GB" \
            -v b="$HOST_MEMORY_BASELINE_GB" 'BEGIN { printf "%.2f", a - b }') GB,"
        echo "[e2e-mem] from a $HOST_MEMORY_BASELINE_GB GB baseline."
    fi
    echo "[e2e-mem] Coverage is incomplete: work after that point did not run."
    echo "[e2e-mem] Exit $HOST_MEMORY_STOP_EXIT marks a memory stop, never a failing test."
    echo "[e2e-mem] Free memory on the host and rerun. Chunk size cannot help:"
    echo "[e2e-mem] LUCIDOS_E2E_WEBKIT_CHUNK bounds the per-chunk delta, not the total."
}
