# 0010 — The user-half of the Thread Queue pool mirrors `thread_summaries.status` (one reconcile point, not hand-synced acquire/release)

- **Status:** Accepted (refines the user-slot mechanics of ADR 0008; does not change its policy)
- **Date:** 2026-06-15

## Context

ADR 0008 put user-initiated work in the one shared capacity pool: a back-pressure
gate (`acquire_user_slot`) reserves a slot when a person sends, the slot counts
toward `max_concurrent_total`, and the Thread Queue panel shows it as "Running".
User slots are in-memory only (a dead response is gone on restart, never
re-fired).

The slot's *lifetime* was hand-synced across several places: the chat handler
acquired it; a `UserSlotGuard` released it when the spawned task returned; and a
bus subscriber proactively released it when the thread "settled" (idled, parked
on the user, completed, errored, canceled). That last release was added because a
coding-agent thread parked on an `AskUserQuestion` blocks its task indefinitely,
so the guard alone would hold the slot forever.

The hand-sync drifted. The subscriber *released* the slot when the thread parked
on a question (correct — a thread waiting on the user isn't consuming a slot) but
nothing *re-acquired* it when the user answered and the thread resumed running.
So an answered thread ran to completion holding no slot — invisible in the panel
("Nothing running" while it was visibly working) and uncounted by the policy. The
same gap hit continuation respawns and post-restart auto-resume: every path that
runs a user thread *other than* the initial chat POST went through
`run_direct_agent` without ever touching `acquire_user_slot`.

The root cause: "is this user thread running?" was stored **twice** — once
authoritatively in `thread_summaries.status` (every code path updates it; it's
what the thread list reads) and again in the pool's in-memory `user_active` set,
kept in sync by enumerating which events acquire vs release. Two representations
of the same fact drift.

## Decision

The pool stays; the policy stays; back-pressure stays (ADR 0008 is unchanged).
What changes is how the **user-half** of the pool is maintained: it becomes a
**faithful mirror of `thread_summaries.status`**, reconciled in **one place**.

- `acquire_user_slot` keeps its single job: the **back-pressure gate** for NEW
  work — reserve a slot, or wait at true pool-max. It seeds the slot.
- From there, one bus subscriber calls `reconcile_user_slot(thread_id)` on every
  status-changing event. Reconcile reads the real, just-committed status (events
  are observed post-`tx.commit()`, post-projection) and converges the pool:
  a user-initiated (`initiator = 'user'`) thread that is `running` occupies
  **exactly one** slot; anything else occupies **none**. It is idempotent and
  *direction-agnostic* — it doesn't matter whether the triggering event was a
  park, a resume, or a termination; reconcile reads where the thread actually
  landed and makes the pool match.
- Adds for a thread that is *already running* (resume after a park, continuation
  respawn, post-restart auto-resume, engine-injected hardening prompt) are
  unconditional — that thread can't be made to wait. Back-pressure applies only
  to NEW work via the gate. reconcile deliberately does **not** fire on the
  gate-covered starts (`MessageReceived`, `SessionStarted`,
  `CodingAgentUserMessageSent`), so a reconcile add can never race the gate's
  unconditional add and double-count.

## Rationale

- **One source of truth.** The thread's status is already mandatory, correct on
  every path, and heavily tested (it's the projection's job). Deriving pool
  membership from it deletes the hand-sync that drifted, instead of adding more
  acquire/release wiring to keep two representations aligned.
- **Fixes the whole family, not one symptom.** Resume-after-answer (the reported
  bug), continuation respawn, and post-restart auto-resume all flip `status`
  back to `running`; reconciling against status covers all three with no
  per-path code. A targeted "re-acquire on `UserQuestionAnswered`" fix would
  have patched only the first.
- **Can't drift by construction.** There is one place the user-half moves in and
  out, and it reads reality rather than predicting it. A future status-writing
  event that someone forgets to list degrades to "the pool lags status until the
  next status event for that thread (or the gate guard's drop)" — never a phantom
  or a leak.

## Consequences

- The panel's "Running" user set tracks `thread_summaries.status` across the
  whole park → resume → terminate cycle, not just the first turn.
- A user thread that resumes while the pool is at max briefly pushes total
  occupancy over `max_concurrent_total`. This is unavoidable and correct: the
  thread is *already running* and ADR 0008's "no eviction" rule forbids pausing
  it. The next completion-driven drain self-corrects. Back-pressure on **new**
  work is unchanged.
- reconcile does one indexed `thread_summaries` lookup per status event for a
  thread. It is gated to status-transition events (not per-token streaming), so
  the cost is a handful of cheap PK lookups per turn.
- The `UserSlotGuard`'s drop is now a **backstop**, not the primary release —
  normally the terminal status event already reconciled the slot away, so the
  drop is a no-op. It still matters if a task dies without a terminal status
  event, or to clear a still-queued waiter.

## Alternatives considered

- **Re-acquire only on `UserQuestionAnswered` / `*PermissionResolved`** (a
  `parked` flag on the slot, toggled by park/resume in the subscriber) —
  rejected: it fixes the reported live-subprocess case but not continuation
  respawn or post-restart auto-resume, which run `run_direct_agent` with no slot
  at all. It also keeps two representations of "running" and a second enumerated
  event list to maintain.
- **Panel reads `thread_summaries.status`; pool/policy stay in-memory as-is** —
  rejected: the display would be correct but the policy's user count would still
  lag reality on a parked-then-resumed thread, so the panel and the policy could
  disagree about pool occupancy. Mirroring *both* off status keeps them
  consistent.
- **Delete the user pool; derive both display and back-pressure from status** —
  rejected: it would remove the in-memory gate, and ADR 0008 deliberately keeps
  user work *queued at true pool-max* ("count user work but never block it" was
  explicitly rejected there). Back-pressure needs the synchronous gate, which
  status (a post-hoc projection) can't provide.
- **Persist user slots as `thread_queue` rows** — rejected for the same reasons
  as ADR 0008: writes on the hot path and restart-drop special-casing for work
  that must never be re-fired. In-memory + reconcile-against-status is the right
  home for ephemeral runtime.
