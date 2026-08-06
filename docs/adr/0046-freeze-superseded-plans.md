# 0046: Freeze a superseded plan and let the glossary carry current truth

- **Status**: Accepted
- **Date**: 2026-08-05

## Context

An implementation plan under `docs/plans/` records the decisions behind a change
before the first edit. It is written once, approved once, and the work follows it.

That shape assumes the design settles. Sometimes it does not. The navigation focus
marker was repainted and then retuned three times in one day, each round on direct
user feedback rather than on anything discovered in the code: an outline frame
became a background highlight; the highlight gained a real turn-on and a slower
dissolve; the turn-on then lost its overshoot, the levels rose, a hold was added and
the dissolve slowed again. Each round invalidated numbers, and one round invalidated
a mechanism and an explicit non-goal, in
`docs/plans/2026-08-05-nav-focus-marker-spotlight-highlight.md`.

The first two rounds amended it in place. By the third the file had accumulated two
supersession notes, an invariant marked RESTATED, one marked ADDED, and a non-goal
contradicted by a later note, while still asserting five superseded numbers and a
deleted keyframe as current fact. Review caught the drift each round, so it was
never wrong for long, but it was wrong again every round.

## Decision

When a plan has been overtaken by later decisions to the point where amending it
again would be the third or later correction, **freeze it**: add a header saying it
records the original decision only, enumerate what is superseded, and name the
surface that carries current truth. Do not delete it, and do not keep amending it.

Current truth lives where it already had to be maintained: the **glossary entry**
for user-meaningful concepts, plus the rules and guard tests at the code.

## Rationale

A plan is a record of a decision at a point in time, not a live specification. Once
it is being edited to stay true, it has quietly become a second copy of the docs,
and a worse one: nobody reads a shipped plan to learn how something behaves, so its
errors are found late and by reviewers rather than by readers.

The glossary is the opposite. `.claude/rules/glossary.md` already makes it normative
and already requires it to move with the code, so it is maintained under a rule that
exists independently of any one change. Pointing at it costs nothing extra and puts
the reader somewhere that is kept honest by default.

Freezing is also a truthful signal. A plan with three amendment notes still *looks*
current at a glance; a frozen header says plainly which parts are history, which is
what a reader actually needs from a dated file.

## Consequences

- A frozen plan keeps its full historical value: the reasoning, the alternatives,
  and the invariants as they stood are all still readable, which is the whole point
  of keeping plans.
- The freeze header carries a real obligation. It must enumerate what is superseded
  specifically rather than just saying "outdated", because an unenumerated freeze is
  worse than an amendment: it neither corrects the text nor tells you which parts to
  distrust.
- The glossary entry inherits the burden. If it drifts there is no longer a second
  place to check, which is the intended trade: one maintained surface beats two
  half-maintained ones.
- The planning *gate* is untouched. A frozen plan still satisfies the marker for the
  work it covered; freezing is about the document's ongoing accuracy, not about
  whether the change was allowed to land.

## Alternatives considered

**Keep amending.** Rejected on evidence: three rounds produced three rounds of
drift, each caught by review rather than prevented. The cost grows with every
amendment, because a reader must now reconcile the body against a stack of notes.

**Delete the plan once superseded.** Rejected. It throws away the alternatives
considered and the reasoning, which is most of what a plan is worth later, and it
would make the ADR and glossary references dangle.

**Write a fresh plan per round.** Rejected for this shape of work. These rounds were
one-line user reactions to something visible on screen, not designs with open
decisions; the `implementation-plan` skill exists for settled decisions that need
sequencing, and a plan per tweak would be ceremony that produces more stale files,
not fewer.

**Make the plan the live spec and drop the glossary entry.** Rejected. Plans are
dated and filed by date, so there is no path from a concept to its current plan;
the glossary is indexed by the concept's name and is already the thing other rules
point at.
