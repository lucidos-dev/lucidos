# 0630: Owner-caused engine threads are attended, and only a trigger announces a parked card; amends ADR 0628

- **Status**: Accepted
- **Date**: 2026-11-08

## Context

Two defects showed up in one day, both after ADR 0628.

1. A landing thread, started by the owner's Apply, ran as unattended. Its
   engine origin was `MergeConflict`, and `resolve_attend_mode` sent every
   engine reason it did not name to unattended and ungranted. The workspace
   grants never applied, and a path-headed script parked a card.
2. Every parked card pushed twice. The engine sent its own push (ADR 0628
   clause 2). The user's "When agent needs me" trigger sent a second one.
   ADR 0628 accepted that double as a consequence. The owner has now ruled
   that the notification comes from the trigger, never from the engine.

## Decision

1. `resolve_attend_mode` classifies every `EngineReason` in an exhaustive
   match, with no wildcard arm. A new reason fails to compile until someone
   classifies it.
2. A reason whose thread exists because the owner acted is **Interactive**.
   `Scheduler` keeps its trigger grant and wait. `MainCheckRed`,
   `MainCheckFlaky` and `PluginAutoUpdate` stay unattended and ungranted,
   because the engine raises them on its own. `MergeConflict` starts a lane
   thread. It is Interactive only when a human queued every member of its
   batch, read from each member's latest `ChangeQueued` actor. An agent can
   apply its own change, so the reason alone proves no owner's press.
3. The engine sends no push when it parks an unattended card. This amends
   ADR 0628 clause 2.
4. An unattended refused shape parks only when a trigger will announce the
   card: an enabled, unpaused trigger that fires on the card's
   `CodingAgentPermissionRequest`. The gate asks what dispatch asks: the
   depth cap at the depth the bus would stamp, `is_subscribable`, then the
   matcher on the real payload, minus the trigger whose fire is emitting.
   Otherwise the request is denied at once. This amends ADR 0628
   clause 1: a reachable push device no longer gates parking, and
   `PushSubscriptionStore::any_push_enabled` is deleted.

Every other clause of ADR 0628 stands, including the permission wait, the
card's options and the expiry.

## Rationale

The attend mode answers "is a human present for this thread?". A landing
thread whose batch the owner queued exists because the owner pressed Apply,
so the answer is yes. An agent's own apply is no such press. Matching
on the reason makes that answer a decision someone took for each reason.

A parked card helps only if somebody hears about it. A device that takes push
says nothing about that once the engine stops pushing. A trigger is the user's
own statement of what they want to hear about, so it is the signal to read.
Reusing the dispatch predicate keeps the gate true to what dispatch does.

## Consequences

- A workspace with no notifying trigger gets the immediate deny for a refused
  shape on an unattended thread, as before ADR 0628.
- A user with the trigger gets one push per parked card.
- A lane thread whose batch an agent queued stays unattended and ungranted.
  A standing apply queues under the actor who armed it (ADR 0168), so an
  owner's armed press still counts as the owner's.
- A trigger matching the card but notifying nobody, such as one that only
  logs it, still counts as announcing. A trigger is the user's own choice.
- ADR 0125 holds: a workspace grant still never decides a request in a session
  that really is unattended.

## Alternatives considered

- **Keep the device-takes-push gate and only remove the push.** Rejected: a
  user with no such trigger would get a card that waits silently for its whole
  permission wait, then a deny.
- **Keep the engine push and silence the trigger.** Rejected: the owner ruled
  that the trigger is the one source, and the trigger carries the user's
  routing choices.
- **Move only `MergeConflict` to Interactive.** Rejected: a wildcard arm would
  keep misclassifying the next reason. The exhaustive match removes that class.
- **Make a human's answer to a parked card flip the thread to Interactive.**
  Not decided here. `human_acted_in` reads messages and question answers only.

Implementation: `engine/cc_permission.rs` (`attend_mode_of_engine_reason`,
`humans_queued_lane_batch`, `unattended_park_bound`, `a_trigger_announces`, `park_card`). Plan:
`docs/plans/2026-11-08-owner-caused-engine-sessions-are-attended.md`.
