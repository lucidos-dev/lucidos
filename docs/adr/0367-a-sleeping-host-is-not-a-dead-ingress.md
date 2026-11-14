# 0367: A round that overlaps host sleep is not ingress evidence, and a long sleep is one quiet event

- **Status**: Accepted; amends
  [ADR 0143](0143-webhook-ingress-probed-per-address-family.md) § Layer 1, the
  two-strike debounce. The stage table and ADR 0172's
  `local-egress-blocked` stand unchanged.
- **Date**: 2026-10-05

## Context

On the night of 2026-10-04 a Mac on mains power with `pmset sleep 1` raised
seven `WebhookIngressDegraded` declarations. It slept, and woke for about 45
seconds every 15 minutes for DarkWake maintenance. Two declarations line up
with `pmset -g log`:

| Event | Time (Oslo) |
|---|---|
| Sleep entered | 04:49:18 |
| `WebhookIngressDegraded` | 04:49:26 |
| DarkWake | 07:25:04 |
| Back to sleep | 07:25:49 |
| `WebhookIngressDegraded` | 07:25:51 |

Three of the seven cleared by themselves on wake. The bar and the event blamed
the funnel each time, when the cause was sleep. Every user with a webhook on a
laptop, or on a desktop that sleeps, can get this.

Two facts explain it:

- **The two failed rounds were never consecutive.** With a dark wake every
  quarter hour, round 1 and round 2 ran in different wakes. The debounce counted
  two strangers as one run.
- **A missed tick fires late.** `tokio-cron-scheduler` runs a job once the wall
  clock passes its next tick. On wake it fires the missed tick at once. The
  07:25:04 round was the 07:15 tick, ten minutes late, in a 45-second wake.

## Decision

**A round the computer slept through is not evidence.** It never declares and
never counts as a strike. It breaks the debounce chain, so the next awake round
is strike 1. It can still recover a standing outage.

A round slept through when any of three signs holds:

| Sign | Measured as |
|---|---|
| During the round | wall-clock elapsed exceeds monotonic elapsed by more than 30 s |
| Since the previous round | the same gap, from that round's end to this round's start |
| Late tick | the round started more than 60 s after its planned quarter hour |

One pure function, `core::host_sleep::slept`, reads all three.

**A long sleep is said once, quietly.** The check sums slept-through rounds into
a *sleep spell*. The first awake round that probes closes it. A spell of 30
minutes or more emits `WebhookDeliveriesSleptThrough`, with the hook, the host,
the port and `slept_secs`. Nothing pushes on it unless a workspace trigger
subscribes.

## Rationale

### Two clocks are enough, and need no OS API

`SystemTime` keeps running while the machine sleeps. `Instant` does not, on both
shipped platforms: macOS reads `CLOCK_UPTIME_RAW` and Linux reads
`CLOCK_MONOTONIC`, and both exclude sleep. So wall time gaining on monotonic time
over a span means the host slept inside it.

The tolerance is 30 seconds. NTP slews the wall clock by well under a second per
quarter hour. A wall clock that steps backward reads as no sleep. A large forward
step reads as a sleep, which only sets one round aside.

### The late tick catches a round the gap cannot

The engine's first round after a start has no previous round to measure from.
The 07:25 round needs no such baseline: it started ten minutes after its planned
instant, and a busy host delays the scheduler by seconds, not a minute. The plan
is the start floored to the quarter hour, in UTC from the epoch, which is how
`Job::new_async` schedules.

### The first round after a wake is set aside too

The "since the previous round" sign marks it, by construction. That is the
re-probe rule: after any sleep the check needs two clean rounds before it
declares. A real outage that spans a sleep is declared one round later than
on an awake host. Missing it outright is not possible, because every clean
round still counts.

### Sleep can take answers away, never add one

A 401 is positive evidence that the whole chain works, whenever it arrived. So a
round that slept through may still recover, by the same rule as any round:
every declared family answered. That retracts an outage the moment the host is
seen healthy, which is what the three self-clearing declarations did.

### Saying it: an event, not a push, and not the bar

GitHub does not resend a failed delivery on its own. So a long sleep with a hook
on may have cost real deliveries, and the honest record says so. Three shapes
were weighed:

- **An event**, chosen. It is on the timeline and costs the user nothing until
  a trigger subscribes, so a morning digest is one trigger away.
- **A line on the Webhooks row.** Deferred. It needs a read route and a
  rendered design, and nobody has asked for it yet.
- **Nothing beyond a log line.** Rejected. The workspace could not tell a quiet
  night from a night it slept through.

One event per spell keeps it quiet. A night of dark wakes is one spell, because
no dark wake makes an awake round that probes. The threshold is 30 minutes, the
time an awake host needs to declare an outage. The sum is a lower bound. A late
tick stands in for the gap only when no earlier round exists to measure from,
so no minute counts twice.

The event names no address family and no stage, and the bar never reads it. The
bench repair trigger subscribes to `WebhookIngressDegraded`, so nothing re-arms a
funnel over a sleep.

### Windows

The engine does not build for Windows: it imports `std::os::unix` without a
guard, and `install.sh` refuses the OS. On Windows `Instant` reads
`QueryPerformanceCounter`, which counts through sleep. If the engine ever builds
there, only the late-tick sign works, and the spell reads no duration.

## Consequences

- **The two matched false outages do not recur.** Pinned by
  `the_two_false_outages_of_that_night_are_not_declared` in
  `scheduler/webhook_ingress/mod.rs`, built from the real timeline.
- **Two genuine failed rounds on an awake host declare exactly as before.** The
  strike rule in `core::webhook_ingress::decide` is unchanged.
- **A real outage that begins during sleep is declared 30 minutes after a full
  wake**, not on the first round.
- **The spell is memory only.** A restart starts a new spell, so it can cost at
  most one report. It is a cache, which the statelessness rule allows.
- **The clock baseline outlives the debounce.** A round that stops early still
  records where it ended, so the next round measures its gap.
- **One new wire event**, on the `webhook` aggregate. No existing payload, stage
  or route changes.

## Alternatives considered

- **An OS sleep API** (IOKit power notifications on macOS, logind on Linux).
  Rejected: two platform bindings to learn what two clocks already say, and the
  macOS one needs a run loop the engine does not have.
- **Skipping the probe during a dark wake.** Rejected: the engine cannot tell a
  dark wake from a full wake without that API, and the clocks make it
  unnecessary. A round that ran is set aside after the fact instead.
- **Raising the strike count on laptops.** Rejected: it delays every real outage
  on every machine, and two strangers would still count as a run.
- **Counting the first round after a wake as strike 1.** Rejected: the network
  may still be coming up, which is the fault that caused the 07:25 case.
- **A push on each long sleep.** Rejected: one "your computer slept" a night is
  noise, and noise teaches the user to ignore the real alarm.
- **Reusing `WebhookIngressDegraded` with a sleep stage.** Rejected: a trigger
  coding against that event would page the user, or re-arm the funnel, for a
  sleep.

Plan: `docs/plans/2026-10-05-a-sleeping-host-is-not-a-dead-ingress.md`.
