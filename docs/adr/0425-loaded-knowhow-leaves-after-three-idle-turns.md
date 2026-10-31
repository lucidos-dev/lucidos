# 0425: A loaded knowhow doc leaves the prompt after 3 finished turns that do not load it; the set is derived from events, a re-load of a loaded doc keeps it without a body, and unload_knowhow drops one

- **Status**: Accepted
- **Date**: 2026-10-11
- **Amends**: [0085](0085-model-curates-what-survives-round-one.md)

## Context

A doc loaded with `load_knowhow` stayed in the thread's `[LOADED KNOWHOW]`
block for the thread's whole life. Nothing removed it: no limit per doc, no
expiry, and a replay after a restart that put every doc back.

That was tolerable while threads were short. Home never ends and cannot be
archived, so every user now has a thread that keeps every doc it ever loaded.
One Home thread carried four system docs, about 197K chars (76K tokens), on
every turn, for tasks finished many turns before.

A second copy hid in the resume tool blocks. They pinned every `load_knowhow`
pair for the thread's life. A pair was stubbed only while its doc was in the
loaded set. So removing a doc from the set alone would have put its full body
back through the resume blocks.

ADR 0085's amendment already named the standing knowhow keep as a design error:
the one thing the model chose and could not un-choose. ADR 0109 answered it for
tool results with keeps that last one round. The knowhow block was the part
left over.

## Decision

- **A doc stays while one of the last 3 finished turns loaded it, or a later
  one did.** A finished turn is one `ResponseGenerated`. The limit is
  `IDLE_TURNS_BEFORE_UNLOAD` in `engine/loaded_knowhow.rs`.
- **A re-load keeps a doc cheaply.** `load_knowhow` on a doc that is in this
  turn's block, with an unchanged body, returns a short kept result and no
  body. A changed body, or a doc loaded only earlier in the same turn, gets the
  full body as before.
- **`unload_knowhow` drops a doc the agent is done with.**
- **A doc leaves only between turns.** Eviction and unload both apply when the
  next turn builds its block.
- **The loaded set is a pure fold over the thread's events**, run each turn:
  `load_knowhow` and `unload_knowhow` pairs, and `ResponseGenerated`. The
  in-memory store and its replay are gone.
- **Resume pairs for `load_knowhow` are no longer pinned.** They follow the
  ordinary tail, and a body in one is always stubbed.
- **The block and the Context viewer row carry one note per doc** saying how
  many more turns it stays.

## Rationale

**The finished turn is the unit the prompt is built in.** The block is built
once per turn and fixed for its rounds. A question the agent asks and the user
answers stays inside the turn, and so does a message typed mid-turn. Counting
`MessageReceived` would count those injections, and would age a doc in the
middle of the work it was loaded for.

**Only a finished turn ages a doc.** A Stop, an abort or a failure means the
work did not finish. Counting them would let a provider outage or a crash loop
empty the block exactly when the agent most needs it back.

**3 comes from the dev workspace's own history.** Of 39 re-loads of a doc on
the same chat thread, 36 came within 3 finished turns. After its last load, a
thread ran a median of 3 more finished turns. Over 967 loads, the tier cost
37.5M input tokens. A limit of 3 cuts that to 13.9M, 5 to 19.4M, and 8 to
24.2M. Going from 3 to 5 spends 5.5M tokens to save one re-load in 39.

**A wrong eviction costs one tool call.** The doc is still in the routing list.
The agent calls `load_knowhow` again and gets the body once, then the doc is
back in the block. That is about the price of keeping it one more turn.

**The agent can see the clock.** Each doc's note says how many more turns it
stays, and the block header says how to keep it. So a doc the agent still
follows does not leave by surprise, and keeping it costs no body.

**The eviction is free in cache terms.** The block follows the conversation
history in the user message, and that message is rewritten at every turn
boundary (ADR 0085 measured 100% of boundaries). An eviction or a changed note
adds no cache write. Nothing changes inside a turn, so the rounds of one turn
still share their prefix.

**Deriving the set from events makes restart safety structural.** Events are
the authority. A cache beside them needed a replay, a thread LRU and a
needs-replay flag, and each was a place for the two to disagree. The classic
history path already reads every event of the thread each turn. The Tree path
and the tool handler run a narrow query for the four relevant shapes.

**The pin carried the body before the block existed.** With the block holding
every loaded body, a pinned pair is only a stub that grows with every load the
thread ever made.

## Consequences

- A long task keeps its doc only if some turn in each window of 3 loads it.
  The note tells the agent when that is due.
- A doc loaded mid-turn shows in the viewer's Loaded knowhow tier from the next
  turn. Until then its body is in the live tool result, which is what the
  prompt carries.
- Every turn on the Tree path runs one more query, filtered to load, unload and
  turn-end events.
- `building-knowhow.md` no longer tells authors a doc stays for the thread's
  life.

## Alternatives considered

- **A token budget for the tier.** Over budget, it must drop something.
  Dropping a doc used in the last few turns breaks the mid-task promise.
  Dropping only idle docs repeats what the turn rule does within 3 turns. A
  second rule for little gain.
- **Wall-clock expiry.** A thread left overnight would lose its doc, although
  no work happened. A burst of turns would keep it no longer than a slow hour.
  Time is not the unit the prompt is built in.
- **Passive relevance** (the doc id in a reply, tools in the doc's domain). A
  classifier between the agent and its context, which ADR 0111 rejected for
  app knowhow: it fails on exactly the turns where the doc matters.
- **Unload only, no automatic eviction.** The agent rarely cleans up after
  itself; the Home thread shows it.
- **Keep the in-memory store and update it per turn.** Two implementations of
  one rule, an incremental one and a replay, which must agree forever.
- **Keep the pin with an "unloaded" stub.** A list of stub pairs that grows
  with every load for the life of the thread, for no reader.
