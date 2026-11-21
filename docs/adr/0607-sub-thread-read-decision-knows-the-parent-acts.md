# 0607: A sub-thread's own read decision is told when its parent agent acts on its report, which amends 0446

- **Status**: Accepted
- **Date**: 2026-10-31
- **Amends**: [0446](0446-read-decision-knows-the-parent-acts.md)

## Context

A nightly trigger thread spawned 18 coding-agent sub-threads. Most finished
clean: "already fixed on main", "all phases passed". Ten of them asked to be
read and landed in Review.

All 18 were spawned with wake mode `wake`. Every report reached the parent as
a child completion, and the parent's agent summarized each one. The user reads
the parent, so each read request duplicated what the parent already handled.

All ten requests came from the sub-thread's own agent (`lucidos request-read
yes`). The forced judge made none. ADR 0446 taught only the judge about the
parent. A sub-thread whose spawn declared a wake mode is told nothing about
being a sub-thread, so its only guidance was the generic read rule: "Yes for an
answer and no change". A clean "no change" report fits that rule, and the agent
followed it. A chat sub-thread has the same gap, since its thread context names
only its depth.

Plan: `docs/plans/2026-10-31-sub-thread-read-decision-knows-the-parent-acts.md`.

## Decision

A sub-thread whose report goes to a parent agent that acts on it is told so at
its own read decision. A routine result (a pass, nothing found, already fixed, a
ready change, a status) is a no, since the parent carries the read. It stays a
yes for a failure, a gap the user must know about, or something only the user
can do or decide.

- **Selection follows the wake duty.** A `wake` or `wake-when-all-done` child
  always gets the rule, since every report reaches the parent agent. A child
  that decides its own wake gets it conditionally: "when you wake your parent".
  A `no-wake` child and a top-level thread get none.
- **A turn the user opens in the sub-thread is answered as usual.** The rule
  says so, matching 0446's "a turn the user opened gets no note".
- **The must-see criteria are written once.** The judge's instructions and the
  agent rule share one macro fragment. The judge's rendered text is unchanged,
  so the measurements of ADR 0472 hold.
- **One rule text serves both surfaces**: the coding-agent session context and
  the chat sub-thread's thread context.

## Rationale

The agent decided wrongly only because it was never told. Telling it fixes the
cause for every sub-thread on every backend and model.

## Consequences

- Clean sub-threads of an orchestrating parent stop landing in Review. Their
  results reach the user through the parent's reply and its read decision.
- A sub-thread's session context grows by one short paragraph. The tools prefix
  and the top-level prompt do not change, so no prompt cache prefix moves.
- An agent that ignores the guidance still lands in Review: the failure is
  visible, never silent.

## Alternatives considered

- **Override the agent's yes in the engine when the parent wakes.** Rejected,
  for the reason in 0446: a failure the parent's reply drops would never reach
  Review.
- **Change the `request_read` tool description.** Rejected: it sits in the
  shared tools prefix, and a per-thread variant would break every sub-thread's
  prompt cache.
- **Soften the generic read rule.** Rejected: it serves top-level threads,
  where "an answer and no change" is a real yes.
