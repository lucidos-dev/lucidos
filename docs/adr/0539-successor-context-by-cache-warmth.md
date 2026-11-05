# 0539: A successor's context comes from the old agent only while its prompt cache is warm; a cold cache starts the successor from a handoff brief

- **Status**: Accepted
- **Date**: 2026-10-20
- **Amends**: [0505](0505-named-handoff-checkpoints.md)
- **Amended by**: [0560: A handoff starts a successor session in the same thread](0560-successor-session-in-the-same-thread.md)

## Context

The composer's *successor strip* offered "Continue in a new thread" on a large
thread. The tap sent "handoff this session" into that thread. The old agent
then ran a full-context turn to write the *handoff narrative*, which is the cost
the strip warns about. On a cold prompt cache the whole context is written
again at the cache-write price.

The *handoff estimate* hid this. It priced the narrative's output only, never
its reread of the old context. It also priced staying as if the cache were
always warm, though a cold cache makes the next call rewrite the context.

The strip's other option, "Move to a new thread", opened a plain draft with no
branch and nothing learned.

Plan: `docs/plans/2026-10-20-successor-card.md`.

## Decision

- **Warm cache: the old agent writes the narrative.** Its context is cheap to
  reread, and the narrative is the best carry-over.
- **Cold cache: the successor starts at once from a *handoff brief*.** No model
  call runs on the old thread. The brief names the old thread, and the
  successor rebuilds only what it needs from the event log and `git log`.
- **The engine picks, at the tap.** The browser sends no path. The engine reads
  the thread's cache warmth when the request arrives.
- **The estimate prices both.** Warm: the handoff adds the narrative turn's
  reread of the old context at the cache-read price. Cold: the handoff costs
  only the successor's own context, and staying adds one rewrite of the old
  context at the cache-write price.
- **One trigger.** The *Successor card* shows at any clean idle where the
  estimate pays. The fixed 300k nudge goes.
- **Stay here is a thread event**, `SuccessorOfferDeclined`. It holds the card
  back until the thread has grown 100k tokens, and a compaction lifts the hold.

## Rationale

**Warmth is the price.** The same thread costs a few cents to reread warm and
dollars to rewrite cold. A flow that ignores warmth goes wrong both ways. It
wakes a cold giant to write a summary. Or it skips a narrative that a warm
cache would give for almost nothing.

**A cold successor can rebuild.** The old thread's events and its branch are on
disk. A successor that reads the last turns and `git log` recovers the working
state at a fraction of the cold rewrite.

**The engine alone knows warmth.** It is computed from the capture timestamps in
SQL, so no client clock or cached flag decides it.

## Consequences

- A cold successor starts with less than a narrative carries. It asks the user
  where to pick up when no request was typed.
- Plan cards and checkpoints read the warmth-aware estimate too. Their verdicts
  move for large contexts.
- The card's history pool is its own checkpoint name. Until it holds three
  samples, the default call count applies.

## Alternatives considered

- **Always write the narrative on the old thread.** It is today's flow. A cold
  large thread pays a full rewrite to produce a summary.
- **Summarise cold threads with a small model.** It adds a model call and a
  second summary format. The successor rebuilding its own context is cheaper
  and keeps one path.
- **Let the browser choose the path.** It breaks the rule that the client sends
  intent and the engine decides, and a stale tab would pick on stale warmth.
