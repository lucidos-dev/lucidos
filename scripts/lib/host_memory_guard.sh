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
# The compressor is kept as a RUNAWAY BACKSTOP only, and is now a share of
# physical memory rather than a fixed byte count. A fixed 12 GB is 75% of a 16 GB
# Mac and 25% of a 48 GB one, so it could not mean the same thing on both.

# ── knobs ───────────────────────────────────────────────────────────────
#   LUCIDOS_E2E_SWAP_MAX_GB        swap in use that stops the run (default 1)
#   LUCIDOS_E2E_COMPRESSOR_MAX_PCT backstop as a share of RAM (default 50)
#   LUCIDOS_E2E_COMPRESSOR_MAX_GB  explicit absolute backstop, overrides the pct
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

# Echoes nothing when physical memory is unreadable and no absolute override is
# set, so the backstop is simply not applied. Swap still is.
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
    awk -v p="$phys" -v q="$pct" 'BEGIN { printf "%.2f", p * q / 100 }'
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

    # Swap first, because it is the condition that means the host is in trouble.
    # The compressor below only means it has been busy.
    if [ -n "$swap" ] && _host_mem_over "$swap" "$swap_max"; then
        HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
        MEMORY_STOP_DETAIL="At $where the host had $swap GB of swap in use, over the $swap_max GB limit."
        echo ""
        echo "[e2e-mem] STOP: $swap GB of swap is in use, over the $swap_max GB limit."
        echo "[e2e-mem] Compression has stopped keeping up, so the run is now"
        echo "[e2e-mem] costing the host. Stopping at this boundary."
        return 1
    fi

    if [ -n "$gb" ] && [ -n "$ceiling" ] && _host_mem_over "$gb" "$ceiling"; then
        HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
        MEMORY_STOP_DETAIL="At $where the compressor was $gb GB, over the $ceiling GB backstop."
        echo ""
        echo "[e2e-mem] STOP: compressor $gb GB is over the $ceiling GB backstop."
        echo "[e2e-mem] Swap is still clear, so this is the runaway backstop rather"
        echo "[e2e-mem] than measured distress. Stopping at this boundary."
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
    echo "[e2e-mem] stops at: swap over $swap_max GB, or compressor over ${ceiling:-no} GB."
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
