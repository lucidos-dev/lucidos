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
# ── The second half: MID-RUN saturation, classified not hidden ──────────────
# Gating only at launch leaves the run blind to a host that goes bad AFTERWARDS.
# On 2026-07-26 an external macOS daemon burst (an MDM agent plus mdmclient /
# mobileassetd / managedcorespotlight — the periodic management sweep of an
# MDM-managed corporate fleet) pinned the host at load 83–227 for ~40 MINUTES
# mid-run.
# The launch gate had already passed on an idle host, so the browser suite ran on
# into starvation and produced mobile-webkit timeouts that read exactly like
# product failures; a human had to notice and re-run.
#
# So the guard also SAMPLES throughout the run:
#   start_host_load_sampler      — background loop appending "<epoch> <load1>"
#   stop_host_load_sampler       — idempotent teardown
#   report_host_load_saturation  — drains the samples at the end, always prints a
#                                  one-line peak/over-cap summary, and when the
#                                  run FAILED and the host was sustainedly over
#                                  the SAME cap, prints a loud banner saying the
#                                  timeouts are not trustworthy evidence.
#
# Deliberately NOT an auto-retry, and it never touches the exit code: a run that
# failed still fails. The point is honest classification — telling the operator
# that these particular failures cannot be read as product defects — not hiding
# them. Same measurement seams, same HOST_LOAD_MAX_RATIO cap, same
# _host_load_over_ratio compare as the launch gate; there is exactly one
# saturation threshold in this file.
#
# Knobs (all optional; sane defaults):
#   HOST_LOAD_MAX_RATIO       load1/ncpu ratio we tolerate before backing off
#                             (default 1.5 — load up to 1.5× core count is fine).
#                             The mid-run sampler classifies against this SAME cap.
#   HOST_LOAD_POLL_SECS       seconds between polls while over-ratio (default 15).
#                             Also the mid-run sampling interval.
#   HOST_LOAD_MAX_WAIT_SECS   total seconds to wait before refusing (default 300)
#   HOST_LOAD_SUSTAINED_MIN_SECS
#                             longest contiguous over-cap stretch that counts as
#                             "sustained" for the mid-run banner (default 120).
#                             Not a second threshold — a duration, so a brief
#                             spike (the engine's own release-build tail, one
#                             heavy chunk) can't be blamed for a failing run.
#   HOST_LOAD_GUARD_DISABLE   =1 → no-op that logs it's disabled and returns 0;
#                             also suppresses the mid-run sampler (escape hatch
#                             for CI where load is meaningless)
#   HOST_LOAD_OVERRIDE        test hook: force the 1-min load reading
#   HOST_NCPU_OVERRIDE        test hook: force the core count
#   HOST_LOAD_SAMPLES_FILE    override the mid-run samples path (tests)
#   HOST_LOAD_SAMPLER_PIDFILE override the sampler pidfile location (tests)
#
# Exit code: HOST_LOAD_SATURATED_EXIT (75, the EX_TEMPFAIL sysexits convention) —
# returned by wait_for_host_load when the host is still saturated after the wait
# cap. Distinguishable from an ordinary test failure (1).
#
# Fail-open by design: if the guard cannot MEASURE the host (unknown OS,
# unreadable load, ncpu empty/zero/non-numeric) it logs and returns 0. A guard
# that can't measure must never block or crash the suite — the same posture as the
# reaper's "no ps → warn and don't start". The mid-run half inherits it: no
# samples, no core count, or a nonsense cap means no banner, never a crash.
#
# Sourced by scripts/lib/e2e.sh; invoked by scripts/e2e-browser.sh — the gate
# right before the Playwright browser swarm spawns, the sampler around it.

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
# _host_load_guard_disabled — true when the escape hatch is set. One reader for
# the launch gate, the sampler, and the report, so "disabled" can never mean
# different things to different halves of the guard.
_host_load_guard_disabled() {
    case "${HOST_LOAD_GUARD_DISABLE:-}" in
        1|yes|true|on) return 0 ;;
        *) return 1 ;;
    esac
}

wait_for_host_load() {
    # Escape hatch: disabled → no-op, proceed.
    if _host_load_guard_disabled; then
        echo "[host-load] guard disabled via HOST_LOAD_GUARD_DISABLE — proceeding"
        return 0
    fi

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

# ── the mid-run sampler ─────────────────────────────────────────────────────
# wait_for_host_load only knows about the instant it fired. Everything below
# watches the REST of the run, so a host that goes bad after launch is classified
# instead of masquerading as product failure. See the header for the incident.

# In-memory handle to the running sampler loop (set by start_host_load_sampler).
HOST_LOAD_SAMPLER_PID="${HOST_LOAD_SAMPLER_PID:-}"

# Where the samples land. Under the e2e workspace's .lucidos/ (ephemeral runtime
# state, mirroring the reaper's pidfile) so a crashed run leaves nothing tracked.
_host_load_samples_file() {
    printf '%s' "${HOST_LOAD_SAMPLES_FILE:-${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.lucidos/host-load-samples}"
}

_host_load_sampler_pidfile() {
    printf '%s' "${HOST_LOAD_SAMPLER_PIDFILE:-${E2E_WORKSPACE:-$HOME/workspaces/e2e-test}/.lucidos/host-load-sampler.pid}"
}

# _host_load_sampler_loop INTERVAL FILE — append "<epoch> <load1>" every INTERVAL
# seconds. Mirrors the reaper's loop, including killing the in-flight `sleep` on
# SIGTERM so nothing is left reparented to init. Every step is failure-tolerant:
# the sampler is observability, and must never be able to fail the run it watches.
_host_load_sampler_loop() {
    local interval="$1" file="$2"
    local sleep_pid="" load1
    trap '[ -n "$sleep_pid" ] && kill "$sleep_pid" 2>/dev/null; exit 0' TERM INT
    while :; do
        load1="$(_host_load_read_load1 || true)"
        if [ -n "$load1" ]; then
            printf '%s %s\n' "$(date +%s)" "$load1" >> "$file" 2>/dev/null || true
        fi
        sleep "$interval" &
        sleep_pid=$!
        wait "$sleep_pid" 2>/dev/null || true
        sleep_pid=""
    done
}

# start_host_load_sampler — begin sampling. Idempotent; a no-op when the guard is
# disabled or the samples file isn't writable. Truncates any previous run's
# samples so a crashed predecessor can't be blamed on this run.
start_host_load_sampler() {
    # Already sampling for THIS run → nothing to do (and don't reap our own loop
    # below). Checked before the disable branch so the escape hatch can't be
    # flipped mid-run into killing a live sampler.
    if [ -n "${HOST_LOAD_SAMPLER_PID:-}" ] && kill -0 "$HOST_LOAD_SAMPLER_PID" 2>/dev/null; then
        return 0
    fi

    # Reap a predecessor. The loop is `disown`ed, so a SIGKILLed e2e-browser.sh
    # leaves it appending forever; two samplers interleaving into one file would
    # make the report describe a run that never happened. stop_ reads the pidfile,
    # so it finds an orphan from a previous process. Runs BEFORE the disable
    # branch: a disabled run must not leave one alive either.
    stop_host_load_sampler

    local file
    file="$(_host_load_samples_file)"

    if _host_load_guard_disabled; then
        # Drop any samples a crashed predecessor left, so the report can't
        # attribute another run's saturation to this one.
        rm -f "$file" 2>/dev/null || true
        echo "[host-load] mid-run sampler disabled via HOST_LOAD_GUARD_DISABLE"
        return 0
    fi

    local interval pidfile
    interval="${HOST_LOAD_POLL_SECS:-15}"
    case "$interval" in ''|*[!0-9]*|0) interval=15 ;; esac

    mkdir -p "$(dirname "$file")" 2>/dev/null || true
    if ! : > "$file" 2>/dev/null; then
        echo "[host-load] cannot write ${file} — mid-run sampling off for this run"
        return 0
    fi

    _host_load_sampler_loop "$interval" "$file" &
    HOST_LOAD_SAMPLER_PID=$!
    disown "$HOST_LOAD_SAMPLER_PID" 2>/dev/null || true

    pidfile="$(_host_load_sampler_pidfile)"
    echo "$HOST_LOAD_SAMPLER_PID" > "$pidfile" 2>/dev/null || true

    echo "[host-load] mid-run sampler started (pid=${HOST_LOAD_SAMPLER_PID}, every ${interval}s → ${file})"
}

# stop_host_load_sampler — terminate the loop. Idempotent and safe in an EXIT
# trap. Reads the pidfile as a fallback so a parent script can reap a sampler
# started by a child. Deliberately LEAVES the samples file: the report drains it.
stop_host_load_sampler() {
    local pid pidfile
    pidfile="$(_host_load_sampler_pidfile)"
    pid="${HOST_LOAD_SAMPLER_PID:-}"
    if [ -z "$pid" ] && [ -f "$pidfile" ]; then
        pid="$(cat "$pidfile" 2>/dev/null)"
    fi

    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi

    rm -f "$pidfile" 2>/dev/null || true
    HOST_LOAD_SAMPLER_PID=""
}

# _host_load_summarize_samples FILE NCPU CAP — print
# "<sustained_secs> <peak_load> <peak_ratio> <over_samples> <total_samples>".
# `sustained_secs` is the LONGEST contiguous stretch of over-cap samples, so an
# isolated spike scores near zero however high it was. Exits 1 (→ no report) on
# unusable inputs or an empty/garbage sample set — fail open, same as the gate.
_host_load_summarize_samples() {
    awk -v ncpu="$2" -v cap="$3" '
        BEGIN {
            if (ncpu !~ /^[0-9]+(\.[0-9]+)?$/ || ncpu + 0 <= 0) exit 1
            if (cap  !~ /^[0-9]+(\.[0-9]+)?$/) exit 1
            peak = 0; n = 0; over = 0; in_run = 0; run_start = 0; sustained = 0
        }
        {
            if ($1 !~ /^[0-9]+$/) next
            if ($2 !~ /^[0-9]+(\.[0-9]+)?$/) next
            ts = $1 + 0; l = $2 + 0
            n++
            if (l > peak) peak = l
            if (l / ncpu > cap) {
                over++
                if (!in_run) { in_run = 1; run_start = ts }
                if (ts - run_start > sustained) sustained = ts - run_start
            } else {
                in_run = 0
            }
        }
        END {
            if (n == 0) exit 1
            printf "%d %.2f %.2f %d %d\n", sustained, peak, peak / ncpu, over, n
        }
    ' "$1"
}

# report_host_load_saturation RUN_RC — drain the samples and classify the run.
#
# ALWAYS prints a one-line load summary when there are samples (cheap, and it is
# the evidence a later triage needs). Prints the loud banner ONLY when the run
# FAILED and the host was over the cap for at least HOST_LOAD_SUSTAINED_MIN_SECS
# contiguously — a green run needs no excuse, and a brief spike is not one.
#
# ALWAYS returns 0. It reports; it does not decide. The caller exits with its own
# code, so a saturated run still fails — the banner says the failures are not
# trustworthy evidence, it does not make them go away.
report_host_load_saturation() {
    local run_rc="${1:-0}"
    case "$run_rc" in ''|*[!0-9]*) run_rc=1 ;; esac

    local file ncpu cap cap_disp min_secs stats
    file="$(_host_load_samples_file)"
    [ -f "$file" ] || return 0

    # Disabled guard → never classify. Whatever is in the file was not sampled
    # for this run (start_host_load_sampler removed its own), and reporting it
    # would be the exact mis-attribution this whole mechanism exists to prevent.
    if _host_load_guard_disabled; then
        rm -f "$file" 2>/dev/null || true
        return 0
    fi

    ncpu="$(_host_load_read_ncpu)"
    case "$ncpu" in
        ''|*[!0-9]*|0)
            echo "[host-load] core count unreadable — cannot classify mid-run host load"
            rm -f "$file" 2>/dev/null || true
            return 0
            ;;
    esac

    cap="${HOST_LOAD_MAX_RATIO:-1.5}"
    min_secs="${HOST_LOAD_SUSTAINED_MIN_SECS:-120}"
    case "$min_secs" in ''|*[!0-9]*) min_secs=120 ;; esac

    if ! stats="$(_host_load_summarize_samples "$file" "$ncpu" "$cap")"; then
        rm -f "$file" 2>/dev/null || true
        return 0
    fi
    rm -f "$file" 2>/dev/null || true

    local sustained peak peak_ratio over total
    read -r sustained peak peak_ratio over total <<< "$stats"
    cap_disp="$(awk -v c="$cap" 'BEGIN { if (c ~ /^[0-9]+(\.[0-9]+)?$/) printf "%.2f", c; else printf "%s", c }')"

    echo ""
    echo "[host-load] mid-run host load: peak ${peak} on ${ncpu} cores (${peak_ratio}x), ${over}/${total} samples over the ${cap_disp}x cap, longest sustained stretch ${sustained}s"

    [ "$run_rc" -ne 0 ] || return 0
    [ "$over" -gt 0 ] || return 0
    [ "$sustained" -ge "$min_secs" ] || return 0

    local mins
    mins="$(awk -v s="$sustained" 'BEGIN { printf "%.0f", s / 60 }')"

    echo ""
    echo "╔══════════════════════════════════════════════════════════════════════════╗"
    echo "║  HOST WAS SATURATED MID-RUN — THIS RUN'S FAILURES ARE NOT TRUSTWORTHY    ║"
    echo "╚══════════════════════════════════════════════════════════════════════════╝"
    echo "  peak load ${peak} on ${ncpu} cores = ${peak_ratio}x, against a ${cap_disp}x cap;"
    echo "  sustained above that cap for ${mins} min (${over} of ${total} samples over cap)."
    echo ""
    echo "  Timeouts and browser wedges under that load are starvation, not product"
    echo "  defects — an external daemon burst did exactly this to the 2026-07-26"
    echo "  nightly (load 83-227 for ~40 min mid-run, bogus mobile-webkit timeouts)."
    echo "  RE-RUN ON AN IDLE HOST before treating any failure here as real."
    echo ""
    echo "  The exit code is unchanged (${run_rc}) — this is classification, not"
    echo "  suppression, and the suite has NOT been retried."
    echo ""
    return 0
}
