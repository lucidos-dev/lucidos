# 0628: An unattended refusal parks a card for its permission wait when a device is reachable; amends ADR 0002

- **Status**: Accepted; clauses 1 and 2 amended by [ADR 0630](0630-owner-caused-engine-threads-are-attended.md)
- **Date**: 2026-11-08

## Context

ADR 0002 settles that an unattended coding-agent session never hangs. Its
2026-08-24 addendum makes a *refusal* (`RequestVerdict::Unclassified`: a shape
the command guard will not settle) an immediate deny. The card then says
"nobody was there to ask".

That was often untrue. A main-check fix thread ran a path-qualified script with
redirects, was denied, and finished only 46 minutes later, after the user wrote
in the thread. The user had push enabled on an active phone the whole time. The
run was rooted in the engine (`MainCheckRed`), not in a trigger, so a
per-trigger setting alone would not have helped.

## Decision

1. An unattended `Unclassified(Unsettled::Refused)` request **parks a card**
   when three things hold:
   - the session's *permission wait* is not `deny`;
   - the workspace has a *reachable device*: at least one push subscription
     with push enabled (`PushSubscriptionStore::any_push_enabled`);
   - no earlier parked card on the thread ran out its wait.

   Otherwise it is denied at once, as before. An unreadable command and a
   file write into engine state are `Unclassified` too, and never park.
2. The engine sends the push itself when it parks the card. An unattended run
   must not depend on the user's "When agent needs me" trigger.
3. The *permission wait* has three values, one wire form in every layer:
   `"deny"`, whole minutes from 1 to 1440, or `"indefinite"`.
   `core/permission_wait.rs` is the one parser.
4. The wait resolves from the root trigger's `permission_wait`, else the
   workspace preference `unattended_permission_wait` (default 30 minutes). A
   child thread inherits its root trigger's wait through the spawn-tree walk in
   `resolve_attend_mode`, beside the grant. An engine-rooted run with no
   trigger reads the preference.
5. A card nobody answers within its wait is withdrawn and resolved as denied,
   with `UNATTENDED_WAIT_EXPIRED_REASON`. The agent gets the deny and routes
   around it, as it does today.
6. The parked card offers allow once and allow for this thread, never "Always
   allow": a workspace grant still never decides an unattended request
   (ADR 0125).
7. On an unattended thread, an allow for this thread settles only another
   refused shape. It names a command head, and the trigger's side-effect grant
   still decides every side effect under that head.

## Rationale

The "never hangs" rule exists so a trigger with nobody to answer cannot stall.
A reachable device means somebody can answer, and the wait still bounds the
stall. So the rule narrows to "never hangs past its permission wait", and only
`"indefinite"` (an explicit user choice) gives that up.

Only a refused shape parks because it is the one verdict a human can usefully
judge. Nobody can judge a command whose text the engine could not read. An
engine-state write outlives the session, and no trigger run should take one. A catastrophic command is never allowed. An ungrantable or ungranted
side effect has its own remedy, the trigger's side-effect grant, which keeps its
meaning.

A push subscription is the only signal that a push can land. It reads the
workspace's own table, which holds for every platform that registers Web Push.

## Consequences

- A workspace with no push-enabled device keeps the immediate deny, by design.
- A stale subscription row still counts as reachable. The card then waits out
  its window before denying. After that, every later refusal on the thread
  denies at once, so a run stalls for at most one wait.
- A user with a "When agent needs me" trigger may get two pushes for one parked
  card.
- The card says "nobody was there to ask" only on the immediate deny. A
  timed-out card says nobody answered within its permission wait.
- No new event variant: a parked card is the ordinary
  `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved` pair.

## Alternatives considered

- **Keep the immediate deny** (ADR 0002 as written). Rejected: it refused work
  a reachable user would have allowed, and its card told them they were absent.
- **Park with no bound.** Rejected as the default: a stale subscription or a
  phone left in a drawer would stall the run for good. It stays available as
  `"indefinite"`.
- **A per-trigger setting only.** Rejected: engine-rooted runs (the main check,
  a webhook, an agent) have no trigger, and the evidence case was one of them.
- **Park every unattended deny.** Rejected: catastrophic commands and side
  effects already have a fixed answer or a grant, and asking would weaken both.
- **Presence or heartbeat as reachability.** Deferred: a push subscription is
  the only signal we hold that a push can land.

Implementation: `engine/cc_permission.rs` (`unattended_may_park`,
`unattended_park_bound`, `park_card`, `await_parked_card`),
`core/permission_wait.rs`. Plan:
`docs/plans/2026-11-08-unattended-refusal-parks-a-card.md`.
