# 0137: A trigger is never woken by an event its own fire emitted

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

An event a trigger's fire emitted could wake that same trigger. Nothing stopped
the loop except `MAX_EVENT_TRIGGER_DEPTH` (3), which ends it after three fires
rather than preventing it.

Shipped guidance turned that backstop into an authoring rule: a trigger must not
subscribe to an event class its own run emits
(`system-knowhow/triggers.md`). The rule is unfollowable for a whole legitimate
class of trigger. An idle detector has to watch every terminator event, and
`TriggerCompleted` is one of them. Being a trigger is what makes it emit that
event. So there was no correct subscription to write, and the only way to obey
was to not detect idleness at all.

The cost showed up in a live workspace. A calendar sync on a 15-minute cron woke
a broad-subscribed idle detector on every fire. In a 200-event
`TriggerExecuted` sample the detector outnumbered the source it was echoing. The
extra rows were the visible part. The real cost was a few hundred thread spawns
a day that nobody asked for.

## Decision

A trigger is never woken by an event its own fire emitted. `EmittedEvent`
carries `emitting_trigger_id` beside `depth`, read at emit from the
`ACTIVE_TRIGGER_ID` task-local, and `find_matching_event_triggers` drops that
trigger from the matches. **Amended: the marker crosses a process boundary
too, inside the signed origin token.** See "Amendment: the marker crosses the
process boundary" below.

## Rationale

The engine knows whose fire emitted a frame. The author has no reliable way to
express it, because the wake arrives on the same event type a real one would.
One comparison in the matcher holds the rule for every trigger, including ones
written before it existed.

It is a **per-subscriber gate**, the same class as the depth cap and a
`condition:` filter. It changes nothing about which event types are
subscribable, so ADR 0113's invariant I8 (one predicate serving both fan-outs)
is untouched.

It fails **open**. An absent marker suppresses nobody, which covers an ordinary
user turn, an HTTP handler, and work a fire hands off. An extra wake is
recoverable. A missing one is a trigger that silently never fires.

Quieting the event log is not the point, and would not have been worth it on its
own. The log is the workspace's memory. Every event is still written, still
reaches SSE, and still reaches every other subscriber. What goes away is one
thread spawn that was never wanted.

## Consequences

- The hard authoring rule in `system-knowhow/triggers.md` is retired.
  Broad-subscribe plus a cheap internal gate is a supported shape, and an
  *intent* trigger may subscribe to an LLM-activity event its own turn emits.
- **This supersedes one consequence bullet of ADR 0113**, which said a trigger
  subscribing to a frame its own run emits stops after `MAX_EVENT_TRIGGER_DEPTH`
  hops. It now never fires on its own frame at all. The rest of 0113 stands.
- The Thread Queue executor scopes `ACTIVE_TRIGGER_ID` around the whole fire,
  on both the cron arm and the event arm. The scope used to sit inside
  `execute_llm_task`, so a script fire emitted unmarked whichever arm ran it.
  The widening also covers the `TriggerExecuted` the executor records, which is
  the frame most likely to match a broad subscription.
- **The queue's own frames state their owner instead of reading the scope.**
  `ThreadQueued`, `ThreadQueueAdmitted` and `ThreadQueueCompleted` go out on
  tasks the fire does not own, so `EventBus::emit_as_trigger` takes the id the
  entry recorded. Without it `ThreadQueueCompleted` stayed unmarked at depth 0,
  and a trigger subscribed to it woke itself with nothing to end the loop.
- **Naming the owner also clears it where it does not belong.** A sub-thread is
  submitted inline on the fire's task. Its entry frames pass `None`, and so does
  the eager `MessageReceived` the executor emits for it. The child must not
  inherit the parent's marker, or the trigger loses the wake it asked for. That
  is the one direction no retry recovers.
- `ThreadQueueDropped` stays unmarked on purpose. A dropped entry never fired,
  so it emitted nothing to suppress, and a trigger should still hear that a fire
  of its was coalesced away.
- The depth cap keeps its job, narrowed to what it can still see: a chain
  running across triggers, where A's fire wakes B and B's fire wakes A.
- **The marker covers the fire, never what the fire hands off.** The
  discriminator is mechanical: a spawn that creates a new thread does not carry
  the trigger, and any other spawn does. So a bash or python tool inside a fire
  carries it, while a sub-thread or coding-agent session the fire starts emits
  unmarked. Suppressing those would break a trigger waiting on a session it
  started. A spawn that starts a new thread states `None` at its own call site,
  which is what keeps that exclusion deliberate. The tool spawns read the
  ambient scope through `build_tool_env_vars`, the one helper named for that
  case, because a tool IS the fire.
- The marker is broadcast-only. It describes the emitting task, not the event,
  so it is absent from the stored row and from every replay path.

## Alternatives considered

- **Persist the marker on the event row**, the way a `DomainEvent` persists its
  depth. Rejected: a self-wake chain is live by definition, so a replay has
  nothing to suppress. It would be a migration plus a column nothing reads.
- **Leave it to the depth cap.** Rejected: that is the status quo. It costs
  three fires per cycle, and the cycle repeats on every cron tick. Three fires
  of an opus intent trigger is a bill nobody meant to pay.
- **Tell workspaces to narrow the subscription.** Rejected outright: see
  Context. The broad subscription is the correct pattern for an idle detector,
  and an earlier sweep that flagged one as over-subscribed had that finding
  retracted.
- **Drop or coalesce the per-fire lifecycle events.** Rejected on two counts.
  `TriggerExecuted` is load-bearing, since `load_trigger_run_history` reads its
  `payload->>'last_run'` for the missed-cron catch-up. Making the
  `ThreadQueue*` frames transient would break ADR 0113's rule that persisted
  means subscribable, and event volume was never the complaint worth fixing.
- **Cap or throttle high-frequency triggers.** Rejected: running often is a
  supported shape by design, and a cap would turn a working trigger into a
  missed one.
- **A plain request header carrying the trigger id**, so a subprocess could
  state which fire it is. Rejected: a header is forgeable, and a forged one is a
  denial-of-wake vector. Any app reaching the API through the SDK could mute a
  trigger by claiming to be it. The claim rides the signed origin token instead,
  where it is authenticated. See the amendment below.

## Amendment, 2026-08-26: the marker crosses the process boundary

The Decision above reads as absolute. One arm was exempt.

`ACTIVE_TRIGGER_ID` is a task-local, and a task-local follows the await chain.
It does not survive a `fork`. So three emit paths arrived unmarked. A trigger's
script runs as a bash or python subprocess. The `lucidos` CLI runs as another
one. An HTTP POST to `/api/v1/events/emit` arrives on a request task that was
never inside the fire.

A trigger whose script emitted through the CLI could therefore still wake
itself, held only by the chain-depth ceiling. That is where everything stood
before this ADR. The defect is not only the extra fires. It is that the rule
reads as absolute while one arm is exempt. An author cannot tell from outside
which of their emits are covered.

**The trigger id rides the thread-bound origin token.** The token is
`<thread>@<depth>@<trigger>.<mac>`, a MAC over the whole prefix under a
per-startup secret. It is minted at one site and verified at one site, both in
`crates/lucidos-engine/src/api/actor.rs`. So the trigger claim is authenticated
exactly the way the source thread id already is. A `-` stands in for an absent
claim, and is signed like any other field.

**An app through the SDK holds no minted token, so it can express no claim at
all.** That is what closes the denial-of-wake direction a plain header would
have opened.

**The claim covers the fire, never what the fire hands off.** The Consequences
bullet states that rule, and it now holds across the process boundary too. What
the boundary adds is a lifetime. A background bash or python child outlives the
fire that spawned it, up to that tool's own timeout, and keeps the token it was
handed. Its later emits still count as the fire's, which is the same answer the
discriminator gives: the spawn started no thread.

Three limits are worth stating outright:

- **The chain depth rides the same prefix, and travels the other way.** ADR 0138
  put it there for the same reason and against the same forgery. The depth
  reaches the work a fire hands off, because a chain the fire started is still
  the chain. The trigger stops at that handoff. `subprocess_origin_env_vars` is
  the one call taking both. So a coding-agent spawn passes the depth and states
  `None` for the trigger, on one line a reviewer can see.
- **The claim reaches the event-emit surface only.** A script holding a token
  can call `lucidos notify`, and the `NotificationCreated` that follows is the
  engine's own bookkeeping rather than the script's emit. It stays unmarked.
- **Only two clients attach the token.** The `lucidos` CLI copies the env var
  into the header, and the Python shim patches `http.client` so urllib and
  requests do. A script shelling out to bare `curl` sends no header, so that
  emit is read as an ordinary external call and suppresses nobody. Fail-open,
  and the same gap `system-knowhow/thread-events.md` already records for
  attribution.

Fail-open is unchanged. A caller with no valid token yields no marker and
suppresses nobody.

Plan: `docs/plans/2026-08-26-a-trigger-never-wakes-itself.md`.
