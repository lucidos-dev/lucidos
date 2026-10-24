# 0503: A plan card's new-thread pick starts a build chain under the planner: amends 0474 and 0493

- **Status**: Superseded by [ADR 0560](0560-successor-session-in-the-same-thread.md): a plan card's pick starts a successor session in the planning thread, so there is no build chain.
- **Date**: 2026-10-16
- **Amends**: [0474](0474-successor-threads.md), [0493](0493-a-plan-cards-new-thread-pick-is-owner-consent.md)
- **Amended by**: [0507: A successor inherits the consent that started it](0507-successor-inherits-handoff-consent.md), [0545: A no-wake child files every report](0545-no-wake-child-files-every-report.md)

## Context

Two coding-agent threads ran their whole build and `/harden` in one context.
One reached 448k tokens at `/harden` start, the other about 280k. Each call
re-reads the whole context, so `/harden` alone cost them 30.6M and 12.9M cache
reads. Most of those calls only polled the suites.

A handoff before `/harden`, or between plan phases, would have paid for itself.
Two things stood in the way:

- **Placement.** ADR 0474 puts a successor in its old thread's place. A child
  thread's successor sits under the parent, which is an act in the parent's
  subtree. From a turn the owner did not open, that needs an owner approval
  card, so an automatic handoff would ask the owner at every checkpoint.
- **Grouping.** The plan card's new-thread pick put the builder in the
  planner's place, top-level for a top-level planner. Each later successor was
  another top-level thread, and nothing grouped the plan with its build.

Plan: `docs/plans/2026-10-16-automatic-handoff-checkpoints-and-build-chain.md`.

## Decision

- **The plan card's new-thread pick starts a *build chain*.** The builder is a
  child of the planning thread, with wake mode `no-wake`. It carries the branch,
  the plan marker and the proposal, as any successor does. The planner stays
  open, does not archive and proposes nothing.
  `SuccessorThreadSpawned` records `starts_build_chain`.
- **Only a top-level planner starts one.** A child planner's successor keeps
  its place under the parent, so that parent still receives the report. The
  planner would otherwise stay idle, owing a report nothing ever sends.
- **The builder's successors land as siblings under the planner**, through
  ADR 0474's placement rule. They archive as today once their successor starts.
  A thread belongs to the chain when its `successor_of` links lead, through
  threads under the same parent, to that parent.
- **Standing consent.** A chain member may spawn its successor under the
  planner with no owner approval card. The consent covers one verb,
  `spawn-successor`, and one target, the chain's own planner. It holds only
  when the engine's *handoff estimate*, rerun for the moment the spawn names,
  says the handoff pays.
- **Every other handoff keeps its place.** "Handoff this session", a repo
  checkpoint card and a top-level thread's automatic handoff still put the
  successor in the old thread's place. A child outside a build chain still
  needs `spawn-successor` consent, so its automatic checkpoint carries on.

## Rationale

**The grant adds no power.** The planner could spawn each chain member itself,
since a thread acts in its own subtree (ADR 0168). The consent lets a chain
member do what its planner may already do, for one verb and one target.

**The estimate bounds the churn.** The consent is not "any handoff". It holds
only when the estimate says the handoff saves more than it costs. A thread
still hands off once, so a chain grows by one member per paying checkpoint.

**The pick needs no card any more.** The builder sits in the planner's own
subtree, so ADR 0493's owner consent is no longer spent on the plan card. It
still covers a new-thread pick on a repo checkpoint card in a child thread.

**`no-wake` keeps the planner quiet.** A clean finish of a chain member files
its report without opening a planner turn. A non-clean finish still wakes the
planner under any wake mode, as `Delivery::for_report` decides.

## Consequences

- The planner groups its chain in the thread list, and holds the plan's
  conversation for the user to come back to.
- The planner's own work is held as carried by its successor, as ADR 0474
  holds any handed-off thread's work.
- An automatic handoff in a build chain asks the owner nothing. Outside one, a
  child thread's automatic checkpoint gives up the saving rather than raise a
  card.
- The planner is not archived when its chain's change applies. The user
  archives it.
- A child planner gets no chain. Its successors hand off automatically only
  when they hold `spawn-successor` authority of their own.

## Alternatives considered

- **A single-use consent per handoff.** It raises an owner approval card at
  every automatic checkpoint, which is what the automatic checkpoints exist to
  avoid.
- **Keep top-level successors for the plan card.** Nothing groups the chain,
  and each successor of a child planner still needs a card.
- **Let any child hand off under its parent with no card.** That lifts every
  child to an act in its parent's subtree. The chain consent is narrower: its
  planner already holds that power over the whole chain.
