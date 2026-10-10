# 0052: A thread subscription outlives Stop, and every other end of one is announced

- **Status**: Accepted
- **Date**: 2026-08-07

## Context

ADR 0047 gave a thread the ability to subscribe to an event, and listed four
ways one could be cancelled: the **Stop waiting** button, a thread-level
Stop/Cancel, archive and discard. ADR 0049 then removed the *attached* shape, so
a subscription no longer holds its thread's turn at all.

That left the Stop coupling behind, and it produced the incident this ADR
exists for. A watch armed at 00:08 died at 02:07 because the user pressed Stop
on an unrelated running turn. `cancel_chat` cancelled every subscription on the
thread, not just what the turn owned, and nothing anywhere said so: no toast, no
transcript line, and the indicator row simply gone. Archive was the same failure
mode and wider, because it cascades to every descendant.

The agent could not help either. It could arm a subscription and then had no way
to read or revoke one, so "is that still watching?" was answered from memory
(wrongly, by two hours, on 2026-08-06) and "stop watching" was answered with an
admission that the subscription was unrevokable.

## Decision

1. **Stop is turn-scoped.** It ends the running turn and touches no
   subscription. `EventWaitCancelCause::ThreadCanceled` is retired: kept in the
   enum and still deserialized, because rows carry it, but emitted by nothing.
2. **Ending a subscription is always announced.** Archive confirms first, naming
   every subscription the cascade would stop, its sub-threads included. Every
   stop, whatever caused it, leaves a line in the transcript at the moment it
   happened.
3. **The agent owns all three verbs on its own subscriptions**: `await_event`
   arms, `list_event_waits` reads, `cancel_event_wait` stands down, mirrored in
   the CLI as `lucidos await-event` / `event-waits list` / `event-waits cancel`.
   An agent stand-down gets its own cause, `AgentStandDown`.
4. **One genus, two species, in that vocabulary.** An **event subscription** is
   `{event_type, condition}` plus the shared matcher. A **trigger subscription**
   spawns a new thread on a match and stays armed; a **thread subscription**
   resumes an existing thread and is spent. The internal names (`EventWait*`,
   `await_event`, `live_event_wait_count`, the `event_wait` module) do not
   change.

## Rationale

**Stop owns the turn, and a subscription was never part of the turn.** That is
the whole argument, and ADR 0049 is what made it true: a subscription holds no
tokio task, no queue slot and no message-array slot, so there is nothing about
it for a turn-scoped control to end. The coupling was a leftover from the parked
shape, where a Stop genuinely did have to resolve the thing holding the turn
open. Keeping it meant one button silently doing two unrelated things, and the
one nobody asked for was the destructive one.

**A silent end is worse than no end.** The user's complaint was not that
subscriptions can be cancelled; archive cancelling them is correct, because
leaving one live behind the archive curtain would wake a thread they consider
closed. It was that a watch could disappear with nothing anywhere recording it.
So the fix is symmetric: the one path that had no business ending a subscription
stops doing it, and every path that legitimately does now says which
subscriptions and how. That is why the archive confirm names them rather than
counting them: "3 subscriptions" cannot be weighed, while "waiting for the
release build to finish" can.

**The transcript line is where the stop belongs, not only the indicator.** A
stop is the one resolution with no wake, so it is the only one that renders
nowhere by default: a delivery and an expiry both re-open the thread and read as
their own turn. The line goes at the resolution's own position rather than
flipping the row that armed it, because a subscription routinely outlives its
turn by hours, so that row is far up the transcript by the time anyone stops it.
`EventWaitCanceled` therefore carries what it stopped, exactly as
`EventWaitDelivered` carries what it matched and for the same reason: the record
has to be readable when the registration is outside the loaded window.

**A read the agent must pull beats state pushed into its context.** The obvious
alternative to `list_event_waits` is to inject the live set into every turn the
way other thread state might be. It was rejected because a snapshot is stale by
construction: a subscription can resolve mid-turn, through a delivery, the
10-second deadline sweep, or a catch-up scan inside a sibling `await_event`
call. A context line saying "you are watching X" that was true when the turn
started is the same failure the tool exists to fix, one layer up. The
system prompt instead routes the question to the tool, which is cheap, always
true, and exact at the moment of asking.

**Both new verbs are scoped to the calling thread, and the absence of an
argument is only the first of three legs.** The tools and the CLI expose no
thread parameter, so neither agent can express another thread. But the HTTP form
the CLI calls has a path segment where the argument is not, so the route also
refuses a request whose thread-bound origin token names a different thread than
the path, which is what turns the shape into a guarantee rather than a
convention. Third, a `wait_id` is itself scoped to its thread under the live
cache's own lock, so even a correctly-addressed call cannot resolve an id
belonging to somewhere else. The token check is deliberately confined to callers
that HAVE a thread: an untokened caller is the ordinary local API surface that
every other `/threads/:id/...` route already trusts, and moving that boundary is
a separate decision, not a side effect of this one. The argument-less shape is
also why the family stays a deliberate non-domain in the capability parity
manifest (ADR 0018), whose generators build a request out of declared args and
would have to turn the thread id into an ordinary flag.

**A partial stop is reported as a failure, not as a success.** `cancel_event_wait`
with `all` runs one emit per subscription, so one can fail while the rest land,
and a failed one is re-armed and will still wake the thread. Saying "nothing is
subscribed any more" there would be the exact lie this surface exists to stop
the agent telling, so the result names which ones are still running instead.

**The vocabulary was chosen because the shipped UI strings were already right
under it.** The panel header **SUBSCRIPTIONS**, the **Stop waiting** button and
the "What this thread is waiting for" label all read correctly once *event
subscription* is the genus, because only the thread species can appear on a
thread screen. A naming that required renaming the UI would have been the wrong
naming.

## Consequences

**Kept.** Archive and discard still cancel, so no subscription survives behind
the archive curtain. The matcher stays shared, so a `condition` that fires for a
trigger fires for a thread. No new table and no migration: `armed_at` rides the
existing `EventWaitStarted` payload, and rows without it fall back to the event
row's own `created`.

**Given up.** A single control that stops everything about a thread at once. A
user who wants the turn AND the subscriptions gone now presses two things, which
is the point. `POST /api/v1/chat/cancel` on an idle-but-subscribed thread
therefore reports `canceled: false` where it used to report `true`.

**Paid deliberately.** Two more tools in every chat turn's tool list, and 256
bytes on every Codex system prompt for the CLI entry (Codex learns the CLI from
its prompt rather than from `CLAUDE.md`). An archive of a subscribed thread is
one extra tap.

**Retired but readable.** `ThreadCanceled` stays deserializable forever. Events
are append-only, so dropping the arm would replay every pre-2026-08-07 row as
`Unknown` and lose why those subscriptions ended.

## Alternatives considered

**Leave Stop as it was and just add a toast.** Rejected: it treats the symptom.
The user pressing Stop on a running turn is not expressing anything about a
watch they armed two hours earlier, so telling them what they just destroyed is
worse than not destroying it.

**Make Stop ask first, like archive.** Rejected for a sharper reason: an archive
confirm is answerable ("I am closing this thread, and these three things go with
it"), while a Stop confirm interrupts an urgent action with an unrelated
question. Stop is pressed to make something stop NOW, and the honest fix is for
it to stop only what the user meant.

**Cancel subscriptions on any user message.** Already rejected in ADR 0047 (S6b)
and re-rejected here: it is the same class of silent loss with an even more
innocuous trigger.

**Inject the live subscription set into the agentic loop's context.** Rejected;
see the Rationale. It would be stale by construction and would need a per-thread
system prompt, which the chat prompt is not.

**Make the archive confirm fetch every cascade member's subscriptions so it can
name all of them.** Rejected in favour of naming what the client already has and
counting the rest ("2 more on sub-threads"). A dialog that waits on the network
before it can open is worse than one that is a line vaguer, and the thread the
user is actually archiving is always fully named.

**Give `EventWaitExpired` a transcript card too, for uniformity.** Rejected: an
expiry already wakes the thread and reads as its own turn, so a card would
report it twice. Only the resolution with no wake needs one.

**Rename the internal `EventWait*` family to match the new vocabulary.**
Rejected: the names are on disk in persisted rows and in a shipped tool name, so
a rename buys consistency in code comments at the cost of a migration and a
compatibility shim for a tool the model already knows.
