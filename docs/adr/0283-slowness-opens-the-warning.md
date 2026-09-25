# 0283: The slowness warning opens on a slow engine as well as on memory, and names memory only on evidence

- **Status**: Accepted (amends [ADR 0274](0274-low-memory-warning-lives-in-the-gateway.md):
  its home in the gateway, its window, and its memory-only rule all stand)
- **Date**: 2026-09-25

## Context

A 48 GB Mac was badly degraded for about twenty minutes. A workspace engine
answered in 20 to 50 seconds. Its database probe reported the database
unreachable about 20 times. A request that only reads a small file took 37
seconds. The low-memory warning never appeared.

The memory numbers could not tell this from an idle Mac. Pressure flapped
between normal, warn and critical. The compressor sat flat at 17.3 GB. Swap was
exactly zero, so the swap floor of max(2 GB, 10% of RAM) was never met. Those
are the readings ADR 0182 measured on a host doing nothing.

When the slowdown ended, the compressor fell to 2.5 GB and available memory
doubled. So memory was the likely cause, and the swap rule could not see it. A
large-RAM Mac compresses and drops cache long before it swaps.

A replay of three days of gateway log tested a direct signal. A 30-second
bucket counted as slow when a cheap request took 5 seconds or more, or the
database probe failed. Eight slow buckets out of ten opened an episode:

| When | Memory | What it was |
|---|---|---|
| The incident above | pressure warn or critical, compressor 17.3 GB | memory |
| Two days earlier | pressure normal, load average 17 to 20 | processor saturation |

Nothing else opened. At a 2-second threshold, milder stretches opened too.

## Decision

The warning becomes the **slowness warning**. An episode opens on a slow
engine as well as on the memory rule, with the same window.

- **Slow sample:** in the last 30 seconds, an engine that is alive and past its
  boot grace failed the supervisor's health probe. Failing means a 5-second
  timeout, or `database_reachable: false` in the body.
- **Elevated sample:** memory is elevated by the ADR 0274 rule, or some engine
  was slow.
- **Reason:** *memory* when the system's memory reading was raised in at least
  half of the window. That is pressure warn or worse on macOS, and PSI
  `some avg60` of 10 or more on Linux. It is also memory when no engine was
  slow in the window, since the memory rule alone held the episode. Otherwise
  the reason is *unclear*.
- **Memory reason:** the bar names the biggest memory users, in every
  workspace window, as before.
- **Unclear reason:** the bar says Lucidos is responding slowly, and names the
  busiest apps by processor share. It shows only in windows of a workspace that
  was slow.

## Rationale

- **Measure the distress, then explain it.** Memory statistics are artifacts on
  macOS (ADRs 0175, 0176, 0182). A health probe that cannot answer in 5
  seconds is not an artifact on any machine, because a healthy engine answers
  in milliseconds.
- **The signal is already paid for.** The supervisor probes every engine every
  2 seconds, and the body already carries `database_reachable` (ADR 0037). The
  gateway reads what it already receives, with no new request and no database
  handle (ADR 0014).
- **Pressure may explain, but never trigger.** A flapping pressure level opened
  false alarms when it decided alone. After slowness is proven, the same
  reading is fair evidence for the reason.
- **Say only what was measured.** The unclear bar claims that Lucidos is slow,
  which the probe proved. It names no cause. The busiest apps are facts the
  user can act on.
- **A slow workspace speaks only for itself.** One engine can be slow for its
  own reasons. A fast workspace's window must not claim to be slow. Memory
  belongs to the whole machine, so its bar shows everywhere.
- **The existing memory rule stays.** A small Mac that swaps hard can make every
  turn slow while the health probe still answers. ADR 0274's case still opens
  the bar on its own.

## Consequences

- Process enumeration still runs only during an episode. It now also reads
  processor time. The first scan of an episode measures over about 2 seconds.
- An unreadable memory reading now counts as not raised rather than closing the
  episode. The gateway can always observe its own engines, so the bar has no
  "unsupported" state.
- One episode can change its reason while it lasts. Its id does not change, so
  a dismissal holds.
- The route moves from `/~/api/v1/control/low-memory` to
  `/~/api/v1/control/slowness`. A client and gateway of different versions show
  no bar, never an error.
- Slowness from a full disk or disk contention shows as unclear. That is
  honest, but it gives no disk-specific advice.

## Alternatives considered

- **Drop the swap term on large Macs.** Rejected: it brings back exactly the
  idle-host false alarms of ADRs 0175 to 0182.
- **Slowness replaces the swap term, but the bar stays memory-only.** Rejected:
  it stays silent for a processor-bound slowdown like the second replay case.
- **Two separate bars.** Rejected: more to build, and both could show at once
  for one condition.
- **Engine-reported request latency percentiles.** Deferred: it needs a new
  engine report and its own tuning. The probe already caught both real cases.
- **The gateway's own timer lag.** Rejected: the small gateway can stay
  responsive while the big engines starve.
- **Name memory only on the swap rule.** Rejected: on a large-RAM Mac that
  never happens, so the incident above would show the unclear bar.
