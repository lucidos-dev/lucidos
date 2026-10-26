# 0507: A successor inherits the consent that started it: amends 0493, 0503 and 0505

- **Status**: Superseded by [ADR 0560](0560-successor-session-in-the-same-thread.md): a handoff needs no authority, so there is no consent to inherit.
- **Date**: 2026-10-17
- **Amends**: [0493](0493-a-plan-cards-new-thread-pick-is-owner-consent.md), [0503](0503-build-chain.md), [0505](0505-named-handoff-checkpoints.md)

## Context

A child coding-agent thread was the successor of another child. The owner had
authorized that first handoff by picking `Implement in a new thread` on its
plan card (ADR 0493). At `/harden` the successor ran
`lucidos successor offer --at pre-harden`. The estimate said the handoff paid.
The verdict still said the handoff needed an owner approval card, which an
automatic checkpoint never raised. So the agent carried on with a large
context.

Nothing had recorded which authority let the first spawn through. The
`SuccessorThreadSpawned` actor is the API subprocess, so the actor alone never
says. The successor held no authority of its own: its predecessor opened its
turn, and it sat in no build chain.

Plan: `docs/plans/2026-10-17-successor-inherits-handoff-consent.md`.

## Decision

- **The spawn records its *handoff consent*.** `SuccessorThreadSpawned` gains
  an optional `handoff_consent` for a successor placed under a parent. The
  gate decides it from the authority that actually let the spawn through:
  - `owner`: the owner's device made the call, or the caller carried their
    standing instruction (a turn they opened, a trigger fire they authorized);
  - `owner-approval`: the spawn spent an owner approval, from an ADR 0387 card
    or an ADR 0493 plan card pick;
  - `carried`: the spawn passed on the consent its predecessor carried.

  A machine-token caller, a spawn the build chain's consent let through and a
  top-level successor record none.
- **The consent carries.** Thread X may spawn its successor under parent P
  with no owner approval card when all of these hold:
  - the spawn names a *handoff checkpoint* run (ADR 0505), as the build
    chain consent requires;
  - the handoff estimate, rerun for that checkpoint, says it pays;
  - X's predecessor recorded a `handoff_consent` on the event that started X;
  - X and its predecessor both sit under P.
- **One *standing consent* predicate** answers the build chain consent and the
  carried one. The context reading and the spawn gate both call it.
- **No consent means ask.** At an automatic checkpoint that pays but has no
  consent, the agent raises an owner approval card for `spawn-successor` on
  the parent. On Allow once it hands off; on Don't allow it carries on. The
  approval it spends is itself a recorded consent, so the next successor
  inherits it.

## Rationale

**The owner already decided.** They authorized the first handoff, and the
successor exists to carry on that same work. Asking again at every paying
checkpoint asks them to repeat a decision they made.

**The grant stays as narrow as the chain's.** It covers one verb,
`spawn-successor`, toward the thread's own parent. It holds only when the
estimate pays, and only for the parent's recorded successor. It is not a
standing instruction, so ADR 0168's "nothing is inherited" still holds for
every other verb.

**Decided at the gate, never inferred.** Reading an `OwnerApprovalSpent` row
near the spawn later would guess at which authority passed. The gate knows,
and the event is the source of truth across restarts.

**A paying handoff with no consent asks.** Carrying on silently gives up a
saving the owner may want. A card costs them one tap, and their answer is
recorded for the rest of the lineage.

## Consequences

- A child thread's successor hands off at a paying automatic checkpoint with
  no card, when the lineage began with the owner's consent.
- ADR 0503's "a child thread's automatic checkpoint gives up the saving rather
  than raise a card" no longer holds. It asks instead.
- Successors spawned before this change carry no record. At a paying
  checkpoint they ask once, and the successor that answer starts inherits it.
- No UI takes a carried consent back. The estimate bounds it, and the owner
  can still stop or archive the thread.
- "Handoff this session" names no checkpoint, so the carried consent never
  applies there.

## Alternatives considered

- **Infer the consent from `OwnerApprovalSpent` rows before the spawn.** It
  guesses, and a nearby approval for another verb or thread could match.
- **Make the consent a standing instruction for the successor.** It would lift
  every clause-4 verb, which is far wider than one handoff.
- **Keep carrying on at a paying checkpoint with no consent.** The thread then
  runs `/harden` on a context the estimate already called too expensive.
- **Put the consent in a projection column.** The event already holds it, and
  one read of the predecessor's event answers the check.
