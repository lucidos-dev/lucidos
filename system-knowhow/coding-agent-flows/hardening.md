---
name: Default flow: hardening
description: Automatic handoff checkpoints at first entry and between review iterations, with safe successor continuation.
---

# Default flow: hardening

A reference of `system-knowhow/coding-agent-flows`, which says how to take it.
Copy the block below to the start of the review flow, markers included.

<!-- default-flow: hardening -->
#### Handoff checkpoints

**First entry.** Before any review, run
`lucidos successor offer --at pre-harden --default-floor` and follow what it
prints. Only a new hardening run enters here. A successor this checkpoint
started skips it and starts at the review. `--default-floor` makes the
surface's default rung the floor for this run.

**Iteration boundary.** When a completed iteration needs another review,
record its actual verdict and commit its fixes first. Then run exactly once:
`lucidos successor offer --at harden-iteration --default-floor`.
Do not invent a progress total: the engine estimates the next review cycle.

Follow its automatic verdict without asking the user. The engine owns the
configured context ceiling and handoff estimate; do not compare token counts
locally or change preferences. A failed or unavailable reading means carry on.
A passing final iteration goes straight to the marker, with no checkpoint.

**Before an iteration handoff.** Join or explicitly stop every session-owned
suite runner and reviewer, including a pending suite kickoff. Record stopped
or interrupted work as incomplete. No process is assumed to survive the turn.

Write the successor narrative with:

- Review head, current HEAD, next iteration number, tier and scope flags.
- Findings, fix commits, suite state paths and actual verdicts.
- Outstanding gates, reviewer status and exact next command.

Preserve evidence on disk; the repository's suite validator decides
whether completed results still count. Missing exit files, stale inputs and a
passing retry subset never certify a full run.

**Iteration successor entry.** Skip this checkpoint once and resume at the
next iteration's scope selection, preserving the review head, iteration count
and tier. Do not repeat first-entry initialization. Missing or unusable review
state widens the review; missing or invalid suite evidence requires rerunning
verification. After another completed iteration needs a repeat, evaluate the
iteration checkpoint again. These rules cover both Claude Code and Codex,
full and fast tiers, root threads and sub-threads.
<!-- /default-flow: hardening -->

## The rest of the flow

The block is the only part Lucidos ships verbatim. The steps after it are the
repository's own, and a review flow usually runs them in this order:

1. Review the whole change against the repository's rules.
2. Fix what the review found, and commit.
3. Run the repository's own verification commands: its linters and test suites.
4. Repeat from step 1 until a pass finds nothing.
