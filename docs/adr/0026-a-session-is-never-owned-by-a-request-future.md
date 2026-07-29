# 0026 — A coding-agent session is never owned by a request future, and a map entry always implies a live loop

- **Status** — Accepted
- **Date** — 2026-07-28

## Context

A user tapped **Apply** on a coding-agent thread from the iOS PWA. The branch had
fallen behind `main`, so `apply_change` Tier 2 resumed a coding-agent session in the
thread's worktree to resolve the conflict. The session merged `main`, resolved the
conflicts, and then died mid-tool 72 seconds in. Seven minutes later the user's next
message came back:

> A coding agent is already running for this thread. Cancel it first or wait for it
> to finish.

…with no such process anywhere on the machine. The thread stayed wedged until the
engine restarted.

Two independent defects, the first causing the second.

**1. The session was owned by the HTTP request.** `POST /api/v1/claude-code/apply-now`
awaited `apply_now`, whose no-live-session branch awaited `apply_change`, whose Tier 2
awaited `run_merge_session_tier2` — an entire coding-agent session, driven inline
inside the axum handler. When iOS Safari dropped the connection (a backgrounded PWA,
or WebKit's request timeout), hyper dropped the handler future and the session went
with it. The subprocess recorded `"interruptedByShutdown": true` in its own transcript;
the engine log has no completion line for that POST, while an unrelated apply eight
minutes later logged `→ 200 (622ms)` normally.

Tier 2 was the last inline merge path. Tier 1's divergent-`main` branch already
spawned (`spawn_in_place_conflict_recovery`), Tier 3 already spawned
(`spawn_merge_session`), and `apply_now`'s live-session path already spawned — each
with a liveness timeout and a panic guard. Tier 2 simply predated the pattern.

**2. A dropped run future left a phantom session.** `run_session` inserts an entry into
`agent_sessions` and removes it on every *completion* path. Cancellation runs none of
them, so the entry survived with `process_exited == false` — a flag only the loop ever
sets — while its `msg_rx` went out of scope with the future. Three independent readers
each asked `!process_exited` and were each fooled:

- `worktree_cleanup`'s `is_active` was a bare `contains_key`, despite a comment
  claiming it probed liveness — it logged "skipping thread … — live agent session
  active" on every cycle, forever.
- The chat follow-up fast path found a "live" session and sent into a dead channel.
- The resume guard found the same entry and refused every follow-up.

The thread had no way out: the fast path couldn't route, the slow path wouldn't spawn,
and cleanup wouldn't reclaim.

## Decision

**A coding-agent session is never owned by a request future.** A handler may *start* a
session; it may not await one. The outcome reaches the user through events
(`ChangeApplied` / `ChangeApplyFailed`), which is where it already lived — the awaited
return value only ever shaped an `ApplyResult` for the caller. Tier 2 now hands the
merge to `spawn_cc_task_guarded` and answers immediately with `ApplyResult::conflict`,
exactly as Tier 1 does. Its Tier-3 fallback moved into the spawned task, which required
extracting Tier 3 into `apply_change_tier3` so both callers reach it.

**An `agent_sessions` entry always implies a live run loop**, enforced two ways because
one alone is insufficient:

- **`AgentSession::is_live()`** — `!process_exited && !msg_tx.is_closed()` — is the
  single liveness predicate, and every reader uses it. The receiver is owned by the run
  future, so a dropped future closes the channel immediately: this is correct *before
  any cleanup has run*, which matters because cleanup is asynchronous and there is
  always a window.
- **`SessionEntryGuard`** reaps the entry when the run future drops, so the map cannot
  grow phantoms and cleanup is never blocked. It identifies its own entry by
  `msg_tx.same_channel` — a recovery hand-off deliberately leaves the outgoing session
  in the map until the incoming one replaces it, and a blind `remove(&thread_id)` would
  delete the replacement. It then settles the thread with
  `ResponseAborted{cause: SessionDropped}`, unless the engine is shutting down (that
  sweep owns its own terminal, and a second emit would double-report).

`AbortCause::SessionDropped` is a new variant rather than a reuse of `SafetyNet`
(the loop ran to EOF without a `Result`) or `ProcessKilled` (the subprocess died under
a live loop). Neither describes "the loop never got to run at all", and the distinction
is exactly what made this incident hard to read.

## Consequences

- Applying a conflicted change from a phone survives backgrounding the app. The HTTP
  call returns `conflict` in milliseconds; the merge finishes on its own and the change
  applies.
- Callers see `Conflict` where Tier 2 previously blocked and returned `Applied`. This is
  not new behavior at the API level — Tier 1 and Tier 3 have long answered this way, the
  frontend resolves its Apply spinner on the events, and the Apply-All driver is
  event-fed.
- Membership in `agent_sessions` is no longer meaningful on its own. New code asks
  `is_live()`; a bare `!process_exited` liveness check is a bug.
- A cancelled session now produces a visible terminal instead of a thread frozen
  mid-turn.
- Guard tests pin both halves: `tier2_merge_session_is_detached_from_the_caller_future`
  and `api_handlers_never_drive_a_coding_agent_session_directly`.

## Alternatives considered

- **Keep the request open (client keepalive, longer timeouts).** Asks a mobile browser
  to hold a multi-minute POST open. The contract is wrong regardless of how well it is
  tuned; a backgrounded tab still loses.
- **Wrap the handler in `tokio::spawn`.** Fixes the one endpoint and hides *which*
  operations are long-running. The tiers already had the right pattern; Tier 2 needed to
  join it.
- **Only reap phantoms, leave the merge inline.** The thread would unwedge, but every
  conflicted mobile Apply would still abort halfway through a resolution.
- **Only detach, don't reap.** Fixes this cause of phantoms and leaves the class open —
  any future cancellation reproduces the wedge.
- **Reuse `AbortCause::SafetyNet` for a dropped session.** Avoids regenerating contract
  artifacts, at the cost of collapsing two genuinely different failures into one
  explanation in the route popover.
