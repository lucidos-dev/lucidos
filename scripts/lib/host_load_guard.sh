# shellcheck shell=bash
# Host-load backpressure guard — refuse to launch heavy e2e work onto a saturated
# host, and back off / abort cleanly instead of wedging the machine.
#
# Overnight 2026-07-01 an EXTERNAL macOS daemon (triald → mobileassetd
# purgeable-CacheDelete loop, misfiring at targetingPurgeAmount:0 with 549 GB
# free — a known Tahoe daemon bug, NOT ours) pinned an 18-core box at load ~96.
# The nightly e2e step then launched its Playwright browser swarm (WebKit +
# Chromium) ON TOP of the already-pegged host; the browsers wedged ("failed
# localhost commit"), the machine became unresponsive, and it had to be
# hard-rebooted. We cannot fix the daemon — but the e2e tooling piled heavy work
# on unconditionally, with no host-load guard. This is that guard.
#
# It samples the 1-minute load average, computes load1/ncpu, and:
#   ratio ≤ cap  → return 0 immediately (a healthy host pays one sample).
#   ratio > cap  → wait-and-back-off, polling until the ratio drops under cap
#                  (return 0) OR a total wait cap is exceeded, at which point it
#                  returns a DISTINCT saturated exit code (75) so the nightly
#                  orchestrator can tell a backpressure abort from a test failure.
#
# It is a companion to the two existing host-resource safety nets:
#   scripts/lib/e2e_lock.sh      — single-writer lock (no concurrent/orphan runs)
#   scripts/lib/webkit_reaper.sh — reaps a single over-RSS browser process
# Neither measures SYSTEM LOAD; this one does.
#
# Knobs (all optional; sane defaults):
#   HOST_LOAD_MAX_RATIO       load1/ncpu ratio we tolerate before backing off
#                             (default 1.5 — load up to 1.5× core count is fine)
#   HOST_LOAD_POLL_SECS       seconds between polls while over-ratio (default 15)
#   HOST_LOAD_MAX_WAIT_SECS   total seconds to wait before refusing (default 300)
#   HOST_LOAD_GUARD_DISABLE   =1 → no-op that logs it's disabled and returns 0
#                             (escape hatch for CI where load is meaningless)
#   HOST_LOAD_OVERRIDE        test hook: force the 1-min load reading
#   HOST_NCPU_OVERRIDE        test hook: force the core count
#
# Exit code: HOST_LOAD_SATURATED_EXIT (75, the EX_TEMPFAIL sysexits convention) —
# returned by wait_for_host_load when the host is still saturated after the wait
# cap. Distinguishable from an ordinary test failure (1).
#
# Fail-open by design: if the guard cannot MEASURE the host (unknown OS,
# unreadable load, ncpu empty/zero/non-numeric) it logs and returns 0. A guard
# that can't measure must never block or crash the suite — the same posture as the
# reaper's "no ps → warn and don't start".
#
# Sourced by scripts/lib/e2e.sh; invoked by scripts/e2e-browser.sh right before
# the Playwright browser swarm spawns.

# Distinct exit code for "host still saturated, refusing to launch". Overridable
# for tests, but 75 (EX_TEMPFAIL) is the contract the nightly orchestrator keys on.
: "${HOST_LOAD_SATURATED_EXIT:=75}"

# ── measurement seams (overridable in tests) ───────────────────────────────
# Both readers honor a test override env var FIRST, then read the real host.
# Being functions, they can also be redefined wholesale by a test after sourcing
# (the repo convention) — needed for the "recovers mid-wait" case, where the
# reading must change across successive polls.

# _host_load_read_load1 — echo the 1-minute load average (may be a float). Echoes
# nothing when it can't read (caller then fails open).
_host_load_read_load1() {
    if [ -n "${HOST_LOAD_OVERRIDE:-}" ]; then
        printf '%s' "$HOST_LOAD_OVERRIDE"
        return 0
    fi
    case "$(uname -s 2>/dev/null)" in
        Darwin)
            # `sysctl -n vm.loadavg` → "{ 1.85 1.90 2.01 }"; field 2 is the 1-min avg.
            sysctl -n vm.loadavg 2>/dev/null | awk '{ print $2 }'
            ;;
        Linux)
            # /proc/loadavg → "1.85 1.90 2.01 3/512 12345"; field 1 is the 1-min avg.
            awk '{ print $1 }' /proc/loadavg 2>/dev/null
            ;;
        *)
            : # unknown OS → echo nothing → caller fails open
            ;;
    esac
}

# _host_load_read_ncpu — echo the CPU core count. Echoes nothing / a non-positive
# value when it can't read (caller then fails open).
_host_load_read_ncpu() {
    if [ -n "${HOST_NCPU_OVERRIDE:-}" ]; then
        printf '%s' "$HOST_NCPU_OVERRIDE"
        return 0
    fi
    case "$(uname -s 2>/dev/null)" in
        Darwin) sysctl -n hw.ncpu 2>/dev/null ;;
        Linux)  nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null ;;
        *)      getconf _NPROCESSORS_ONLN 2>/dev/null ;;
    esac
}

# ── pure helpers (awk float math — never bash arithmetic for floats) ────────

# _host_load_ratio LOAD NCPU — print load/ncpu to 2 decimals. Prints "?" when the
# inputs are unusable (non-numeric / ncpu ≤ 0) so a log line can't divide by zero.
_host_load_ratio() {
    awk -v l="$1" -v n="$2" 'BEGIN {
        if (l !~ /^[0-9]+(\.[0-9]+)?$/ || n !~ /^[0-9]+(\.[0-9]+)?$/ || n + 0 <= 0) {
            print "?"; exit 0
        }
        printf "%.2f", l / n
    }'
}

# _host_load_over_ratio LOAD NCPU CAP — exit 0 (true) iff LOAD/NCPU > CAP.
# FAILS OPEN: any unusable input (non-numeric LOAD/NCPU/CAP, or NCPU ≤ 0) exits 1
# ("not over"), so a guard that can't measure never blocks the suite and awk never
# divides by zero. Float-exact: 27/18 = 1.5 is NOT over a 1.5 cap; 27.1/18 IS.
_host_load_over_ratio() {
    awk -v l="$1" -v n="$2" -v cap="$3" 'BEGIN {
        if (l !~ /^[0-9]+(\.[0-9]+)?$/) exit 1
        if (n !~ /^[0-9]+(\.[0-9]+)?$/) exit 1
        if (cap !~ /^[0-9]+(\.[0-9]+)?$/) exit 1
        if (n + 0 <= 0) exit 1
        exit !(l / n > cap)
    }'
}

# ── the guard ───────────────────────────────────────────────────────────────
# wait_for_host_load — the entry point. Returns 0 to proceed, or
# HOST_LOAD_SATURATED_EXIT (75) when the host stays over-ratio past the wait cap.
wait_for_host_load() {
    # Escape hatch: disabled → no-op, proceed.
    case "${HOST_LOAD_GUARD_DISABLE:-}" in
        1|yes|true|on)
            echo "[host-load] guard disabled via HOST_LOAD_GUARD_DISABLE — proceeding"
            return 0
            ;;
    esac

    local cap poll max_wait ncpu load1 ratio cap_disp waited
    cap="${HOST_LOAD_MAX_RATIO:-1.5}"
    poll="${HOST_LOAD_POLL_SECS:-15}"
    max_wait="${HOST_LOAD_MAX_WAIT_SECS:-300}"

    # Validate integer knobs; a typo like "15s" or 0 would break the loop, so fall
    # back to the defaults rather than spin or misbehave.
    case "$poll" in ''|*[!0-9]*) poll=15 ;; esac
    [ "$poll" -lt 1 ] && poll=1
    case "$max_wait" in ''|*[!0-9]*) max_wait=300 ;; esac
    # cap is validated inside _host_load_over_ratio (float); a bad cap fails open.

    # Core count. Unusable → fail open (can't compute a ratio without it).
    ncpu="$(_host_load_read_ncpu)"
    case "$ncpu" in
        ''|*[!0-9]*|0)
            echo "[host-load] could not determine CPU core count (got '${ncpu}') — proceeding without load guard"
            return 0
            ;;
    esac

    cap_disp="$(awk -v c="$cap" 'BEGIN { if (c ~ /^[0-9]+(\.[0-9]+)?$/) printf "%.2f", c; else printf "%s", c }')"

    # First sample. Under cap (or unreadable load → fails open in the compare) →
    # proceed with a single sample: negligible overhead on a healthy host.
    load1="$(_host_load_read_load1)"
    if ! _host_load_over_ratio "$load1" "$ncpu" "$cap"; then
        return 0
    fi

    # Over cap → wait-and-back-off.
    ratio="$(_host_load_ratio "$load1" "$ncpu")"
    echo "[host-load] load ${load1} / ${ncpu} cores = ${ratio}x > ${cap_disp}x cap; waiting up to ${max_wait}s for the host to settle…" >&2
    waited=0
    while [ "$waited" -lt "$max_wait" ]; do
        sleep "$poll"
        waited=$((waited + poll))
        load1="$(_host_load_read_load1)"
        if ! _host_load_over_ratio "$load1" "$ncpu" "$cap"; then
            ratio="$(_host_load_ratio "$load1" "$ncpu")"
            echo "[host-load] load recovered: ${load1} / ${ncpu} cores = ${ratio}x ≤ ${cap_disp}x cap after ${waited}s — proceeding" >&2
            return 0
        fi
        ratio="$(_host_load_ratio "$load1" "$ncpu")"
        echo "[host-load] load ${load1} / ${ncpu} cores = ${ratio}x > ${cap_disp}x cap; waited ${waited}s/${max_wait}s…" >&2
    done

    # Still saturated after the wait cap — refuse rather than pile the browser
    # swarm onto a pegged host and wedge the machine.
    load1="$(_host_load_read_load1)"
    ratio="$(_host_load_ratio "$load1" "$ncpu")"
    echo "[host-load] host still saturated after ${max_wait}s (load ${load1} / ${ncpu} cores = ${ratio}x > ${cap_disp}x cap) — refusing to launch to avoid wedging the machine" >&2
    return "$HOST_LOAD_SATURATED_EXIT"
}
