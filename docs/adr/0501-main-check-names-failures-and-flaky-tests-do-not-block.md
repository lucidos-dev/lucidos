# 0501: The main check names each failed test, reruns the engine's failures once, and opens a flaky fix thread for a pass with flaky tests; fix threads run on Sonnet at medium effort

- **Status**: Accepted
- **Date**: 2026-10-16
- **Amends**: [0464](0464-main-check-on-github.md)
- **Amended by**: [0536](0536-fast-test-loop.md): `/harden`'s retry
  round names failed engine tests with the same parser.

## Context

In its first three days the main check ran 33 times and ended red 12 times.
Read from what each fix thread landed, about seven were timing flakes. Two
failed on Linux only. Three were a spec that missed a UI rename.

Every fix thread ran Claude Code on Opus at high effort, and the dozen cost
about 92 USD. Cache reads and writes from large contexts drove the cost.

Two defects fed it. The verdict carried the last 25 lines of each failed
phase, which is the totals block. So each fix thread started by fetching the
whole run log. And a flaky test blocked every later check until someone closed
the red.

## Decision

**The verdict names every failed test, a pass with flaky tests blocks nothing,
and fix threads run on a mid-tier model.**

- **The verdict carries `failures`**: the suite, the name and the head of the
  error of each failed test, parsed from cargo and Playwright output. The fix
  prompt lists them and keeps the raw tail only when nothing parsed.
- **The prompt names Linux and forbids the symptom fix.** A timing failure is a
  race to find. Raising a timeout, a retry count or a frame allowance needs a
  measured reason stated in the commit.
- **Fix threads run on Claude Code, Sonnet 5.5, medium effort.** The engine
  passes the implementer when it opens the thread. No other engine turn
  changes. The prompt names a handoff checkpoint, which reads the one existing
  threshold through `lucidos successor offer`.
- **The engine job reruns its failed tests once, by exact name.** Playwright
  already retries each browser test once. A test that passes the rerun is
  flaky: the job passes and reports it. A compile error, a crash or a failure
  list that does not match the summary skips the rerun and stays a failure.
- **A check whose only problems are flaky tests is a pass.** `MainCheckPassed`
  carries `flaky` and `flaky_fix_thread_id`. The pass opens one *flaky fix
  thread* and holds no check. At most one is open. While it is, a later pass
  records its flaky tests and opens nothing.
- **The flaky fix thread closes** when a change from it lands, or when the
  maintainer archives or deletes it. No event records the close: recovery
  derives it from the landing, the archive or the delete the log already holds.

A red check behaves as ADR 0464 says. A test that fails twice stays red.

## Rationale

A fix thread that starts from a named failure skips the log fetch, and most of
its context. A mid-tier model reads a named cause well, and costs a fraction of
the large one. The handoff checkpoint caps what a long investigation costs.

A flaky red blocked the check for everyone, and its fix is a different job from
a regression's. Splitting it keeps main's verdict honest and the flake in view.
Deriving the close from existing events keeps the state stateless: nothing new
lives only in memory, and no new event type needs a consumer.

## Consequences

- A flaky test no longer turns main red. It opens a thread, once.
- A thread archived or deleted is noticed at the next flaky pass or the next
  restart, not at once. The snapshot can show it open until then.
- A new engine reason, `main_check_flaky`, marks the thread's first prompt.
- A real regression that fails once and passes its rerun reads as flaky.
  The flaky thread then carries it. This is the cost of not blocking.

## Alternatives considered

- **A fresh-job rerun of failed browser specs.** Playwright's in-run retry
  already covers it, and another GitHub run costs ten minutes or more.
- **Let a flaky test block, as before.** It spends a fix thread per flake and
  stops every later check.
- **Skip the rerun and quarantine known flaky tests.** A list rots, and it
  hides the test from the one signal that finds the race.
- **A preference for the fix-thread implementer.** One named constant now. A
  preference is a later change if the maintainer wants to tune it from the UI.
