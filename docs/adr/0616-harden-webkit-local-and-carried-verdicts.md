# 0616: A macOS host runs the touched WebKit specs, a passing verdict carries, and the driver reruns a run with no verdict once

- **Status**: Accepted
- **Date**: 2026-11-02
- **Amends**: [0536](0536-fast-test-loop.md),
  [0584](0584-busy-host-filtered-e2e-goes-to-github.md)

## Context

ADR 0536 sent the touched mobile-webkit specs to a GitHub macOS runner. One
`/harden` on a frontend change then spent about 2.5 hours on that leg. A
cancelled job left no verdict (exit 76), and `/harden` read that as FAIL. The
retry round passed. The full round then sent WebKit to GitHub again, on the
same commit.

Over the last 300 runs, a WebKit-only run took a median of 10 minutes with the
macOS engine reused and 26 minutes with it built. The p90 was 43 minutes.

## Decision

Three changes, one per cause.

- **A. A macOS host runs the touched WebKit specs itself.** `e2e-touched-webkit`
  runs `ENGINE_CARGO_PROFILE=e2e ./scripts/e2e-browser.sh --webkit -f ...`.
  - The `e2e` profile builds dependencies at opt-level 3 and workspace crates
    incrementally.
  - A busy host still hands the run to GitHub through ADR 0584.
  - A Linux host keeps the GitHub path, because mobile-webkit runs on macOS only.
    `uname -s` picks the path.
  - On macOS the suite starts after `e2e-touched` ends, since a WebKit run takes
    both e2e slots.
- **C1. A passing touched-spec verdict carries into the next round.** A plain
  PASS records each spec with the commit it ran at, in `.lucidos/harden-carried`.
  A later round drops a spec whose record is valid: the commit is an ancestor
  of HEAD, and every path changed since passes `hs_unsafe_paths`. A suite with
  no specs left reports `PASS carried from <commit>` and runs nothing. A retry
  round carries nothing.
- **C2. The driver reruns a run with no verdict once.** On exit 76 with no
  shard test failure, the driver runs `gh run rerun <id> --failed` and
  reports again. A second miss stays 76. A failed test is never retried.

## Rationale

A runs the leg where the engine is already warm and no queue exists. C1 stops
the full round from repeating a leg that already passed on the same code. C2
fixes the no-verdict case at the driver, so every caller benefits, the main
check included.

## Consequences

- The WebKit leg on macOS costs a local run, not a 10 to 43 minute GitHub run.
- A contributor with no GitHub access can now run the WebKit leg.
- `/harden` skips a touched spec only when the diff since its pass touched no
  unsafe path. Any source edit reruns it.
- The main check is unchanged. It still runs full WebKit on GitHub.

## Alternatives considered

- **Self-hosted or paid runners.** Rejected in ADR 0536. They add cost and
  upkeep for a leg that runs locally.
- **Release profile for the local WebKit run.** It compiles slower on an
  engine edit. The `e2e` profile gave the same WebKit speed.
- **Run WebKit beside `e2e-touched`.** A WebKit run takes both slots, so the
  second suite would always find a slot held.
- **Retry any failed run.** A failing test is a verdict, so only a missing
  verdict reruns.
