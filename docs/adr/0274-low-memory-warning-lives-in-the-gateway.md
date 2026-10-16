# 0274: The low-memory warning lives in the gateway and fires on sustained pressure, never on free RAM

- **Status**: Accepted (amended by
  [ADR 0283](0283-slowness-opens-the-warning.md): a slow engine also opens an
  episode, and the warning is now the slowness warning)
- **Date**: 2026-09-24

## Context

A user's Lucidos sat at "Requesting" for long stretches and felt slow on every
turn. Their 16 GB Mac had 8.4 GB of swap in use. Chrome held about 7 GB. The
engine used 865 MB, and macOS had compressed or swapped out 858 MB of it, so
every message first paged the engine back in.

Lucidos was the victim, not the cause. Nothing told them the machine was starved,
so they could not tell a slow machine from a slow Lucidos.

The e2e memory guard had already met every trap a detector like this can fall
into. Free RAM and compressor size read "danger" on a healthy Mac (ADRs 0175,
0176). One critical kernel reading appears on an idle host (ADR 0182).

## Decision

The gateway samples kernel memory pressure every 30 seconds. An episode opens
when 8 of the last 10 samples are elevated, and never before the window holds
10 samples. It closes after 10 consecutive normal ones. A workspace banner
names the biggest users by app and recommends one action.

- **macOS elevated:** the pressure level is warn or worse, AND swap in use is at
  least max(2 GB, 10% of RAM).
- **Linux elevated:** PSI `some avg60` is at least 10.
- **Unreadable:** no warning, ever.

## Rationale

- **The gateway is the one process per machine**, and memory belongs to the
  machine. It runs in the macOS app and the Linux tarball. An engine-side check
  would warn once per workspace.
- **Pressure plus swap, held for five minutes**, is the shape the e2e guard
  proved. A single signal, or a single sample, false-positives on healthy hosts.
- **Swap never decides alone.** It is accumulated state and stays high long
  after pressure ends, so it corroborates an opening but never holds one open.
- **Attribution is the point.** Without it the warning blames Lucidos for
  Chrome's memory. Footprint counts compressed pages, which RSS misses.
- **A banner, not a push notification.** A push arrives when the user is not
  looking at Lucidos. A banner explains the slowness while they are.
- **Lucidos is named by process tree.** The roots are the gateway, every
  running engine, and the embedded postmaster. An engine outlives the gateway
  that spawned it and is then reparented, so a restarted gateway adopts it
  without being its parent. Its pidfile names it instead.
- **Engines come from live stacks, never the registry.** A stopped workspace
  keeps its pidfile. Once the OS reuses that pid, it names somebody else's app,
  and the banner would blame Lucidos for it.
- **Workspace windows poll the gateway rather than subscribing.** The gateway has no
  event stream of its own; it only proxies each engine's. The answer is a
  cached snapshot, so a poll a minute costs one loopback read.

## Consequences

- One loopback GET per workspace window per minute, and one sysctl or file read
  per 30 seconds in the gateway. Process enumeration runs only during an
  episode.
- The desktop app's web content runs in a system WebKit process that is not a
  gateway child. It groups under "Web pages in Safari and other apps", so the
  Lucidos total runs slightly low.
- A few system programs get a plain name instead of their executable, such as
  "Virtual machines (such as Docker)". They stay processes, never apps, so the
  banner names them but never tells the user to quit one.
- Lucidos does not yet shed its own load under pressure. That is a separate
  decision about what to give up.

## Alternatives considered

- **Free RAM or compressor thresholds.** Rejected: both measured macOS
  artifacts rather than danger (ADRs 0175, 0176).
- **Per-engine detection with a notification**, like the low-disk warning.
  Rejected: one warning per workspace, and a notification is the wrong weight
  for a standing condition.
- **A minimum-RAM check at install.** Deferred: 16 GB was enough once Chrome
  was restarted, so there is no evidence for a floor.
- **Shelling out to `top`.** Rejected: `libc` already exposes footprint, parent
  and path per process, with no fork and no parsing.
