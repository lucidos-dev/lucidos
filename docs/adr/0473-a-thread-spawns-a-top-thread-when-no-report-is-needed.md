# 0473: A thread spawns a top-thread, not a child, when it will not act on the result

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0451](0451-spawn-declares-the-wake-mode.md)
- **Amended by**: [0475](0475-any-thread-creates-a-top-thread.md). A
  top-thread needs no owner authority, so the advice applies in every turn.

Plan: `docs/plans/2026-10-14-spawn-a-top-thread-when-no-report-is-needed.md`.

## Context

A child thread costs its parent, even when the result is for the user:

- Its report re-opens the parent: on every clean finish under `wake`, the
  default outside Home, and on every failure under any mode. The cost knowhow
  prices one re-entry at $0.504 on Opus.
- Under `no-wake` its report is filed. The parent reads it next turn, and it
  stays in that transcript for good. Home never ends, so filed reports pile up
  there.
- It holds one of the parent's child slots while it runs.

A top-thread pays none of this. In the Lucidos development workspace, children
re-opened a parent 927 times in 30 days. The prompts never said when a
top-thread was the better choice. The spawn tools default to a child, and ADR
0451 named `no-wake` as the mode for work "the user reads in the child".

## Decision

Agent guidance asks one question before a spawn: will THIS thread act on the
result? If not, spawn with `relation: "top"` (`--relation top`). If so, spawn a
child, with the wake mode that fits. `no-wake` stays, for a result the user
reads that this thread may still want next turn.

## Rationale

The spawning thread knows the answer at spawn time, as ADR 0451 argued for the
wake mode. Saying it with the relation removes the cost at its source. A wake
mode only shapes how a report arrives, while a top-thread sends none.

Home loses no control: `follow_up_child_thread` from Home reaches any thread in
its workspace, top-threads included.

## Consequences

- The spawn tools keep `child` as their default. This is guidance, so a model
  that skips it costs what it cost before.
- A top-thread still needs the owner's standing instruction or approval (ADR
  0168). The advice applies in a turn the user opened or a trigger firing they
  authorized. Elsewhere a `no-wake` child stays the fallback.
  *Superseded by ADR 0475:* a top-thread needs no authority, so no fallback.
- A top-thread never reports a failure. Use it only where the user reads the
  thread, and so sees the failure there.
- The plan-builder handoff in `implementation-plan` spawns a top-thread, since
  the planning thread ends its turn and reads nothing back.

## Alternatives considered

- **Make `top` the default relation.** Rejected for now. The user asked for
  advice, and a default flip changes every spawn, including the owner-gated
  ones that would then fail.
- **Rely on `no-wake`.** It saves the clean-finish re-entry but still files the
  report and still wakes on failure. It also needs the agent to pass a mode, so
  it asks for the same judgment and saves less.
