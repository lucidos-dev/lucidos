# 0236: A queue the engine drains from memory keeps its membership in the event store

- **Status**: Accepted
- **Date**: 2026-09-21

## Context

A chat follow-up typed while a turn runs is emitted as `MessageReceived` and
then handed to that turn's in-memory injection channel. The agentic loop drains
the channel at its next round boundary.

An engine restart killed the channel, and nothing on any resume path read the
row back. The message was then named by no `UserPromptInjected`, no
`QueuedMessageRemoved` and no response. It rendered as a "Queued" bubble pinned
to the transcript that only the user's trash icon could clear. Dozens of such
rows accumulated over 60 days in one workspace.

Engine Statelessness (CLAUDE.md) already said in-memory state is cache. The
channel looked like it obeyed that rule, because the message really was durable
before it ever reached the channel. What was NOT durable was a second thing
nobody had named: the answer to whether the loop had consumed it.

## Decision

A queue the engine drains from memory must have its **membership** derivable
from the event store. The in-memory structure carries delivery and wakeup only.

Concretely, for the chat injection queue: `chat::queued_recovery`'s
`STRANDED_QUEUED_MESSAGES_SQL` is the single definition of "still owed an
answer", and every resume asks it.

## Rationale

The rule is about a distinction the original design blurred. A queue holds two
things: the ITEMS, and the CURSOR saying how far the consumer has got.
Persisting the items is not enough. An unrecoverable cursor loses them just as
completely, and it loses them silently, because each item is still sitting in
the database looking fine.

The cursor was already written down here, in three markers nobody read: a
`UserPromptInjected.injected_message_id` (consumed), a
`QueuedMessageRemoved.removed_message_id` (retracted), and any event's
`request_event_id` (a turn picked it up). Turning those into a query is
cheaper than inventing a queue table. It also cannot drift from the timeline
the user sees, because it IS the timeline.

The derived form buys idempotency for free. Announcing a recovered message
WRITES the marker that excludes it. A double Continue, a resume racing the boot
sweep, and a resume that is itself interrupted therefore all converge. No
separate "already handled" flag has to be kept honest.

What cannot move is the wakeup. A database row cannot un-park a tokio task, and
`injection_notify` is what pulls `bash_output(wait_secs=120)` out of a
two-minute wait when the user types. Polling the database instead is the storm
that counter exists to prevent. So the rule binds membership, not delivery.

## Consequences

- **A new in-memory queue owes a membership query.** If its items cannot be
  identified from persisted events, that is a signal the events are missing,
  not that the rule should bend.
- **Ask it where memory cannot answer, and nowhere else.** The chat queue's
  one reader is the resume, because that is where the channel is gone. Adding a
  reader somewhere the channel still answers is not free: see the rejected
  alternative below.
- **A cost per read.** Two small indexed queries per resume. Not per turn, not
  per round, and not per injected message.
- **Only `UserText` is covered today.** The three other `InjectedPromptKind`s
  each have a persisted counterpart of a different event type with its own
  consumed-marker, so covering them means four predicates. Each already has its
  own recovery path. Widening this is a separate change with its own argument.
- **Rows stranded before the fix stay stranded.** Each reader's window opens at
  the turn that owns the drain, so an older orphan falls outside it. Answering a
  weeks-old message in a conversation that moved on is worse than the bubble,
  and the trash icon is the sanctioned clear.

## Alternatives considered

**Read the query at the turn tail too, unioned with the channel drain.** Built,
and removed at hardening. It looked like the general form of the rule, and it is
where the "two definitions can disagree" worry points. The contract it would
have joined is what refutes it.

`finalize_turn_and_drain_injections` drops the guard under the same lock the
send takes, so a message is either buffered or refused. A buffered one is in
the drain; a refused one is started as a fresh turn by its own caller. There is
no third case, so the query's whole yield there is the second set, and
re-submitting it answers each message twice.

The lesson generalizes, and it is why the decision says membership rather than
"query everywhere". A structure whose contents ARE recoverable in-process needs
no second opinion. Ask the store where the in-process answer is gone.

**Move the whole injection queue into a table.** A real queue with rows, a
claimed flag and a delete. Rejected: it duplicates state the events table
already holds, and the duplicate can disagree with the timeline the user reads.
It also does not remove the in-process wakeup, so the complexity buys nothing
the query does not.

**Ask the query at the loop's round boundary too, replacing `try_recv`.** The
purest form of the rule. Rejected on cost and on benefit: it runs once per round
per running thread, and the two readers above already guarantee nothing is lost.
It would also split ordering across two sources, since the other three prompt
kinds have no query form.

**A backfill migration for the stranded rows.** Rejected. Fabricating the
ingestion of a weeks-old message is the same mistake as fabricating a response.
The detection query is recorded in the plan for an operator who wants a census.
