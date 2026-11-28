# 0755: One open card per thread: later cards queue, and engine reviews post into regular threads

- **Status**: Accepted
- **Date**: 2026-11-20

## Context

Several producers raised cards on their own: step cards, start cards,
unattested acts, grant-file reviews, blocked triggers and agent permission
prompts. Nothing stopped two of them from showing in one thread at once.
Parallel agent calls with different inputs opened parallel permission cards.

The trigger grant review also built its own thread, with a fixed opening
message and title. The owner asked for one card at a time in every client, and
for no special engine threads.

## Decision

A thread has at most one open card. The EventBus records a later
`UserQuestionAsked` as `UserQuestionQueued`, a *queued card*, and posts it with
the same `tool_use_id` once the open card closes. Agent permission prompts wait
for a free slot instead. A card posts only in the thread whose work waits on
it. No card goes to the home thread, and the engine makes no thread to host
one. The trigger grant review is a Lucidos Agent thread that asks one trigger
per card.

## Rationale

- **The gate sits in Validate, under the per-thread append lock.** Every
  producer passes there, and the lock makes "is a card open?" race-free. No
  producer needs its own check for correctness.
- **A queued card is a new event type, not a flag.** Every existing consumer
  (projection, push, voice, lifecycle status, clients) ignores the new type. So
  a queued card is invisible by construction, and no client folds anything.
- **The queue lives in events.** A bus subscriber, `engine/card_queue.rs`,
  posts the oldest queued card when the slot frees. A startup sweep runs the
  same release, so the queue survives a restart with no in-memory state.
- **Agent permission prompts never queue.** Each one blocks a live agent call
  that a restart kills anyway. So the site waits in memory and emits again
  when Validate refuses it with `CardSlotTaken`.
- **A card belongs to the work that waits on it.** A blocked trigger run's card
  sits in the run's own thread, and an agent's ask sits in the agent's thread.
  The home thread and a dedicated review thread were both tried as hosts. Each
  became a second place to look, with no work behind its cards.

## Consequences

- A queued agent card whose turn moved on is answered `Superseded` instead of
  posted. A queued engine card (`permission_request` set) waits until posted.
- The owner cannot answer a queued card. Only the engine may retire one, with
  `Canceled` or `Superseded`.
- A waiting agent prompt and a queued card contend for a freed slot. The bus
  admits whichever emits first.
- Threads holding several live cards at upgrade keep them. New cards queue
  until all of them close.
- Grant files no longer raise Import or review cards: boot imports each file
  once, then deletes it (ADR 0677).
- **The next engine review is an agent conversation too.** A release notice
  action sends a seeded prompt, and the agent asks with prepared permission
  requests. Do not add an engine-made thread to host cards. The landing lane
  thread is not this kind: it runs an agent turn.
- Amended by `docs/plans/2026-11-21-engine-cards-and-trust-design.md`, which
  replaced the home-thread review.

## Alternatives considered

- **Fold extra cards in each client.** Rejected: every client (desktop, PWA,
  push, voice) would need the same rule, and push would still fire per card.
- **A check in each producer.** Rejected: check-then-raise races, and the next
  producer forgets it. The start card already raced this way.
- **Persist waiting agent permission prompts.** Rejected: the agent call they
  answer dies with the process, so a durable prompt would outlive its caller.
- **A "N more waiting" hint on the card.** Deferred: it needs a design pass,
  and the owner asked for no special view.
