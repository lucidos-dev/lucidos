# 0731: An integrated sub-thread hands off to a successor session like any coding-agent thread; amends 0465, retires 0474's block

- **Status**: Accepted
- **Date**: 2026-11-17
- **Amends**: [0465](0465-integrated-sub-threads.md)

## Context

ADR 0474 said an integrated sub-thread never hands off. Then a handoff spawned
a *successor thread* on a new branch. That broke the hand-up: the parent waited
for the old thread's branch, and the successor carried none of the integrated
plumbing.

ADR 0560 replaced successor threads with *successor sessions* in the same
thread. The thread keeps its id, worktree, branch and parent's wait. The block
stayed anyway. So a long integrated sub-thread could not hand off by command,
on the *Successor card*, at a *handoff checkpoint* or at its *context ceiling*.
Past the ceiling it got no re-entry and kept filling its context.

## Decision

An integrated sub-thread hands off like any other coding-agent thread.
`HandoffBlock` loses its `Integrated` variant, and `thread_context` no longer
reads the integrated flag.

## Rationale

- **Nothing the hand-up needs moves.** The successor session runs on the same
  branch, so the parent merges the same branch at the end.
- **The flag stays with the thread.** `thread_summaries.coding_agent_integrated`
  is per thread, and every session start reads it. The successor session still
  runs the fast tier and still proposes nothing.
- **The parent waits through it.** The parent callback already holds the report
  while a handoff is pending. The successor session reports at the real end.

## Consequences

- The `integrated` refusal reason of `lucidos successor start` is gone.
- ADR 0474's "An integrated sub-thread never hands off" no longer holds. 0474
  is already superseded by 0560.

## Alternatives considered

- **Keep the block and let the parent spawn a fresh integrated child.** The
  new child starts on its own branch without the old one's context, and the
  parent must merge two branches. A same-thread session needs neither.
