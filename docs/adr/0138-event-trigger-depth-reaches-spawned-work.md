# 0138: The event-trigger chain depth reaches the work a fire hands off, and its ceiling is a policy field

- **Status**: Accepted
- **Date**: 2026-08-26

## Context

`EVENT_TRIGGER_DEPTH` is a tokio task-local. It follows an await chain and it
does not follow a `tokio::spawn`. Every path where a trigger fire handed work to
another task therefore restarted the chain at zero.

Measured with a probe executor, with the submit wrapped in
`EVENT_TRIGGER_DEPTH.scope(2, …)`: `prepare` read depth **2**, `execute` read
**0**. The boundary is `ThreadQueue::spawn_execution`.

The hole was a class, not one field. `ThreadQueueRequest::EventTrigger` was the
only variant carrying a depth, so `SubThread`, `CodingAgent` and `AgentChat` all
lost it at the same boundary. A script fire's `lucidos events emit` lost it too,
on the axum request task.

Scoping the task-local at that one boundary is not enough. `SubThread` spawns
again in `spawn_thread`, and a coding-agent session spawns repeatedly:
`CodingAgentIdled` is emitted from `run_session`, `apply_now`, `idle_snapshot`,
`external_watchdog`, and from `agent_recovery` after a restart. A site missed
reads zero silently, which is the failure ADR 0133 rejected a task-local for.

## Decision

**The depth is carried explicitly, and every task boundary re-establishes it
from that carried value.** Three carriers, one per kind of boundary:

- **The persisted queue request**, for the queue's own spawn. All four
  spawn-bearing `ThreadQueueRequest` variants carry a `depth`, stamped once in
  `submit`. `Cron` carries none: it roots a fresh chain.
- **A per-thread registration**, for work that spawns again inside itself.
  `EventBus::emit` resolves a thread event from the deeper of the task-local and
  that registration. It is keyed by thread but owned per queue entry, so one
  entry completing never clears a sibling's binding.
- **The HMAC-signed agent-origin token**, for a subprocess's HTTP emit.

**A spawn does not consume a hop.** Work a fire hands off runs at the fire's own
depth. A hop is a trigger fire, counted once in `handle_domain_event`.

**The ceiling moves to `CapacityPolicy::max_event_trigger_depth`, default 5**,
and a fire the cap stops now raises a notification.

## Rationale

**Why not a task-local everywhere.** It fails silently from a spawned task, and
the coding-agent path has an open-ended number of them. Every one of those emits
already holds the thread id. So it is the only key that can cover them without a
per-site change.

**Why the registry is a cache and not state.** What survives a restart is the
depth on the persisted request, and a re-queued entry registers again when it is
admitted. The binding itself is dropped when the entry completes, so the depth
lasts as long as the WORK rather than the thread. A later user *Continue*
therefore starts a fresh chain at 0.

**Why the deeper of the two carriers wins.** Under-counting re-opens the loop
the cap exists to end, and does it silently. Over-counting suppresses a fire,
which the notification names for the user. Prefer the recoverable error.

**Why the signed token rather than a header.** The depth would otherwise be a
claim any caller could lower to escape the cap. The token's value is documented
as opaque to every client, so widening its signed prefix needs no CLI, Python
shim or SDK change. A header would have needed all three and been forgeable.

**Why the self-wake precedent does not transfer.** A sibling change deliberately
kept its self-wake marker out of spawned work: a suppressor reaching hop 1 kills
a wake the user wanted, so a trigger waiting on a coding agent it started would
never fire. Depth is a counter. It suppresses nothing until the ceiling, so that
same trigger fires at depth 1 and is unaffected.

**Why the ceiling moved, and why it is a field.** At 3 it was nominal: any chain
routed through spawned work escaped counting. Counting them makes it real. The
release chain in the maintainer's own workspace already used three hops end to
end, several times a day, with zero headroom left:

| Depth | Event | Trigger it fires |
|---|---|---|
| 0 | `LucidosReleased`, from a user chat thread | |
| 1 | `SitePublishRequested` | Bump DMG link, publish on release |
| 2 | `SitePublished` | Publish lucidos.dev site |
| 3 | `FrontDoorCheckDispatched` | Verify front door after publish |

That is a sample of one. Other workspaces cannot be enumerated at all. So the
default gets two links of headroom, and the field lets a longer pipeline be
allowed rather than guessed at.

**Why the cap notifies.** Making a previously-uncounted chain counted means a
legitimate long chain can now stop. A verification step that silently stops
running is worse than one that runs too often. The report only goes out when a
trigger really would have fired, so a deep event nobody subscribes to stays
silent.

## Consequences

- A loop through a sub-thread, a coding agent, an agent chat or a script now
  terminates. Before, only a loop that stayed on one await chain did.
- **A chain that previously escaped counting can now hit the ceiling.** It is
  never silent: the user gets a notification naming the trigger, the event and
  the knob.
- The depth is now visible in four more places, so a reader has four things to
  keep consistent rather than one. The single resolution point (`emit_depth`)
  and the single stamp (`submit`) are what keep that bounded.
- A subprocess token minted before this change resolves to depth 0. Tokens do
  not outlive an engine startup, so there is nothing to migrate.
- **A queue frame going out on a task the fire does not own states its depth.**
  That mirrors ADR 0137, which made it state its trigger. `ThreadQueueCompleted`
  comes from the sibling task that joins the work. At depth 0 two triggers subscribed
  to it would wake each other forever, and the self-wake gate cannot help,
  because each wakes the OTHER.
- A policy written before this change loads and takes the new default. A
  ceiling of 0 is refused at both write surfaces: it would stop every event
  trigger from ever firing.
- **Work handed to thread recovery at boot resumes at depth 0.** The handoff
  completes the entry with no in-memory slot, so a binding made there would
  never expire. A later user *Continue* would then inherit a chain it has
  nothing to do with. Resetting is safe because the resume is gated on cause
  (`CLAUDE.md` § Engine Statelessness): a crash leaves the manual Continue
  button, so a loop cannot restart itself into a fresh budget. A re-queued row
  keeps its chain, because the depth is on the request.

## Alternatives considered

**Scope the task-local at every inner `tokio::spawn`.** Rejected: whack-a-mole
over an open-ended set, and a missed site reads zero silently rather than
failing. That is the objection ADR 0133 already recorded.

**A `thread_summaries.chain_depth` column instead of the registry.** Durable
with no rebuild, and the emit path already opens a transaction. Rejected because
it marks a thread permanently: a user manually continuing a trigger-created
thread would keep firing at the old depth forever. The registry expires with the
work, which is the honest lifetime.

**A separate unsigned `x-lucidos-event-trigger-depth` header.** Rejected on both
counts it was meant to win on. It needs a new env var and a matching CLI
constant, and it is forgeable: any caller could declare 0 and escape the cap.

**A new `EventTriggerChainCapped` system event.** Rejected. A persisted frame is
subscribable (ADR 0113). The report can only exist at or past the ceiling, so a
trigger subscribed to it could never fire. Shipping a subscribable event nobody
can usefully subscribe to is worse than shipping none. The report is for the
human, and `NotificationCreated` is already that surface.

**Cap on a repeated trigger instead of on chain length.** A loop is a repeat, so
carrying the visited trigger ids would kill A→A and A→B→A while leaving an
acyclic pipeline of any length alone. Genuinely the better model, and it removes
the length guess. Rejected for now on two grounds. The visited set would have to
ride both the request and the signed token. And it is harsher than today for a
pipeline that legitimately revisits one trigger, which is a failure mode no
visible workspace can validate.

**Keep the ceiling at 3.** Defensible, because the cap now announces itself. It
lost to the measurement: the one real pipeline available sat at exactly three of
three, and the change to the script emit path is what put it there.
