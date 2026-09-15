#!/bin/bash
# Host-memory stop condition for the browser e2e phase.
#
# The browser projects, mobile-webkit above all, push this host hard. This guard
# reads it between chunks and stops the run at a clean boundary when the host is
# genuinely in danger. It is the sibling of host_load_guard.sh (CPU saturation,
# exit 75) and webkit_reaper.sh (per-process RSS), and it owns the one thing
# neither of those can see.
#
# WHAT IT MEASURES, AND WHY THAT CHANGED AGAIN.
#
# It used to stop on the size of the macOS VM compressor, against two ceilings
# scaled to RAM. That instrument is retired. A per-boundary attribution over a
# full nightly showed it measures something the run neither owns nor controls.
# Every number below comes from
# docs/plans/2026-09-07-e2e-memory-guard-stops-on-kernel-pressure.md.
#
# The compressor is a pool of physical RAM holding compressed ANONYMOUS PAGES,
# and those pages belong to whatever was idle when somebody needed room. Each
# chunk's fresh browser is a peak-demand excursion. It squeezes cold pages
# host-wide, then exits and frees its own. The squeezed pages stay compressed,
# because macOS never proactively decompresses and nothing faults on them
# overnight. So the pool integrates the run's excursions instead of measuring
# what the run holds. More chunks means more excursions, which is why chunk size
# never moved the curve.
#
# Measured: across 37 boundaries of one nightly, the summed phys_footprint of
# every process the run owns went 7.41 GB to 7.99 GB while the compressor went
# 4.86 GB to 17.11 GB. The e2e engine moved 11 MB. Postgres in the Docker VM did
# not move a byte. No Playwright browser reached any boundary's top eight, so
# the per-chunk fresh browser really does die.
#
# The pool also ignores demand. Eighty minutes after a clean teardown,
# allocating and touching 4 GB drove free RAM from 4.70 GB to 0.62 GB and the
# compressor did not move by a single page.
#
# WHAT REPLACES IT: THE KERNEL'S OWN VERDICT, ONCE IT HOLDS STILL.
#
# kern.memorystatus_vm_pressure_level is 1 normal, 2 warn, 4 critical. One
# sysctl, no root. It is what separated the one recorded freeze from every
# healthy night. At the freeze the host read compressor 17.41 GB, free 0.04 GB,
# pressure CRITICAL. A healthy nightly stop read compressor 17.11 GB, free
# 4.16 GB, pressure NORMAL. The compressor readings differ by 0.30 GB and
# nothing else about the two hosts is comparable.
#
# AN INSTANTANEOUS CRITICAL READING IS NOT THAT VERDICT. Ten samples of an idle
# host, 18 seconds apart, with nothing of ours running, read
# 2 2 4 4 2 4 4 2 4 4. Available memory stayed flat at 11 GB, the compressor did
# not move by a page, swap was 0.00 and load was falling. Six of ten read
# critical on a machine doing nothing. The level is a transient edge
# notification, raised as the compressed idle pool is probed, and it clears on
# its own. A single reading is therefore the same class of false positive as the
# retired compressor cap and the uncorroborated available floor.
#
# SO CRITICAL STOPS ON THREE GROUNDS, AND ON NOTHING ELSE.
#
#   1. COLLAPSE, immediately and unconditionally. Critical with available memory
#      at or under max(HOST_MEMORY_COLLAPSE_ABS_GB, a share of RAM), 2.40 GB
#      here. That is the freeze, which read 0.04 GB free. An idle host cannot
#      reach it: the lowest idle reading on record is 8.79 GB.
#   2. SWAP, immediately. Critical with any swap in use. Swap is accumulated
#      state and means compression stopped keeping up, so it needs no sustain.
#   3. SUSTAIN, otherwise. Critical still standing at the boundary, re-sampled
#      HOST_MEMORY_CRITICAL_CONFIRM_SAMPLES times at
#      HOST_MEMORY_CRITICAL_CONFIRM_SECS intervals, with EVERY sample critical.
#      The loop exits on the first sample that is not, so a healthy host pays a
#      few seconds rather than the whole window.
#
# UNANIMOUS, NOT A MAJORITY. Six of the ten idle samples above read critical, so
# a majority rule would still refuse a healthy host. Unanimity across 40 seconds
# is a claim that oscillation cannot make.
#
# THE AVAILABLE FLOOR IS NOT THE CORROBORATOR HERE, and the collapse level is.
# An idle host read 8.79 GB against the 9.60 GB floor, and the same host
# produces critical ticks, so that conjunction is reachable with nothing
# running. The runaway backstop is not a corroborator either, because it already
# stops on its own.
#
# WARN IS REPORTED, NEVER A STOP. Across 5749 host samples it occurred 92 times
# with no freeze, and it tracks host load as much as memory: one warn sample sat
# at load average 254. Stopping on it would trade one false positive for
# another.
#
# THE CHECK IS PEAK-AWARE, AND THE SUSTAIN ARM READS THE BOUNDARY INSTANT. A
# boundary sample bounds the reading at the boundary and says nothing about the
# chunk that just ran, whose observed deltas reach 1.16 GB. So a run-scoped
# sampler ticks every few seconds, and the boundary judges the WORST observation
# since the previous boundary. The sustain arm is the one exception: a critical
# that had cleared by the boundary is recorded and never acted on, because there
# is nothing live left to confirm. The collapse and swap arms still read the
# folded window, since critical with the headroom gone is the freeze whenever it
# happened. With no sampler the boundary falls back to one direct read, which is
# the old behaviour and never worse than it.
#
# THE OTHER TWO STOPS ARE UNCHANGED, and both measure real conditions.
#
# SWAP IN USE means compression stopped keeping up, and it is correct wherever
# swap exists. It is inert on the host this suite runs on today, which has no
# swapfile at all and reports 0.00 GB on every sample. That is precisely why the
# retired compressor cap was the only stop that ever fired here.
#
# The FREE-HEADROOM FLOOR is a CORROBORATED stop, and it used to be an absolute
# one. Available memory (free + speculative + purgeable + file-backed) below
# max(HOST_MEMORY_FREE_FLOOR_ABS_GB, a share of RAM) is now necessary and never
# sufficient. The floor still scales with the host, so it means the same danger
# on a 16 GB machine and a 48 GB one, and its 8 GB minimum still matches the
# pre-flight gate's AVAILABLE_MIN_GB.
#
# WHY THE FLOOR NEEDED CORROBORATING. It inherited the retired cap's defect
# through the same pool. `available` is depressed by the idle compressed pages,
# because each fresh browser squeezes cold pages host-wide and macOS never
# proactively decompresses. So the number reads low while the host is healthy.
# This host read 8.79 GB available at 09:00 with nothing running, under the
# 9.60 GB floor. A threshold a quiet idle Mac cannot clear is not measuring
# scarcity. The run is not holding it either: over one nightly the summed
# phys_footprint of every process the run owns went 7.41 GB to 7.99 GB.
#
# WHAT CORROBORATES: the kernel at warn or worse, or ANY swap in use. Both
# measure the host rather than the pool. Swap counts from the first byte, a
# lower bar than the 1 GB ceiling that stops on its own. Pressure counts at the
# boundary instant, or in two window samples, which is the sustain shape the
# floor already has. WARN ALONE IS STILL NEVER A STOP. What is new is the
# conjunction, and sustained sub-floor headroom under warn is far rarer than
# either half. When neither signal can be READ, the floor fails open and the
# boundary line says the corroboration was unavailable.
#
# STAYS BELOW, NOT DIPS BELOW (ADR 0176). The floor folds available with MIN,
# and the minimum of a noisy series falls as the window grows. So the boundary
# was judging the deepest 5-second trough of 50 to 120 samples. A fresh browser
# per chunk makes exactly such troughs. The floor needs the same sustain the
# critical-pressure stop needs: under it at the boundary instant, or under it in
# at least two samples of the window. A stop needs sustain AND corroboration.
#
# THE SWAP FOLD IS THE RIGHT DIRECTION and needs no sustain rule. It folds with
# MAX, so a longer window can only find a HIGHER reading. Swap in use is
# accumulated state rather than an instantaneous level. A sample over the limit
# means the host really did swap that much out, which a later sample does not
# undo.
#
# The compressor keeps ONE ceiling, the RUNAWAY BACKSTOP at 50% of RAM. It is
# neither a danger reading nor a survivability margin. It is a net for a host
# doing something nobody modelled. The recorded high-water mark on this machine
# is 25.98 GB, survived, so the backstop sits above anything a healthy run
# reaches.

# ── knobs ───────────────────────────────────────────────────────────────
#   LUCIDOS_E2E_SWAP_MAX_GB          swap in use that stops the run (default 1)
#   LUCIDOS_E2E_FREE_FLOOR_MIN_GB    absolute available-memory floor (default 8)
#   LUCIDOS_E2E_FREE_FLOOR_PCT       available floor as a share of RAM (default 20)
#   LUCIDOS_E2E_COLLAPSE_MIN_GB      absolute collapse level (default 2)
#   LUCIDOS_E2E_COLLAPSE_PCT         collapse level as a share of RAM (default 5)
#   LUCIDOS_E2E_CRITICAL_SAMPLES     confirm samples a sustained critical needs (default 8)
#   LUCIDOS_E2E_CRITICAL_SECS        seconds between confirm samples (default 5)
#   LUCIDOS_E2E_COMPRESSOR_MAX_PCT   runaway backstop as a share of RAM (default 50)
#   LUCIDOS_E2E_COMPRESSOR_MAX_GB    runaway backstop absolute; when set it wins
#   LUCIDOS_E2E_MEM_POLL_SECS        sampler tick in seconds (default 5)
#
# Four stops, in order: kernel pressure critical, swap distress, the corroborated
# headroom floor, the runaway backstop. The available floor is max(the MIN_GB,
# the PCT share of RAM): 9.6 GB on a 48 GB host, 8 GB on a 16 GB one. The
# collapse level is the same shape one order down: 2.40 GB on a 48 GB host,
# 2 GB on a 16 GB one. The 8 GB minimum matches the pre-flight gate's
# AVAILABLE_MIN_GB, and the gate carries the same corroboration rule, the same
# collapse level and the same confirm window, so the two never disagree. An
# explicit LUCIDOS_E2E_COMPRESSOR_MAX_GB replaces its share, because an operator
# naming a number means that number.
#
# LUCIDOS_E2E_COMPRESSOR_CAP_GB and _CAP_PCT are GONE, not renamed. A caller
# still setting one is passing a knob nothing reads, so the start report names
# it rather than letting the run look capped when it is not.
#
# Test seams, honored before any real host read:
#   HOST_COMPRESSOR_GB_OVERRIDE
#   HOST_AVAIL_GB_OVERRIDE
#   HOST_SWAP_USED_GB_OVERRIDE
#   HOST_PHYSMEM_GB_OVERRIDE
#   HOST_PRESSURE_LEVEL_OVERRIDE

# A memory stop must never read as a red project, so it carries its own exit
# code. 71 is sysexits' EX_OSERR: an OS resource condition, not a test verdict.
# It sits beside the host-load guard's 75 (EX_TEMPFAIL). Playwright exits 0, 1 or
# 130, so 71 can never collide with a Playwright code.
HOST_MEMORY_STOP_EXIT=71

# The free-headroom floor. A run stops when available memory stays below
# max(HOST_MEMORY_FREE_FLOOR_ABS_GB, the RAM share) AND a second signal agrees.
# The 8 GB minimum matches the pre-flight gate's AVAILABLE_MIN_GB so the two
# guards agree. The header says why a bare available reading is not scarcity,
# what corroborates it, and where the flat compressor cap went wrong.
HOST_MEMORY_FREE_FLOOR_ABS_GB=8
HOST_MEMORY_FREE_FLOOR_PCT=20

# The COLLAPSE level, the headroom that critical pressure needs beside it to stop
# the run at once. Same max(absolute, share of RAM) shape as the floor, one order
# down: 2.40 GB on a 48 GB host. The freeze read 0.04 GB free and the lowest
# reading an idle host has produced is 8.79 GB, so the level sits between the two
# with room on both sides. The pre-flight gate carries the same pair.
HOST_MEMORY_COLLAPSE_ABS_GB=2
HOST_MEMORY_COLLAPSE_PCT=5

# The sustain window a critical reading must survive when nothing corroborates
# it. Eight samples five seconds apart span 40 seconds. The confirm exits on the
# first sample that is not critical, so these are a ceiling rather than a cost.
# The pre-flight gate carries the same pair.
HOST_MEMORY_CRITICAL_CONFIRM_SAMPLES=8
HOST_MEMORY_CRITICAL_CONFIRM_SECS=5

# Kernel memory-pressure levels, as macOS reports them in
# kern.memorystatus_vm_pressure_level. Only CRITICAL stops a run; the header says
# why WARN is reported instead.
HOST_MEMORY_PRESSURE_NORMAL=1
HOST_MEMORY_PRESSURE_WARN=2
HOST_MEMORY_PRESSURE_CRITICAL=4

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

# Available memory in GB, from vm_stat: free + speculative + purgeable +
# file-backed pages. These are the pages macOS can reclaim for a new allocation
# without swapping, so together they are the headroom that a freeze exhausts. This
# is the SAME definition the pre-flight gate uses (data/knowhow/lucidos-ops/scripts/
# preflight-memory-gate.sh), so the start gate and the in-run floor agree; keep the
# two in step. The page size comes from vm_stat's own header (16 KB on Apple
# silicon, 4 KB on Intel), where the gate hardcodes 16 KB.
_host_mem_read_available_gb() {
    if [ -n "${HOST_AVAIL_GB_OVERRIDE:-}" ]; then
        printf '%s' "$HOST_AVAIL_GB_OVERRIDE"
        return 0
    fi
    vm_stat 2>/dev/null | awk '
        /page size of/ { for (i = 1; i < NF; i++) if ($i == "of") page = $(i + 1) + 0 }
        /^Pages free:/ { free = $NF + 0 }
        /^Pages speculative:/ { spec = $NF + 0 }
        /^Pages purgeable:/ { purge = $NF + 0 }
        /^File-backed pages:/ { filebacked = $NF + 0 }
        END {
            pages = free + spec + purge + filebacked
            if (page > 0 && pages > 0) printf "%.2f", pages * page / 1073741824
        }
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

# The kernel's own memory-pressure verdict: 1 normal, 2 warn, 4 critical. This is
# the reading that separated the recorded freeze from every healthy night, and it
# needs no root. Echoes nothing when the oid is missing or returns a non-level, so
# a host that cannot be judged is never stopped.
_host_mem_read_pressure_level() {
    if [ -n "${HOST_PRESSURE_LEVEL_OVERRIDE:-}" ]; then
        printf '%s' "$HOST_PRESSURE_LEVEL_OVERRIDE"
        return 0
    fi
    local level
    level="$(sysctl -n kern.memorystatus_vm_pressure_level 2>/dev/null)"
    case "$level" in
        "$HOST_MEMORY_PRESSURE_NORMAL" | "$HOST_MEMORY_PRESSURE_WARN" | "$HOST_MEMORY_PRESSURE_CRITICAL")
            printf '%s' "$level"
            ;;
    esac
}

# True when the level in $1 is warn or worse, which is what corroborates the
# available floor. EXACT matches only. A garbage value, an unknown one and an
# unreadable one all corroborate nothing, because a reading nobody can interpret
# is not evidence that the host is in trouble.
_host_mem_pressure_is_warn_or_worse() {
    case "$1" in
        "$HOST_MEMORY_PRESSURE_WARN" | "$HOST_MEMORY_PRESSURE_CRITICAL") return 0 ;;
    esac
    return 1
}

# The level as the word a human reads at 06:30. An unknown value is named as
# unknown rather than guessed at, because a wrong word here is how a log lies.
_host_mem_pressure_word() {
    case "$1" in
        "$HOST_MEMORY_PRESSURE_NORMAL") printf 'normal' ;;
        "$HOST_MEMORY_PRESSURE_WARN") printf 'warn' ;;
        "$HOST_MEMORY_PRESSURE_CRITICAL") printf 'critical' ;;
        '') printf 'unreadable' ;;
        *) printf 'unknown(%s)' "$1" ;;
    esac
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

# max(an absolute GB level, a share of physical RAM), which is the shape BOTH the
# available floor and the collapse level take. $1 and $2 are the resolved
# override values, $3 and $4 their defaults. An unusable override falls back to
# its default, and an unreadable RAM total leaves the absolute standing: a level
# in bytes needs no RAM total to compute, and neither level may lapse just
# because the share cannot be worked out.
_host_mem_scaled_level_gb() {
    local abs="$1" pct="$2" abs_default="$3" pct_default="$4"
    _host_mem_is_number "$abs" || abs="$abs_default"
    if ! _host_mem_is_number "$pct" ||
        ! awk -v p="$pct" 'BEGIN { exit (p + 0 > 0 && p + 0 <= 100) ? 0 : 1 }'; then
        pct="$pct_default"
    fi
    local phys
    phys="$(_host_mem_read_physical_gb)"
    if [ -z "$phys" ]; then
        printf '%s' "$abs"
        return 0
    fi
    awk -v a="$abs" -v p="$phys" -v q="$pct" \
        'BEGIN { share = p * q / 100; printf "%.2f", (share > a) ? share : a }'
}

# The available-memory floor, in GB. A run stops when available memory stays below
# it AND something corroborates. It scales with the host: 9.60 on a 48 GB machine,
# 8.00 on a 16 GB one.
_host_mem_available_floor_gb() {
    _host_mem_scaled_level_gb \
        "${LUCIDOS_E2E_FREE_FLOOR_MIN_GB:-$HOST_MEMORY_FREE_FLOOR_ABS_GB}" \
        "${LUCIDOS_E2E_FREE_FLOOR_PCT:-$HOST_MEMORY_FREE_FLOOR_PCT}" \
        "$HOST_MEMORY_FREE_FLOOR_ABS_GB" "$HOST_MEMORY_FREE_FLOOR_PCT"
}

# The collapse level, in GB. Critical pressure with available memory at or under
# it stops the run at once, with no sustain window. 2.40 on a 48 GB machine,
# 2.00 on a 16 GB one. It sits between the 0.04 GB the freeze read and the
# 8.79 GB an idle host has produced, so neither end is a coincidence.
_host_mem_collapse_level_gb() {
    _host_mem_scaled_level_gb \
        "${LUCIDOS_E2E_COLLAPSE_MIN_GB:-$HOST_MEMORY_COLLAPSE_ABS_GB}" \
        "${LUCIDOS_E2E_COLLAPSE_PCT:-$HOST_MEMORY_COLLAPSE_PCT}" \
        "$HOST_MEMORY_COLLAPSE_ABS_GB" "$HOST_MEMORY_COLLAPSE_PCT"
}

# The compressor RUNAWAY BACKSTOP, in GB. An explicit LUCIDOS_E2E_COMPRESSOR_MAX_GB
# wins outright: an operator who names a number means that number. Otherwise it is
# the MAX_PCT share of RAM, 24 on a 48 GB host. It is the ONLY compressor
# ceiling left, and it is not a danger reading: it catches a host doing something
# nobody modelled, above the 25.98 GB this machine has survived.
#
# Echoes nothing when physical memory is unreadable and no explicit override is
# set, so the compressor backstop is simply not applied. Swap and the available
# floor still are. A guard that cannot measure the host must not invent a ceiling.
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

# ── the critical-pressure sustain window ────────────────────────────────
# How many samples a critical reading must survive, and the seconds between
# them. Eight at five is a 40 s window. Each has a floor of its own: one sample
# is not a window, and a zero interval would take every reading of the same
# latched level in the same instant, which is the false positive this exists to
# remove. An unusable override falls back rather than disarming the rule.
_host_mem_critical_confirm_samples() {
    local n="${LUCIDOS_E2E_CRITICAL_SAMPLES:-$HOST_MEMORY_CRITICAL_CONFIRM_SAMPLES}"
    case "$n" in
        '' | *[!0-9]*) n="$HOST_MEMORY_CRITICAL_CONFIRM_SAMPLES" ;;
        *) [ "$((10#$n))" -ge 2 ] || n="$HOST_MEMORY_CRITICAL_CONFIRM_SAMPLES" ;;
    esac
    printf '%s' "$((10#$n))"
}

_host_mem_critical_confirm_secs() {
    local s="${LUCIDOS_E2E_CRITICAL_SECS:-$HOST_MEMORY_CRITICAL_CONFIRM_SECS}"
    if ! _host_mem_is_number "$s" ||
        ! awk -v v="$s" 'BEGIN { exit (v + 0 >= 1) ? 0 : 1 }'; then
        s="$HOST_MEMORY_CRITICAL_CONFIRM_SECS"
    fi
    printf '%s' "$s"
}

# The confirm loop's wait, as its own seam so a test can drive the loop without
# spending 40 seconds on it. Production has one behaviour and one caller. Same
# posture as the HOST_*_OVERRIDE reader seams above.
_host_mem_confirm_wait() {
    sleep "$1"
}

# Does CRITICAL pressure still stand after the sustain window? Re-reads the level
# up to `samples` times, `secs` apart, and answers 0 only when EVERY read was
# critical. It returns on the first read that is not, so a host whose level
# oscillates costs a few seconds rather than the whole window. An unreadable
# level ends the confirm without a stop, the fail-open posture every reader here
# has: a host nobody can judge is never stopped.
#
# Echoes "<taken> <of> <span_secs> <word>", where <word> names the level that
# ended it. The caller prints that whichever way the answer went, so a 06:30
# reader sees how long the kernel actually held critical.
_host_mem_critical_persists() {
    local total secs taken=0 level word=critical rc=0
    total="$(_host_mem_critical_confirm_samples)"
    secs="$(_host_mem_critical_confirm_secs)"
    while [ "$taken" -lt "$total" ]; do
        _host_mem_confirm_wait "$secs"
        level="$(_host_mem_read_pressure_level)"
        taken=$((taken + 1))
        if [ "$level" != "$HOST_MEMORY_PRESSURE_CRITICAL" ]; then
            word="$(_host_mem_pressure_word "$level")"
            rc=1
            break
        fi
    done
    printf '%s %s %s %s' "$taken" "$total" \
        "$(awk -v n="$taken" -v s="$secs" 'BEGIN { printf "%g", n * s }')" "$word"
    return "$rc"
}

# True when $1 is strictly greater than $2. Float math goes through awk, never
# bash arithmetic, which is integer only.
_host_mem_over() {
    awk -v a="$1" -v b="$2" 'BEGIN { exit (a + 0 > b + 0) ? 0 : 1 }'
}

# The worse of two readings of one dimension, by mode `max` or `min`. Absent is
# "" or "-", and an absent side never wins: a dimension one source could not read
# must not erase the side that read it.
_host_mem_worse_of() {
    local mode="$1" a="$2" b="$3"
    case "$a" in '' | '-') a="" ;; esac
    case "$b" in '' | '-') b="" ;; esac
    [ -n "$a" ] || {
        printf '%s' "$b"
        return 0
    }
    [ -n "$b" ] || {
        printf '%s' "$a"
        return 0
    }
    if [ "$mode" = "min" ]; then
        awk -v x="$a" -v y="$b" 'BEGIN { print (x + 0 < y + 0) ? x : y }'
    else
        awk -v x="$a" -v y="$b" 'BEGIN { print (x + 0 > y + 0) ? x : y }'
    fi
}

# ── the peak-aware sampler ──────────────────────────────────────────────
# A boundary reading bounds the host AT the boundary and says nothing about the
# chunk that just ran. Observed per-chunk compressor deltas reach 1.16 GB, so a
# mid-chunk excursion can come and go entirely between two boundary samples. The
# sampler closes that: it ticks while the chunk runs, and the boundary check
# folds every sample since the previous boundary into one worst-case reading.
#
# Same shape as host_load_guard.sh's sampler, deliberately: a disowned loop, an
# idempotent stop that reaps by recorded pid AND pidfile, and an orphan reap at
# start.
#
# The pidfile arm is what makes that "AND pidfile" possible, and it is also the
# one place this can signal a process it did not spawn: a SIGKILLed run leaves
# the file behind, and the pid in it can be recycled before the next run reads
# it. So the kill is gated on is_protected_host_pid (ADR 0025), which refuses
# this process, any ancestor of it, pid 1, and any pid a known engine pidfile
# claims. That narrows the window rather than closing it, which is the honest
# description: a recycled pid belonging to none of those is still reachable.
#
# It is a REFINEMENT, never a requirement. With no sampler, no samples or an
# unwritable file, the boundary falls back to one direct read, which is the
# behaviour this guard had before and is never worse than it.

# In-memory handle to the running loop (set by start_host_memory_sampler).
HOST_MEMORY_SAMPLER_PID="${HOST_MEMORY_SAMPLER_PID:-}"

_host_mem_samples_file() {
    printf '%s' "${HOST_MEMORY_SAMPLES_FILE:-${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.lucidos/host-memory-samples}"
}

_host_mem_sampler_pidfile() {
    printf '%s' "${HOST_MEMORY_SAMPLER_PIDFILE:-${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.lucidos/host-memory-sampler.pid}"
}

# One sample line per tick: "<level> <compressor> <available> <swap>". An
# unreadable dimension is written as "-", so a partial sample still contributes
# the dimensions it did read instead of being dropped whole.
_host_mem_sampler_loop() {
    local interval="$1" file="$2" level gb avail swap
    local sleep_pid=""
    # The sleep runs as a child so the trap can reach it. Without this the loop
    # ignores its own stop for up to a full interval, leaving an orphaned sleep
    # behind. Same shape as _host_load_sampler_loop, and the reason it is shaped
    # that way there.
    trap '[ -n "$sleep_pid" ] && kill "$sleep_pid" 2>/dev/null; exit 0' TERM INT
    while :; do
        level="$(_host_mem_read_pressure_level)"
        gb="$(_host_mem_read_compressor_gb)"
        avail="$(_host_mem_read_available_gb)"
        swap="$(_host_mem_read_swap_used_gb)"
        printf '%s %s %s %s\n' "${level:--}" "${gb:--}" "${avail:--}" "${swap:--}" >> "$file" 2>/dev/null || return 0
        sleep "$interval" &
        sleep_pid=$!
        wait "$sleep_pid" 2>/dev/null || true
        sleep_pid=""
    done
}

start_host_memory_sampler() {
    # Already sampling for THIS run: nothing to do, and do not reap our own loop
    # in the branch below.
    if [ -n "${HOST_MEMORY_SAMPLER_PID:-}" ] && kill -0 "$HOST_MEMORY_SAMPLER_PID" 2>/dev/null; then
        return 0
    fi

    # Reap a predecessor. The loop is disowned, so a SIGKILLed e2e-browser.sh
    # leaves it appending forever, and two samplers interleaving into one file
    # would make a boundary judge a host that never existed.
    stop_host_memory_sampler

    local file interval pidfile
    file="$(_host_mem_samples_file)"
    interval="${LUCIDOS_E2E_MEM_POLL_SECS:-5}"
    case "$interval" in '' | *[!0-9]* | 0) interval=5 ;; esac

    mkdir -p "$(dirname "$file")" 2>/dev/null || true
    if ! : > "$file" 2>/dev/null; then
        echo "[e2e-mem] cannot write ${file}, so boundary checks use a single reading this run"
        return 0
    fi

    _host_mem_sampler_loop "$interval" "$file" &
    HOST_MEMORY_SAMPLER_PID=$!
    disown "$HOST_MEMORY_SAMPLER_PID" 2>/dev/null || true

    pidfile="$(_host_mem_sampler_pidfile)"
    echo "$HOST_MEMORY_SAMPLER_PID" > "$pidfile" 2>/dev/null || true

    echo "[e2e-mem] peak sampler started (pid=${HOST_MEMORY_SAMPLER_PID}, every ${interval}s)"
}

# Idempotent, and safe in an EXIT trap. Kills ONLY the pid this run recorded, and
# takes the run-scoped samples file with it. Both are this run's, so neither may
# outlive it: a leftover window belongs to a host that no longer exists.
stop_host_memory_sampler() {
    local pid pidfile from_file=""
    pidfile="$(_host_mem_sampler_pidfile)"
    pid="${HOST_MEMORY_SAMPLER_PID:-}"
    if [ -z "$pid" ] && [ -f "$pidfile" ]; then
        pid="$(cat "$pidfile" 2>/dev/null)"
        from_file=1
    fi
    case "$pid" in '' | *[!0-9]*) pid="" ;; esac

    # A pid read off disk may have been recycled since a SIGKILLed run wrote it.
    # The in-memory handle cannot have been, so only the pidfile arm is gated.
    # `command -v`, because a standalone lib test does not source ports.sh.
    if [ -n "$from_file" ] && [ -n "$pid" ] &&
        command -v is_protected_host_pid >/dev/null 2>&1 &&
        is_protected_host_pid "$pid"; then
        echo "[e2e-mem] stale sampler pidfile names a protected pid ($pid), leaving it alone"
        pid=""
    fi

    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi

    rm -f "$pidfile" "$(_host_mem_samples_file)" 2>/dev/null || true
    HOST_MEMORY_SAMPLER_PID=""
}

# Fold every sample since the previous boundary into one worst-case reading, and
# echo "<level> <compressor> <available> <swap> <criticals> <warns> <under-floor>
# <count>". Worst means: the HIGHEST pressure level, the HIGHEST compressor, the
# LOWEST available, the HIGHEST swap. A dimension no sample could read comes back
# as "-" rather than empty, because an empty FIELD would shift every field after
# it when the caller splits the line. Echoes nothing with no samples at all.
#
# THE THREE COUNTS ARE WHY A WORST-CASE FOLD IS NOT A VERDICT ON ITS OWN. The
# caller needs each separately from its dimension's extreme. `<criticals>` is how
# many samples read CRITICAL pressure, and `<warns>` how many read WARN or worse,
# which is the floor's corroboration. `<under-floor>` is how many read available
# memory strictly under $1, the caller's floor, and it is 0 when no usable floor
# was passed. One 5-second blip on any of them is not the freeze signature, and
# treating it as one is the false stop this guard exists to remove.
#
# The available fold is the one that needs this most. It folds with MIN, so its
# value is the deepest instantaneous trough of the window and gets lower the
# longer the chunk ran. The count does not move with window length.
#
# A SHORT RECORD IS DISCARDED WHOLE. A truncated final line leaves `$3` empty,
# which is not the "-" sentinel, and `"" + 0` is 0. That zero can never be
# raised again on the `available` dimension, because that one folds with min, so
# one torn write would stop the run on a 0.00 GB scarcity reading that no sample
# ever took.
#
# It TRUNCATES the file, so each boundary judges its own chunk rather than the
# whole run. A tick landing between the read and the truncate is lost, which is
# at most one sample of a window that holds several, and the boundary takes its
# own direct reading at that same instant anyway.
#
# EVERY dimension needs its own `_seen` flag, and 0 cannot double as "unseen".
# Swap reads 0.00 on this host on every sample, and a freshly booted one reads a
# 0.00 compressor, so a plain `> max` comparison against an unset variable never
# fires and the dimension comes back "-" as if nothing could read it. The
# pressure level is the one exception: 0 is not a level macOS reports.
_host_mem_window_worst() {
    local floor_gb="${1:-}"
    _host_mem_is_number "$floor_gb" || floor_gb=""
    local file
    file="$(_host_mem_samples_file)"
    [ -s "$file" ] || return 0
    local window
    window="$(awk -v crit="$HOST_MEMORY_PRESSURE_CRITICAL" -v warn="$HOST_MEMORY_PRESSURE_WARN" \
        -v floor_gb="$floor_gb" '
        NF < 4 { next }
        { n++ }
        $1 != "-" && ($1 + 0) > lvl { lvl = $1 + 0 }
        $1 != "-" && ($1 + 0) == crit + 0 { ncrit++ }
        $1 != "-" && (($1 + 0) == warn + 0 || ($1 + 0) == crit + 0) { nwarn++ }
        $2 != "-" && (comp_seen == 0 || ($2 + 0) > comp) { comp = $2 + 0; comp_seen = 1 }
        $3 != "-" && (avail_seen == 0 || ($3 + 0) < avail) { avail = $3 + 0; avail_seen = 1 }
        $3 != "-" && floor_gb != "" && ($3 + 0) < (floor_gb + 0) { nunder++ }
        $4 != "-" && (swap_seen == 0 || ($4 + 0) > swap) { swap = $4 + 0; swap_seen = 1 }
        END {
            if (n == 0) exit 0
            printf "%s %s %s %s %d %d %d %d\n",
                (lvl > 0 ? sprintf("%d", lvl) : "-"),
                (comp_seen ? sprintf("%.2f", comp) : "-"),
                (avail_seen ? sprintf("%.2f", avail) : "-"),
                (swap_seen ? sprintf("%.2f", swap) : "-"),
                ncrit + 0,
                nwarn + 0,
                nunder + 0,
                n
        }' "$file")"
    : > "$file" 2>/dev/null || true
    [ -n "$window" ] || return 0
    printf '%s' "$window"
}

# ── per-process attribution ─────────────────────────────────────────────
# The global reading says HOW MUCH the compressor holds; it cannot say WHICH
# process holds it. Across a mobile-webkit run the compressor climbs to ~17 GB
# and nobody knows the owner. The browsers are reset every few specs, so the
# surviving suspects are the e2e engine and the coding-agent subprocesses the
# tests spawn. This block names them, every boundary, so a run log can be turned
# into a per-process time series afterwards.
#
# It is PURELY ADDITIVE OBSERVABILITY. It never changes a return value and never
# influences a stop, so a measurement failure can neither end a run nor mask a
# real stop. The tag below is distinct so the whole curve greps out of a log.
HOST_MEMORY_ATTR_TAG="[e2e-mem-attr]"

# How many processes to list, ranked by phys_footprint.
HOST_MEMORY_ATTR_TOP_N=8

# ── measurement seams (overridable in tests) ────────────────────────────
# Each is a thin wrapper over one host command so the test can feed known text
# without touching the real host, exactly like the vm_stat / sysctl seams above.

# One "pid<TAB>command" line per process THIS user owns. footprint can only
# measure a process the caller owns (footprint -a needs root), so the uid filter
# here is what keeps a root-owned pid out of the measured set. The command is the
# full executable path, so the caller can key on both basename and path.
_host_mem_attr_proc_list() {
    ps -axo pid=,uid=,comm= 2>/dev/null | awk -v u="$(id -u)" '
        $2 == u {
            pid = $1
            sub(/^[[:space:]]*[0-9]+[[:space:]]+[0-9]+[[:space:]]+/, "")
            printf "%s\t%s\n", pid, $0
        }'
}

# phys_footprint for the given pids, in ONE call, as raw bytes. footprint (not
# RSS) is the instrument because a page compressed out of a process's resident
# set has LEFT rss while still counting against the compressor this guard
# explains: measuring rss here once produced a bogus "20 GB belongs to no live
# process" finding. Echoes nothing when footprint is missing or refuses, so the
# caller degrades to a note.
_host_mem_attr_footprint() {
    [ "$#" -gt 0 ] || return 0
    command -v footprint >/dev/null 2>&1 || return 0
    local args=() p
    for p in "$@"; do args+=(-p "$p"); done
    footprint -f bytes "${args[@]}" 2>/dev/null
}

# A pid's current working directory, or empty. A KERNEL FACT, and the only safe
# way to tell an e2e-spawned coding agent from the user's OWN claude/codex
# session: both run the same binary, and a coding agent's command line quotes the
# e2e paths verbatim inside its ~22 KB system prompt, so nothing but the cwd can
# separate them. Same discriminator as e2e_lock.sh's _e2e_proc_cwd.
_host_mem_attr_cwd() {
    lsof -a -d cwd -p "$1" -Fn 2>/dev/null | sed -n 's/^n//p' | head -1
}

# Is $1 the directory $2, or anything beneath it? Whole-component prefix test, so
# a sibling like `<ws>-old` never matches `<ws>`. Trailing slashes come off the
# root first, the same care e2e_lock.sh's _e2e_path_under takes.
_host_mem_attr_path_under() {
    local path="$1" root="$2"
    while [ "${root%/}" != "$root" ] && [ -n "${root%/}" ]; do root="${root%/}"; done
    [ -n "$path" ] && [ -n "$root" ] || return 1
    case "$path" in
        "$root" | "$root"/*) return 0 ;;
    esac
    return 1
}

# Emit the attribution block for one boundary. Always returns 0. $1 is the
# boundary label the global line already carries. $2 is the INSTANTANEOUS
# compressor reading, never the folded window peak. The footprints below are
# instantaneous too, so only that reading compares honestly with them.
_host_mem_attr_report() {
    local where="$1" compressor_gb="$2"
    local tag="$HOST_MEMORY_ATTR_TAG"
    local topn="${HOST_MEMORY_ATTR_TOP_N:-8}"

    # The e2e engine is the primary suspect, keyed on the disposable workspace's
    # OWN pidfile so we never confuse it with the user's dev/personal engines.
    local ws engine_pid="" ws_real=""
    ws="${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}"
    engine_pid="$(cat "$ws/.lucidos/engine.pid" 2>/dev/null || true)"
    case "$engine_pid" in '' | *[!0-9]*) engine_pid="" ;; esac
    ws_real="$(cd "$ws" 2>/dev/null && pwd -P)" || ws_real=""

    # A browser belongs to the RUN when its executable path sits under the
    # Playwright browsers cache. A system Safari or the user's own Chrome does
    # not. Same discriminator webkit_reaper.sh and e2e_lock.sh key on, and it
    # survives orphaning: a run browser re-parented to launchd still runs from the
    # cache, which ancestry would misread as host. Honors PLAYWRIGHT_BROWSERS_PATH;
    # resolved once, so the per-process test below forks nothing.
    local pw_cache="${PLAYWRIGHT_BROWSERS_PATH:-ms-playwright}"
    pw_cache="${pw_cache%/}"

    # Classify every owned process into a candidate kind, in one pass. Membership
    # in the RUN is decided by FACT, never by name: a coding agent by its cwd, a
    # browser by whether its executable path is under the Playwright cache. So the
    # user's own claude/codex (cwd elsewhere), a system Safari, and a stray node
    # are told apart from the run's own. A non-run browser or node is kept as
    # visible host demand (browser-host / node-host), never silently dropped:
    # losing it would hide real memory from the attributed-vs-compressor line.
    local meta_lines="" pids="" pid comm base kind cwd
    while IFS=$'\t' read -r pid comm; do
        [ -n "$pid" ] || continue
        base="${comm##*/}"
        kind=""
        case "$base" in
            lucidos-engine)
                kind="engine"
                ;;
            claude | codex)
                cwd="$(_host_mem_attr_cwd "$pid")"
                if _host_mem_attr_path_under "$cwd" "$ws" ||
                    _host_mem_attr_path_under "$cwd" "$ws_real"; then
                    kind="agent"
                fi
                ;;
            node | lucidos)
                cwd="$(_host_mem_attr_cwd "$pid")"
                if _host_mem_attr_path_under "$cwd" "$ws" ||
                    _host_mem_attr_path_under "$cwd" "$ws_real"; then
                    kind="agent"
                elif [ "$base" = "node" ]; then
                    # A node with no cwd under the run is the user's own editor,
                    # dev server or tooling, not a process the run started. Host
                    # demand, kept visible rather than counted as the run.
                    kind="node-host"
                fi
                ;;
            *)
                case "$comm" in
                    *WebKit* | *WebContent* | *hromium* | *[Ff]irefox* | *headless* | *Playwright*)
                        # A run browser runs from the Playwright cache; a system
                        # Safari or the user's own Chrome of the same name does
                        # not. Keep the run ones as browser; the rest are visible
                        # host demand.
                        case "$comm" in
                            *"$pw_cache"*) kind="browser" ;;
                            *) kind="browser-host" ;;
                        esac
                        ;;
                    *Virtualization.VirtualMachine*)
                        kind="docker-vm" ;;
                esac
                ;;
        esac
        [ -n "$kind" ] || continue
        meta_lines="${meta_lines}META"$'\t'"${pid}"$'\t'"${kind}"$'\t'"${base}"$'\n'
        pids="${pids} ${pid}"
    done <<EOF
$(_host_mem_attr_proc_list)
EOF

    # One footprint call over the candidates, joined with their kind/name. The
    # header line "<name> [<pid>]: 64-bit    Footprint: <N> B" carries the
    # per-process footprint; the "Shared with …" and de-dup "Summary" lines are
    # excluded by requiring BOTH "]:" and "Footprint:".
    local analysis fp_out
    # shellcheck disable=SC2086 # deliberate word-split: $pids is a space-separated pid list
    fp_out="$(_host_mem_attr_footprint $pids)"
    analysis="$( { printf '%s' "$meta_lines"; printf '%s\n' "$fp_out"; } | awk -F'\t' -v epid="$engine_pid" '
        $1 == "META" { kind[$2] = $3; name[$2] = $4; next }
        (index($0, "]:") > 0 && index($0, "Footprint:") > 0) {
            m = index($0, "]:")
            pre = substr($0, 1, m - 1)
            tmp = pre; off = 0
            while ((k = index(tmp, "[")) > 0) { off += k; tmp = substr(tmp, k + 1) }
            pid = substr(pre, off + 1)
            fpos = index($0, "Footprint: ")
            rest = substr($0, fpos + length("Footprint: "))
            sp = index(rest, " ")
            b = substr(rest, 1, sp - 1)
            if (pid ~ /^[0-9]+$/ && b ~ /^[0-9]+$/) bytes[pid] = b + 0
            next
        }
        END {
            attributed = 0; nprocs = 0; agent_bytes = 0; agent_count = 0
            for (p in kind) {
                if (!(p in bytes)) continue
                b = bytes[p]; attributed += b; nprocs++
                iseng = (epid != "" && p == epid) ? 1 : 0
                if (kind[p] == "agent") { agent_bytes += b; agent_count++ }
                printf "PROC\t%d\t%s\t%s\t%d\t%s\n", b, p, kind[p], iseng, name[p]
            }
            eng_present = (epid != "" && (epid in kind)) ? 1 : 0
            eng_measured = (epid != "" && (epid in bytes)) ? 1 : 0
            eng_bytes = eng_measured ? bytes[epid] : -1
            printf "AGG\t%d\t%d\t%d\t%d\n", attributed, nprocs, agent_bytes, agent_count
            printf "ENG\t%d\t%d\t%d\n", eng_present, eng_measured, eng_bytes
        }
    ')"

    local attributed_bytes nprocs agent_bytes agent_count eng_present eng_measured eng_bytes
    attributed_bytes="$(printf '%s\n' "$analysis" | awk -F'\t' '$1=="AGG"{print $2; exit}')"
    nprocs="$(printf '%s\n' "$analysis" | awk -F'\t' '$1=="AGG"{print $3; exit}')"
    agent_bytes="$(printf '%s\n' "$analysis" | awk -F'\t' '$1=="AGG"{print $4; exit}')"
    agent_count="$(printf '%s\n' "$analysis" | awk -F'\t' '$1=="AGG"{print $5; exit}')"
    eng_present="$(printf '%s\n' "$analysis" | awk -F'\t' '$1=="ENG"{print $2; exit}')"
    eng_measured="$(printf '%s\n' "$analysis" | awk -F'\t' '$1=="ENG"{print $3; exit}')"
    eng_bytes="$(printf '%s\n' "$analysis" | awk -F'\t' '$1=="ENG"{print $4; exit}')"

    # A measurement that pinned nothing degrades to a single note and carries on.
    # Missing footprint, a root refusal, or no candidates all land here.
    if [ "${nprocs:-0}" -eq 0 ]; then
        echo "$tag after $where: no candidate footprint measured (footprint missing, refused, or no candidates); attribution skipped, run continues"
        return 0
    fi

    # Top N by phys_footprint. The e2e engine's line is tagged so it stands out.
    local rank=0 b p k iseng nm mb
    printf '%s\n' "$analysis" | awk -F'\t' '$1=="PROC"{print}' |
        sort -t"$(printf '\t')" -k2 -nr | head -n "$topn" |
        while IFS=$'\t' read -r _ b p k iseng nm; do
            rank=$((rank + 1))
            mb="$(awk -v v="$b" 'BEGIN { printf "%.0f", v / 1048576 }')"
            if [ "$iseng" = "1" ]; then
                echo "$tag after $where: #$rank $nm pid=$p ${mb} MB ($k)  <-- e2e engine"
            else
                echo "$tag after $where: #$rank $nm pid=$p ${mb} MB ($k)"
            fi
        done

    # The e2e engine, always, whether or not it made the top N: its ABSENCE from
    # a top-N list is itself a finding, so its line must never depend on rank.
    if [ -n "$engine_pid" ]; then
        if [ "${eng_measured:-0}" = "1" ]; then
            mb="$(awk -v v="$eng_bytes" 'BEGIN { printf "%.0f", v / 1048576 }')"
            echo "$tag after $where: e2e engine pid=$engine_pid ${mb} MB (primary suspect)"
        elif [ "${eng_present:-0}" = "1" ]; then
            echo "$tag after $where: e2e engine pid=$engine_pid was a candidate but footprint could not be read"
        else
            echo "$tag after $where: e2e engine pid=$engine_pid not among measured candidates (it may have exited or been unreadable)"
        fi
    else
        echo "$tag after $where: e2e engine pidfile absent or unreadable at $ws/.lucidos/engine.pid"
    fi

    # Coding-agent subprocesses in aggregate: the tests spawn real ones, at least
    # one outlives its spec by ~45s, and many small ones would hide in a top-N
    # list while mattering as a sum.
    mb="$(awk -v v="${agent_bytes:-0}" 'BEGIN { printf "%.0f", v / 1048576 }')"
    echo "$tag after $where: coding-agent subprocesses: ${agent_count:-0} procs, ${mb} MB total (cwd under e2e workspace)"

    # The attributed sum against the compressor. The gap is the point of the whole
    # block: it says whether the suspects account for the compressor or whether
    # the accumulator is somewhere these candidates never looked.
    #
    # SAY WHICH COMPRESSOR READING THIS IS. The global line just above may carry
    # the folded window peak. A first real run put 6.87 GB there and 5.02 GB
    # here, one line apart, both called "compressor". A reader at 06:30 sees a
    # contradiction rather than two honest readings of different windows.
    local att_gb gap
    att_gb="$(awk -v v="${attributed_bytes:-0}" 'BEGIN { printf "%.2f", v / 1073741824 }')"
    if [ -n "$compressor_gb" ] && _host_mem_is_number "$compressor_gb"; then
        gap="$(awk -v c="$compressor_gb" -v a="$att_gb" 'BEGIN { printf "%.2f", c - a }')"
        echo "$tag after $where: attributed $att_gb GB across ${nprocs} procs (phys_footprint) vs the instantaneous compressor $compressor_gb GB, gap $gap GB"
    else
        echo "$tag after $where: attributed $att_gb GB across ${nprocs} procs (phys_footprint); the instantaneous compressor was unreadable, no gap"
    fi
    return 0
}

# ── the guard ───────────────────────────────────────────────────────────

# Report the host at one chunk boundary. Returns non-zero when the run must stop,
# and records the reason for the final verdict. Every boundary prints a line, so
# an unattended run leaves the whole curve in its log rather than only the point
# where it stopped.
check_host_memory_at_boundary() {
    local where="$1"
    local gb avail swap level level_now gb_now avail_now ceiling floor swap_max
    level="$(_host_mem_read_pressure_level)"
    gb="$(_host_mem_read_compressor_gb)"
    avail="$(_host_mem_read_available_gb)"
    swap="$(_host_mem_read_swap_used_gb)"
    ceiling="$(_host_mem_compressor_ceiling_gb)"
    floor="$(_host_mem_available_floor_gb)"
    swap_max="$(_host_mem_swap_ceiling_gb)"
    # The readings AT the boundary, kept before the fold overwrites them. Three
    # things need the instant rather than the window: the sustained-critical rule
    # below, the sustained-floor rule beside it, and the attribution block. That
    # block's per-process footprints are instantaneous, so a mid-chunk peak
    # cannot honestly be compared against them.
    level_now="$level"
    gb_now="$gb"
    avail_now="$avail"

    # Fold the sampler's window over the instantaneous reading, worst wins per
    # dimension. Folding rather than replacing is what makes the sampler purely
    # additive: the boundary is never blinder than a direct read, and the window
    # can only make it stricter. `<<EOF` rather than `<<<`, which bash 3.2 has
    # but the repo's other libs avoid for the same portability reason.
    local window w_level w_gb w_avail w_swap w_crit w_warn w_under w_n
    local samples="" crit_seen=0 warn_seen=0 under_seen=0
    window="$(_host_mem_window_worst "$floor")"
    if [ -n "$window" ]; then
        read -r w_level w_gb w_avail w_swap w_crit w_warn w_under w_n <<EOF
$window
EOF
        level="$(_host_mem_worse_of max "$level" "$w_level")"
        gb="$(_host_mem_worse_of max "$gb" "$w_gb")"
        avail="$(_host_mem_worse_of min "$avail" "$w_avail")"
        swap="$(_host_mem_worse_of max "$swap" "$w_swap")"
        samples="$w_n"
        case "$w_crit" in '' | *[!0-9]*) w_crit=0 ;; esac
        crit_seen="$w_crit"
        case "$w_warn" in '' | *[!0-9]*) w_warn=0 ;; esac
        warn_seen="$w_warn"
        case "$w_under" in '' | *[!0-9]*) w_under=0 ;; esac
        under_seen="$w_under"
    fi

    if [ -z "$gb" ] && [ -z "$avail" ] && [ -z "$swap" ] && [ -z "$level" ]; then
        echo "[e2e-mem] after $where: host memory unreadable, check skipped"
        return 0
    fi
    local scope="single reading"
    [ -n "$samples" ] && scope="worst of $samples samples"
    echo "[e2e-mem] after $where: pressure $(_host_mem_pressure_word "$level"), compressor ${gb:-?} GB, available ${avail:-?} GB, swap ${swap:-?} GB ($scope)"

    # Per-process attribution: which process holds the compressor. Purely additive,
    # so it runs AFTER the global reading and BEFORE any stop decision, and its
    # result is discarded (|| true) because a failed attribution must never change
    # the stop verdict below.
    _host_mem_attr_report "$where" "$gb_now" || true

    # Four stops, and they mean different things about the host. The wording says
    # which one fired, because this line is what somebody reads at 06:30 to decide
    # whether the Mac was in trouble or the run merely got greedy.
    #
    # Kernel pressure first: it is the kernel's own verdict, and the reading that
    # distinguished the recorded freeze from a healthy night. Swap second,
    # because it means compression stopped keeping up. Available memory third,
    # because it is the headroom a freeze exhausts. The runaway backstop last,
    # and it is not a danger reading.

    # CRITICAL NEEDS A SECOND FACT BEFORE IT IS BELIEVED, and the header carries
    # the evidence: an idle host read critical in six of ten samples while
    # available memory was flat at 11 GB and swap was zero. Three arms, cheapest
    # first, so a host in real trouble never waits out a window it does not need.
    local crit_now="" collapse confirm=""
    if [ -n "$level_now" ] && [ "$level_now" = "$HOST_MEMORY_PRESSURE_CRITICAL" ]; then
        crit_now=1
    fi
    if [ -n "$crit_now" ] || [ "${crit_seen:-0}" -ge 1 ]; then
        # Resolved here rather than beside the floor and the ceiling above,
        # because only this branch reads it and it costs a sysctl. Most
        # boundaries never see a critical reading at all.
        collapse="$(_host_mem_collapse_level_gb)"
        # 1. The freeze signature, folded over the window, because a freeze
        # counts whenever in the chunk it happened. Unconditional and immediate.
        if [ -n "$avail" ] && [ -n "$collapse" ] && ! _host_mem_over "$avail" "$collapse"; then
            HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
            MEMORY_STOP_DETAIL="At $where the kernel reported CRITICAL memory pressure with available memory at $avail GB, at or under the $collapse GB collapse level. That is the signature of the one freeze this host has had."
            echo ""
            echo "[e2e-mem] STOP: the kernel reports CRITICAL memory pressure and available"
            echo "[e2e-mem] memory is $avail GB, at or under the $collapse GB collapse level."
            echo "[e2e-mem] This is the FREEZE SIGNATURE, and it is unconditional. The one"
            echo "[e2e-mem] recorded freeze on this host read critical pressure with 0.04 GB"
            echo "[e2e-mem] free. Critical alone oscillates here while memory is flat, so it"
            echo "[e2e-mem] waits out a sustain window. Critical with the headroom gone does"
            echo "[e2e-mem] not wait. The run must stop at this boundary."
            return 1
        fi
        # 2. Swap corroborates from the first byte and needs no sustain rule: it
        # is accumulated state rather than an instantaneous level.
        if [ -n "$swap" ] && _host_mem_over "$swap" 0; then
            HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
            MEMORY_STOP_DETAIL="At $where the kernel reported CRITICAL memory pressure with $swap GB of swap in use. Two instruments agreed the host was in trouble."
            echo ""
            echo "[e2e-mem] STOP: the kernel reports CRITICAL memory pressure and $swap GB of"
            echo "[e2e-mem] swap is in use."
            echo "[e2e-mem] This is CORROBORATED CRITICAL PRESSURE. Swap means compression"
            echo "[e2e-mem] stopped keeping up, so a second instrument agrees with the kernel."
            echo "[e2e-mem] It is accumulated state rather than an instantaneous level, so it"
            echo "[e2e-mem] needs no sustain window. The run must stop at this boundary."
            return 1
        fi
        # 3. Nothing corroborates, so the level itself has to hold. Only a
        # critical STANDING at the boundary is worth confirming: one that cleared
        # during the chunk leaves nothing live to re-sample.
        if [ -n "$crit_now" ]; then
            # The confirm's answer is its EXIT STATUS, and its line is the
            # evidence both branches print. So the status is captured first and
            # the line parsed once, rather than in each branch.
            local c_taken c_of c_span c_word held=""
            if confirm="$(_host_mem_critical_persists)"; then
                held=1
            fi
            read -r c_taken c_of c_span c_word <<EOF
$confirm
EOF
            if [ -n "$held" ]; then
                HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
                MEMORY_STOP_DETAIL="At $where the kernel reported CRITICAL memory pressure and held it through all $c_of confirm samples over ${c_span}s, with available ${avail:-?} GB and swap ${swap:-?} GB."
                echo ""
                echo "[e2e-mem] STOP: the kernel reports CRITICAL memory pressure, and it HELD."
                echo "[e2e-mem] This is the KERNEL'S OWN VERDICT, not a proxy, and it was"
                echo "[e2e-mem] re-sampled before it was believed: $c_taken of $c_of samples over"
                echo "[e2e-mem] ${c_span}s all read critical. A single reading oscillates on this"
                echo "[e2e-mem] host while memory is flat, so only a sustained one is evidence."
                echo "[e2e-mem] The run must stop at this boundary."
                return 1
            fi
            echo "[e2e-mem] note: the kernel read CRITICAL at this boundary and did not hold it. Sample $c_taken of $c_of, ${c_span}s in, read $c_word."
            echo "[e2e-mem] Recorded, not a stop: an instantaneous critical reading is a transient edge notification on this host, seen in 6 of 10 samples of an idle machine. Available ${avail:-?} GB, swap ${swap:-?} GB."
            echo "[e2e-mem] A stop needs critical to persist through the whole window, or available at or under ${collapse:-no} GB, or swap in use."
        else
            echo "[e2e-mem] note: the kernel read CRITICAL in ${crit_seen:-0} of ${samples:-1} samples during the chunk, and read $(_host_mem_pressure_word "$level_now") at the boundary itself."
            echo "[e2e-mem] Recorded, not a stop: the excursion had cleared by the boundary, so there was nothing left to confirm. Available ${avail:-?} GB, swap ${swap:-?} GB."
        fi
    fi

    # WARN is stated and not acted on. It has occurred 92 times in three months of
    # host samples with no freeze, and it tracks host load as much as memory, so
    # stopping on it would swap one false positive for another.
    if [ -n "$level" ] && [ "$level" = "$HOST_MEMORY_PRESSURE_WARN" ]; then
        echo "[e2e-mem] note: the kernel reports WARN pressure. Recorded, not a stop: only CRITICAL is."
    fi

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

    # THE FLOOR NEEDS SUSTAIN AND CORROBORATION, and they are different claims.
    #
    # SUSTAIN says the dip was not a 5-second trough. Available is the one
    # dimension the window folds with MIN, so the folded value is the deepest
    # instantaneous trough and it falls further the longer the chunk ran. A fresh
    # browser per chunk produces exactly such troughs as normal operation. Same
    # two arms as the pressure rule: still under the floor at the boundary itself,
    # or under it in at least two samples.
    #
    # CORROBORATION says something other than the available number agrees the
    # host is in trouble. A low reading alone measures the idle compressed-page
    # pool, which a quiet host also produces, so it is necessary and never
    # sufficient. The header carries the evidence.
    #
    # A survived breach always logs the same measurement line, then one reason
    # line per missing half. So a 06:30 reader gets the window minimum, the
    # sample count and the boundary reading whichever half declined the stop,
    # and reads WHICH one declined right underneath them.
    if [ -n "$avail" ] && [ -n "$floor" ] && _host_mem_over "$floor" "$avail"; then
        local sustained="" corroborated_by="" swap_word="unreadable"
        [ -n "$swap" ] && swap_word="$swap GB"
        if { [ -n "$avail_now" ] && _host_mem_over "$floor" "$avail_now"; } ||
            [ "${under_seen:-0}" -ge 2 ]; then
            sustained=1
        fi
        # Swap from the first byte, which is a lower bar than the ceiling that
        # stops on its own. It folds with max, so no sustain rule applies.
        if [ -n "$swap" ] && _host_mem_over "$swap" 0; then
            corroborated_by="$swap GB of swap in use"
        fi
        if _host_mem_pressure_is_warn_or_worse "$level_now"; then
            corroborated_by="${corroborated_by:+$corroborated_by and }the kernel at $(_host_mem_pressure_word "$level_now") pressure"
        elif [ "${warn_seen:-0}" -ge 2 ]; then
            corroborated_by="${corroborated_by:+$corroborated_by and }the kernel at warn or worse in $warn_seen of ${samples:-1} samples"
        fi
        if [ -n "$sustained" ] && [ -n "$corroborated_by" ]; then
            HOST_MEMORY_STOP_COMPRESSOR_GB="$gb"
            MEMORY_STOP_DETAIL="At $where available memory was $avail GB, under the $floor GB floor, corroborated by $corroborated_by. The host was genuinely low on memory."
            echo ""
            echo "[e2e-mem] STOP: available memory $avail GB is under the $floor GB floor, corroborated by $corroborated_by."
            echo "[e2e-mem] This is CORROBORATED SCARCITY. Free, speculative, purgeable and"
            echo "[e2e-mem] file-backed pages are the headroom a new allocation draws on. That"
            echo "[e2e-mem] reading alone also falls on an idle host, so it is not scarcity by"
            echo "[e2e-mem] itself. Here it is sustained AND a signal that measures the host"
            echo "[e2e-mem] rather than the compressed-page pool agrees. The run must stop."
            return 1
        fi
        echo "[e2e-mem] note: available dipped to $avail GB, under the $floor GB floor, in ${under_seen:-0} of ${samples:-1} samples. It read ${avail_now:-?} GB at the boundary itself."
        if [ -z "$sustained" ]; then
            echo "[e2e-mem] Recorded, not a stop: it is not sustained. The floor needs the boundary reading, or two samples under it. One 5-second trough is a browser launch, not host scarcity."
        fi
        if [ -z "$corroborated_by" ]; then
            echo "[e2e-mem] Recorded, not a stop: it is uncorroborated. Pressure $(_host_mem_pressure_word "$level_now") at the boundary, warn or worse in ${warn_seen:-0} of ${samples:-1} samples, swap $swap_word."
            echo "[e2e-mem] A low available reading on its own measures the idle compressed-page pool, which an idle host also produces. The floor needs the kernel at warn or worse, or swap in use."
        fi
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
    local gb avail swap level ceiling floor swap_max collapse c_of c_secs
    level="$(_host_mem_read_pressure_level)"
    gb="$(_host_mem_read_compressor_gb)"
    avail="$(_host_mem_read_available_gb)"
    swap="$(_host_mem_read_swap_used_gb)"
    ceiling="$(_host_mem_compressor_ceiling_gb)"
    floor="$(_host_mem_available_floor_gb)"
    collapse="$(_host_mem_collapse_level_gb)"
    swap_max="$(_host_mem_swap_ceiling_gb)"
    c_of="$(_host_mem_critical_confirm_samples)"
    c_secs="$(_host_mem_critical_confirm_secs)"
    HOST_MEMORY_BASELINE_GB="$gb"

    if [ -z "$gb" ]; then
        echo "[e2e-mem] browser phase start: compressor unreadable, pressure $(_host_mem_pressure_word "$level")"
    else
        echo "[e2e-mem] browser phase start: pressure $(_host_mem_pressure_word "$level"), compressor $gb GB, available ${avail:-?} GB, swap ${swap:-?} GB"
    fi
    echo "[e2e-mem] stops at: kernel pressure critical (the freeze signature), swap over $swap_max GB (distress), available under ${floor:-no} GB WITH a corroborating signal (corroborated scarcity), or compressor over ${ceiling:-no} GB (runaway backstop)."
    echo "[e2e-mem] Critical pressure stops the run when it PERSISTS through $c_of re-samples ${c_secs}s apart, or with available at or under ${collapse:-no} GB, or with any swap in use. One critical reading on its own is recorded and never stops the run: it oscillates here while memory is flat."
    echo "[e2e-mem] The corroborating signals are the kernel at warn or worse, or any swap in use. Available under the floor on its own is recorded and never stops the run."
    echo "[e2e-mem] The compressor has no survivability cap: it measures squeezed idle pages host-wide, not what this run holds."

    # A retired knob that is still set would otherwise leave the run looking
    # capped when nothing reads the value. Naming it is cheaper than a silent
    # surprise at 06:30.
    if [ -n "${LUCIDOS_E2E_COMPRESSOR_CAP_GB:-}${LUCIDOS_E2E_COMPRESSOR_CAP_PCT:-}" ]; then
        echo "[e2e-mem] NOTE: LUCIDOS_E2E_COMPRESSOR_CAP_GB / _CAP_PCT is set and is no longer read. Unset it."
    fi
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
    echo "[e2e-mem] To finish only what was lost, rerun with LUCIDOS_E2E_WEBKIT_CHUNKS=<first>-<last>."
    echo "[e2e-mem] Add LUCIDOS_E2E_WEBKIT_PHASE=nav when the loss is in nav: the"
    echo "[e2e-mem] range narrows nav only, and the CC phase it leaves whole owns"
    echo "[e2e-mem] 93 to 97 percent of the memory a discharge costs."
}
