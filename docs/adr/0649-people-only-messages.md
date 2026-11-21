# 0649: People-only messages, the agent pause and the People row; work item retired

- **Status**: Accepted
- **Date**: 2026-11-10

Amends [ADR 0623](0623-team-workspace-factory.md) decisions 4 and 7. Plan:
`docs/plans/2026-11-10-people-only-messages.md`.

## Context

In a *team workspace*, people could not talk inside a thread without the agent
answering. Every `MessageReceived` starts a turn. In a chat thread the Lucidos
Agent replies, and in a coding-agent thread the message becomes a prompt. So
the team took its talk about the work to a chat tool, outside the thread.

ADR 0623 also named a root thread and its sub-threads a *work item*. It was
never an entity of its own, yet it had its own routes, SDK namespace and term.

## Decision

1. **Each message is addressed by mentions.** A *person mention* (`@Name`, excluding the sender)
   pushes that operator and makes them a watcher of the root thread. The
   *agent mention* (`@agent`) wakes the agent. One message may carry both.
2. **The agent pause is shared thread state, held in events.** A message
   that mentions only people records `AgentPaused` and stays between people.
   While paused, an untagged message stays between people too, except a
   reply to an open question card, which is the card's answer. `@agent`
   records `AgentResumed` and wakes the agent. **Resume** on the hint line
   ends the pause by hand, through `POST /threads/:thread_id/agent-pause`, and
   the event records cause `chip`. Pausing never stops a running turn.
3. **A people-only message is its own event, `PeopleMessagePosted`.** It is
   metadata and never starts a turn. The engine alone decides wake or not,
   from the mentions and the pause state. Only device-origin human sends take that route. Other input (trigger fires,
   child reports, event-wait deliveries, API callers, question answers)
   always wakes the agent.
4. **The agent sees everything, with names.** People-only messages are
   context on the next woken turn. On a team gateway every human message in
   the history carries its operator's name. A coding-agent session gets the
   paused messages once, at the head of its next woken prompt.
5. **A mention stores the operator id.** It names who is *addressed*, not who
   acted, and the operator id is the only stable handle for a person.
6. **A core People row on the thread header**, team gateways only. It shows
   the root thread's driver and watchers as round initials ("Sam drives"),
   with Watch, Unwatch, Take and Release. This amends ADR 0623 decision 7: a mention makes someone a
   watcher without their action, so leaving must be one tap from the thread.
   The cross-thread views stay in the Factory app.
7. **The term *work item* is retired.** A root thread and its sub-threads is
   what it always was. This amends ADR 0623 decision 4. The routes become
   `/api/v1/threads/people` and `/api/v1/threads/:thread_id/people`, the SDK
   namespace `lucidos.threads.people`, and the Rust module
   `engine::thread_people`. The stored event types (`WorkItemDriverTaken`,
   `WorkItemDriverReleased`, `WorkItemWatched`, `WorkItemUnwatched`,
   `WorkItemGateEscalated`) keep their names as legacy spellings.
8. **The pause shows as one hint line, never as a mark on each message.**
   While paused, a line above the composer reads "Agent paused. Mention
   @agent or Resume". While the agent listens, nothing draws. A people-only
   message carries no marker, since it always sits between an "Agent paused"
   card and the next "Agent resumed" card.
9. **The client asks the engine who is a person and whether the agent is
   paused.** `GET /operators/people` answers the people, you, and whether the
   gateway is a team. `GET /threads/:thread_id/agent-pause` answers the pause.

## Rationale

Mentions are plain prompt text, so pausing and resuming work from the prompt
(prompt-first). A per-message decision keeps one agent per thread, which a
coding-agent session needs. A separate event makes "a people message started a
turn" impossible by type, rather than by a flag every consumer must check.

The pause lives in events so it survives an engine restart and reads the same
for everyone. Names resolve from the operator snapshot at read time, so a
renamed operator reads right in every new prompt (ADR 0297).

The list route sits at `/threads/people`, not on `GET /threads` with a query.
App reach is per route, and `GET /threads` is host-only. Opening it to apps
would expose the whole thread list.

People come from the engine's operator snapshot, not the browser's gateway
list, because the engine merges a relayed person into a local one by email.
The pause gets its own read route because the transcript loads in pages: the
last pause flip may sit on a page the client never loaded.

No compatibility shim keeps `/work-items` alive. The API was weeks old, with
one known caller: the Factory app template, which a team copies again.

## Consequences

- A gateway with one operator sees no change: no hint line, no People row,
  no mentions, no new events, and the history still says "User".
- A people-only message pushes its mentioned people plus the driver and
  watchers, minus the sender's person, under one tag per thread. The push
  offers Unwatch, except to the driver, who cannot unwatch.
- A message that wakes the agent and mentions people pushes only those
  people, minus the sender.
- Mention side effects (watching, pushes) run after the message is
  acknowledged and recorded. A slow push never delays the send, and a
  message that never records tells nobody.
- A person's message waits for any message admitted before it on the
  thread, so a mention cannot overtake an `@agent` sent first.
- On a team gateway a message names its writer by the device the gateway
  authenticated, never a device id the request body claims.
- Until the gateway names its people after an engine start, a send it would
  route as a team is refused with a 503 (`operators_unknown`). That covers a
  send with mentions and one to a paused thread. So does the chip.
- Anyone can resume the agent. A mention, a role or the pause is never
  authority (ADR 0623 decision 1).
- A Factory app copied before the rename stops working until it is copied
  again.
- Stored events never change, so the `WorkItem*` type names stay in the
  events table, the generated TS types and `thread-events.md`.

## Alternatives considered

- **A thread recipient list beside driver and watchers.** Lost: a second
  membership concept, and it still needs a per-message answer to "does the
  agent reply?".
- **A field on `MessageReceived` saying "do not reply".** Lost: every
  consumer of `MessageReceived` would have to check it, and one that forgot
  would start a turn.
- **The model deciding when to chime in.** Lost: nondeterministic, and a
  person cannot tell in advance whether the agent will answer.
- **Several agents in one thread ("agents as bots").** Deferred, as a
  direction rather than a decision. It breaks one agent per thread, since a
  coding-agent thread is one session. Reaching another agent stays a child
  thread.
- **Renaming the stored event types.** Lost: events are immutable, and rows
  already carry the old names.

## Delivery recovery

Mention intent is persisted before the send acknowledges and is released only
when its message event is recorded. This includes messages that wake the agent.
A worker drains recorded intent at startup and during normal operation, waiting
for the operator snapshot. A database lock serializes workers. Auto-watch is
idempotent, and the resolved device audiences and driver actions are frozen
before watch events and notifications are emitted. Thread deletion removes pending intent.

Recovery retains person scoping after membership shrinks, including before the
first delivery plan is frozen (the exception in ADR 0623, decision 6).

Each message/person pair has one stable notification identity. The frozen plan
retains the recipient key and devices. One person handling a toast cannot
suppress another person's OS push.

Notification creation
and awaited push completion have separate durable receipts: recovery skips
completed dispatches, rather than treating NotificationCreated as proof of a
push. Web subscriptions complete independently.

Native completion follows the
client acknowledgement of OS acceptance or in-app handling. Native delivery
requires an explicit positive acceptance result from updated clients. Older
clients returning no result retain their existing successful-completion semantics.
They can still lose a banner rejected by the OS; preserving that compatibility
was an explicit product decision.

The first presence
decision is retained, so retries do not repeat in-app checks.
This closes the acknowledgement-to-task and notification-to-task gaps.

External push transport is not an exactly-once protocol. A crash after external
acceptance but before the local completion receipt can retry the same identity.
Existing notification tags replace the same visible notification rather than
stacking it; a transport retry can still alert again. Recording completion before
transport would silently lose the push instead. No startup sweep can remove this
external ambiguity without an end-to-end receiver receipt.
