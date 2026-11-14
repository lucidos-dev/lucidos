# 0475: Any thread creates a top-thread or detaches its own child with no owner authority

- **Status**: Accepted
- **Date**: 2026-10-14
- **Supersedes in part**: [0168](0168-a-thread-acts-in-its-own-subtree.md)
  clause 4, for creating a top-thread, and its amendment for an agent's detach
  ([0278](0278-a-child-thread-can-move-to-top-level.md)).
- **Amends**: [0473](0473-a-thread-spawns-a-top-thread-when-no-report-is-needed.md)

Plan: `docs/plans/2026-10-14-any-thread-spawns-a-top-thread.md`.

## Context

ADR 0168 clause 4 made creating a top-thread the workspace owner's button,
because the workspace root is nobody's subtree. ADR 0278 added an agent moving
its own child to top level, since that also puts a thread at the root.

In practice the gate taxed the work ADR 0473 asks for. A thread whose turn the
owner did not open got a 403 on `spawn-thread --relation top`. It then had to
raise an owner approval card, or fall back to a `no-wake` child that still
files its report into the parent. The gate was also uneven: the HTTP create
and the detach checked it, while the chat agent's own `relation: "top"` spawn
never did.

The owner asked for any thread to create a top-thread with no special
permission.

## Decision

Creating a top-thread needs no authority. Neither does an agent moving its own
direct child to top level. Every other clause-4 act keeps the gate, the
standing instruction and owner approvals.

## Rationale

**Creating a thread is not acting on one.** Clause 4's other verbs change or
stop work that exists: Apply, Discard, archive, cancel, answering a card. A new
top-thread changes nothing that exists. It holds no reach over its siblings,
and its spawner holds none over it.

**The root stays row-less.** Clause 1 still holds, so no thread reaches every
top-thread by clause 3. What clause 1 protected was reach over the root's
contents, not admission to it.

**The detach only gives something up.** The ladder still limits an agent to its
own direct children. Moving one out costs the agent its report and costs
nobody else anything.

## Consequences

- `ThreadReachVerb::CreateTopThread` and `DetachChild` stay in the enum as
  retired verbs, because stored approval cards carry their wire names. The gate
  answers either with "allowed", and an approval request for either is a 409.
- A trigger thread, or a verified subprocess with no thread, creates a
  top-thread freely too. The credential gate (ADR 0169) and the attribution
  checks still apply.
- `SpawnSuccessor`, a successor placed under the old thread's parent, keeps the
  gate. It adds a child to the parent's subtree, not a top-thread.
- ADR 0473's fallback to a `no-wake` child where the owner's authority was
  missing is gone. Its advice now applies in every turn.

## Alternatives considered

- **Keep the gate and lean on owner approval cards.** Rejected by the owner.
  Each card interrupts them for an act that changes nothing they own.
- **Ungate top-thread creation, keep the detach gated.** Rejected: both put a
  thread at the root, and the detach is the milder of the two. Keeping only
  the milder one gated would invert the risk.
- **Remove the retired variants from the enum.** Rejected: old approval cards
  would stop deserializing, and their threads' timelines with them.
