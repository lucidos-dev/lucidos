---
name: e2e-lock-busy
description: Use when an e2e script refuses to start because every e2e slot is held ("ERROR: another e2e run is in progress", exit 75). Give up at once, never wait: no await-event, no sleep, no retry loop. Rerun a committed selection with --github, or rely on the other tests and the main check and report that the spec did not run.
---

# Every e2e slot is busy. Give up, do not wait.

Two concurrent Playwright sessions OOM'd a 32 GB Mac on 2026-04-19, so
`scripts/lib/e2e_lock.sh` caps local runs. A Chromium-only run takes one of two
*e2e slots*, `e2e-test` and `e2e-test-2`. A WebKit run or the umbrella takes
both (ADR 0536). An entrant that finds no free slot exits **75**.

That refusal is correct and is never worked around: **never delete the lock
file, never force it, never re-run with a flag that skips it.**

## Never wait for the lock (ADR 0541)

Waiting parked a dozen threads at once, each keeping its cache warm and
blocking its own Apply. So:

- **No `lucidos await-event --on E2ELockReleased`.**
- **No `sleep`, no retry loop, no rerun on a timer.**

## Do this instead

1. **A committed selection: run it on GitHub.** Add `--github` to the same
   command. A WIP commit is fine for this.
2. **Otherwise give up.** Rely on the tests that did run: engine tests,
   Vitest, typecheck. The *main check* runs every spec on GitHub after the
   change lands, and a red one opens a fix thread.
3. **Say so.** Name the spec that did not run in your report.

`/harden` does this for you: a touched-spec suite refused with 75 reads `SKIP`,
and a `SKIP` passes.

## A wait armed before this rule

`lucidos event-waits list` shows it. Cancel it with
`lucidos event-waits cancel --on E2ELockReleased`. Taking the lock also stands
it down on its own.

## Orphans are a different refusal

"orphaned processes" exits 2, not 75. A dead run left browsers or an engine
that the sweep could not stop. Clean up the listed PIDs, as the message says.
