# 0682: Main checks continue while red; repairs wait for idle capacity

- **Status**: Accepted
- **Date**: 2026-11-13
- **Amends**: [0464](0464-main-check-on-github.md), [0501](0501-main-check-names-failures-and-flaky-tests-do-not-block.md)

## Context

A red main stopped checks of later landings. Opening a repair immediately also
competes with chosen work for local build capacity. One repair should own all
findings, reducing repeated hardening costs.

## Decision

Checks continue while red, one at a time with coalescing. A newer failed check
updates the red under its existing repair owner. A newer pass closes it with
`passed_on_newer_main`; the failed verdict remains recorded as failed.

A fix landing closes red even while a check runs. That older check cannot reopen
red: its failure records `superseded_by_fix`, and a follow-up checks newer main.
Flaky findings share the existing repair owner.

The approved implementation plan also defers automatic repair starts until
workspace work and machine builds have been idle for two minutes. Fix now is an
owner action bypassing that admission check. A running repair is never stopped.
The waiting-only 30-second sampler observes other workspaces' build slots; it
does not retry checks. Automatic repairs use a yielding background build class.

## Rationale

Only the latest checked main determines red. Stable ownership avoids competing
repair sessions; retaining failed events preserves the audit history. Admission
protects chosen work without preempting a repair already underway.

## Consequences

The scheduler changes reach source checkouts capable of running the main check;
packaged installs cannot run it. Build classes cover macOS and Linux source
builds. Hardening, E2E selection, releases and GitHub checks remain unchanged.
A later pass drops waiting work but never stops a running repair.
Optional removal of touched WebKit checks is outside this decision.

## Alternatives considered

- Blocking later checks while red loses newer evidence and requires manual Close.
- One thread per finding repeats hardening and races other repairs.
- Stopping running repairs on new work discards paid progress.
