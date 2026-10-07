---
name: Thread Queue
description: System-wide admission control for ALL thread work: one shared capacity pool gating event triggers, cron fires, run_thread, run_coding_agent and user chat, queueing the rest. Load when a trigger, spawn or chat is waiting, when changing concurrency limits, or when debugging back-pressure.
---

# Thread Queue

System-wide admission control for the shared thread pool. One capacity pool
gates every path that creates running work. Over capacity, work waits in a queue
instead of running unbounded.

User-initiated work (chat responses, user-typed coding-agent threads) is
**prioritized, not exempt** (ADR 0008, superseding ADR 0007). It counts against
`max_concurrent_total`, drains ahead of background, and ignores the per-kind and
per-trigger caps. It queues only when the pool is full, so a person sees
"requesting" only at true pool-max. `reserved_background` is a floor that
background can reclaim ahead of user work, so priority can't starve triggers or
cron.

## What shares the pool

| Path | Kind | How it's gated |
|---|---|---|
| An event trigger matching a domain/thread event | `event-trigger` | background, via `submit` |
| A trigger's cron schedule firing (incl. missed-grace catch-up) | `cron` | background, via `submit` |
| `run_thread` LLM tool (agent-driven sub-thread) | `sub-thread` | background, via `submit` |
| `run_coding_agent` LLM tool (agent-driven coding-agent thread; `coding_agent` preserves Claude Code vs Codex) | `coding-agent` | background, via `submit` |
| Agent/Engine-mode `POST /api/v1/chat/stream` that starts a NEW thread (cross-workspace task POSTs, `lucidos spawn-thread`) | `sub-thread` or `coding-agent`, by `use_coding_agent`; `coding_agent` preserves Claude Code vs Codex for coding-agent rows | background, via `submit` |
| User-initiated chat / user-typed coding-agent threads (a person typing, any workspace; follow-ups on existing threads; child→parent callbacks) | `user-chat` | user, via `acquire_user_slot` |
| Waking a thread parked on an *event wait* (a matching event arrived, or the wait timed out) | `user-chat` | user, `acquire_user_slot` |

Not gated at all: engine recovery resumes, and mid-flight injections into an
already-running thread (they feed an existing response, not a new one).

**A parked thread occupies ZERO slots.** A thread that called `await_event` has
ended its turn: no tokio task, no response in flight. `reconcile_user_slot`
reads its `waiting_for_event` status and releases the slot. This is what makes
the primitive safe. If parked threads held slots, they could fill the pool while
the work that would emit their events queued behind them: a deadlock.

The wake is admitted as `user-chat`, like a child-thread callback. It resumes
work already admitted once, and a woken thread the user watches must not wait
behind a saturated per-trigger cap. It is prioritized but still counted (ADR
0008). So a parked *trigger* thread's wake bypasses that trigger's cap, which is
correct: the fire was admitted when it started.

**Two mechanics, one pool:**

- **Background spawns** go through `ThreadQueue::submit`. The queue owns their
  execution (via the executor) and persists them in the `thread_queue`
  projection, so a restart re-queues work that never ran.
- **User-initiated work** goes through `ThreadQueue::acquire_user_slot`. The chat
  handler runs it itself. The queue only **gates the start** (back-pressure at
  true pool-max) and counts it. These slots are **in-memory only** and are not
  persisted as rows (see § Persistence & restart). The panel API merges them in,
  and a transient `ThreadQueueChanged` refreshes the panel when only user-slot
  state moves.

  The user half of the pool **mirrors `thread_summaries.status`**, the
  authoritative "is this thread running?" that the thread list reads. After the
  gate seeds the slot, one bus subscriber reconciles it on every status-changing
  event (`ThreadQueue::reconcile_user_slot`):
  - A user-initiated thread that is `running` occupies exactly one slot.
  - The slot is removed when the thread parks on the user
    (`waiting_for_user_answer`), idles, completes, errors, or is canceled or
    aborted.
  - The slot is re-added when the thread resumes (the user answers a question or
    resolves a permission prompt), is continued, or auto-resumes after a restart.

  Reconcile reads the committed status (events are observed post-commit). So the
  pool can't drift, and the panel's "Running" set always matches thread status.

## Capacity policy

Configurable caps, persisted event-sourced: the latest `CapacityPolicyChanged`
event IS the policy. Defaults in parentheses:

- `max_concurrent_total` (32): the hard ceiling on threads running at once across
  ALL kinds, background **and** user-initiated.
- `reserved_background` (8): slots background can always *reclaim* ahead of
  user-initiated work. When a slot frees and background is below this floor with
  work waiting, background takes it first. Above the floor, user waiters win.
  `0` means pure user priority (background can starve). Clamped to
  `max_concurrent_total`.
- `max_concurrent_event_trigger` (8) / `max_concurrent_cron` (8) /
  `max_concurrent_sub_thread` (16) / `max_concurrent_coding_agent` (24):
  per-kind caps. They cover background kinds only; user-initiated work has no
  kind bucket.
- `max_concurrent_per_trigger` (1): concurrent runs of one trigger. 1 keeps a
  trigger's fires strictly in arrival order (FIFO per trigger). Governs event
  triggers; cron coalesces (see below).
- `max_queued_per_trigger` (25): hard ceiling on one event trigger's backlog.
  Cron never reaches it, since it coalesces to one.
- `overflow` (`drop-oldest`): what happens at that ceiling.
  `drop-oldest` drops the trigger's oldest waiting fire and notifies.
  `pause-trigger` pauses the trigger and notifies; its queued fires wait for a
  manual resume.
- `max_event_trigger_depth` (5): how many trigger fires one event *chain* may
  make (A fires B, B's event fires C, …). Past it the event is still stored and
  reaches SSE, but fires no further triggers. The user gets a notification
  naming the trigger that did not fire. This is the recursion backstop under the
  authoring rule in `system-knowhow/triggers.md`. Raise it when a chain is
  legitimately longer. Must be ≥ 1 (0 would stop every event trigger firing).

  A spawn does NOT consume a hop. A fire's sub-thread, coding agent or script
  runs at the fire's own depth, and carries it into everything they emit.
- `max_concurrent_children_per_thread` (10): how many *live children* one
  thread may have at the same time. A live child is running, waiting for an
  answer, paused for a resume, holding an event wait, or queued here. A
  `run_thread` past it is refused, and a finished child frees its slot. A child
  follow-up is never refused. Must be ≥ 1.

  Not a pool cap: admission never reads it, only the spawn's recursion guard
  does. See `system-knowhow/orchestrating-sub-threads.md` § Limits you will
  actually hit.

**Cron coalescing (cron kind only).** A cron fire carries no distinct payload
(`Cron { trigger_id }` and nothing else). So a cron trigger holds **at most one
entry** (active + queued ≤ 1). A cron submission while one of its fires is
active *or* queued is **coalesced**: dropped as redundant, with no persisted
queue event. Its scheduler `await` resolves at once, so the cron loop or
missed-grace catch-up moves on to the next occurrence rather than hanging.

Coalescing is intrinsic to the cron kind, not a knob, so
`max_queued_per_trigger` and `overflow` never apply to cron. Event triggers
carry a per-fire `event_payload`, keep strict FIFO under the caps above, and
never coalesce. Coalescing stops a restart storm (each boot re-queuing the
in-flight fire and re-firing the missed occurrence) from stacking identical cron
fires.

An **off-schedule run** (`triggers(action="run")`, the trigger row's *Run once*
button) is a third submitter of the cron kind, so it coalesces by the same rule.
The scheduler wants the redundant fire dropped, but a person who asked for a run
needs the truth. So `SubmitOutcome` carries a `coalesced` flag, and the run
action answers that the trigger is already running or queued and nothing new
started. The two scheduler submit sites ignore the flag.

Concurrency caps of **0 mean "hold"**: admission pauses and the queue
accumulates. For example, `max_concurrent_total: 0` freezes all work, including
new user responses. `max_queued_per_trigger`, `max_event_trigger_depth` and
`max_concurrent_children_per_thread` must each be ≥ 1.

## Ordering

A freed slot is filled in three passes: **(1)** background reclaims up to
`reserved_background` first, **(2)** user-initiated waiters take priority (FIFO),
**(3)** background fills whatever capacity remains. Within background, FIFO is:

- **strict per trigger**: a trigger's fires run in arrival order, so a new fire
  queues behind the trigger's backlog even when capacity is free.
- **best-effort across triggers**: an entry blocked by its own trigger's cap
  doesn't hold up other triggers' entries.

A new background `submit` also yields a free slot to a waiting user, unless
background is still below its reserved floor. Cron fires never queue behind each
other: they coalesce to one entry per trigger (see *Cron coalescing* above).

## Persistence & restart

**Background** entries live in the `thread_queue` projection (event-sourced from
`ThreadQueued` / `ThreadQueueAdmitted` / `ThreadQueueDropped` /
`ThreadQueueCompleted`). Rows also persist:

- **The coding-agent backend** (`claude-code` default, `codex` when requested),
  so queue drain and restart requeue never silently fall back to Claude Code.
- **Attribution (`origin`)** for both spawn kinds (`sub-thread`,
  `coding-agent`). A spawn that waited or was re-fired still names who started
  it in the message route popover: its *spawning thread*, or the engine and its
  reason for an engine-seeded thread such as a plugin *setup thread*. It is
  separate from `parent_thread_id` because a *top-thread* has an origin and no
  callback linkage. Entries queued before the field existed carry none.
- **The event-trigger chain depth**, for every spawn kind. A fire's spawned work
  runs at the fire's depth, whether admitted at once, drained after a wait, or
  re-fired after a restart. Rows written before the field carry 0, which is what
  they meant.

On engine restart:

- `queued` entries are reloaded as-is.
- `admitted` entries are work the old process had already started, and **one
  rule covers all four kinds** (ADR 0133). An entry whose bound thread still
  exists is **handed off** to thread-level recovery (CC auto-resume / chat
  settle), never re-run. An entry that bound no thread **re-queues and
  re-fires**. A trigger fire binds its thread as soon as it creates one, so a
  fire that parked on a question never runs twice. A fire that died before
  creating one ran nothing, so it is still owed. A **script** fire creates no
  thread, so it always re-runs.
- **Cron recovery is idempotent.** Duplicate cron rows for one trigger (left by
  a restart storm) collapse to one entry on reload. The oldest is re-queued; the
  rest emit `ThreadQueueDropped` (reason "coalesced on recovery") to clear their
  projection rows. So repeated reboots never re-stack a cron backlog. A row that
  hands off does not take that slot: it already ran, so a sibling row that never
  ran still gets its turn.

**User-initiated** slots are in-memory only: NOT persisted and NOT re-fired. A
dead response is gone on restart (the person re-sends if they want), and the
pool count resets to its live background occupancy.

The queue also keeps an in-memory binding from a thread to its chain depth. It
keeps a coding-agent session's events on the chain, since they come from tasks
no task-local reaches. It is a cache, not state:

- Each binding is owned by its queue entry, so one entry completing never takes
  a sibling's.
- A restart keeps the depth on the row's request, and a re-queued entry binds
  again when admitted.
- The binding drops when the entry completes, so a later user *Continue* on the
  same thread starts a fresh chain at 0.
- Work **handed off** at boot gets no binding and resumes at 0. Nothing would
  ever clear one, and the resume is gated on cause, so a crashed loop cannot
  restart itself into a fresh budget.

Draining starts after the scheduler has replayed trigger configs, so paused or
deleted triggers are honored from the first admission decision.

## The panel

**Thread Queue** is a row under **Settings → System**, beside Backup, Memory,
Disk Usage and Environment Variables. It shows the Running set (with the total
cap, counting background **and** user-initiated work), the Queued backlog, and
the capacity policy editor. Each **background** queued entry has **Run now**
(force-admit, ignoring every cap) and **Drop** (discard without running).
Running entries can't be dropped: cancel the thread itself. User-initiated
entries (`kind: "user-chat"`) have no Run now or Drop, since they are already
prioritized. Cancel those from the chat.

## Notifications

- A trigger's oldest waiting fire has waited 5 minutes or more →
  "`<trigger>` is significantly delayed". The message states that wait and
  how many fires are waiting (10-minute cooldown per trigger). Queue depth
  alone never alerts, since a burst of quick fires drains in seconds. A paused
  trigger never alerts, since its fires wait for resume. The check runs with
  the minute-long drain loop, so an alert can lag by up to a minute.
- The pool hits `max_concurrent_total` (background + user) → one "Lucidos is at
  capacity" notification (10-minute cooldown), not one per queued entry.
- A per-trigger queue overflow → notification naming what was dropped (or
  that the trigger was paused).
- An event chain reaches `max_event_trigger_depth` and a real trigger would
  have fired → "`<trigger>` did not fire". It names the event, the depth and
  the ceiling (10-minute cooldown per trigger). It taps to the notification
  itself, not the panel: the fix is usually to restructure the chain or raise
  the ceiling. A deep event that matches no trigger stays silent.

Every other Thread Queue notification taps through to the **Thread Queue** panel
(`Tap::Navigate { target: thread-queue }`), so the user lands on that backlog.

## HTTP API

- `GET /api/v1/thread-queue` → `{ entries, policy }`. `entries` are background
  rows (FIFO) plus the in-memory user-initiated occupants (`kind: "user-chat"`);
  `status` is `queued` or `admitted`. This endpoint and the `list_thread_queue`
  LLM tool read the SAME merged view (`ThreadQueue::snapshot`), so the panel and
  the tool always agree on who occupies the pool.
- `POST /api/v1/thread-queue/run-now` `{ entry_id }`: force-admit a background
  entry (user-initiated entries have no queue row).
- `POST /api/v1/thread-queue/drop` `{ entry_id }`: drop a queued background
  entry.
- `PUT /api/v1/thread-queue/policy`: replace the capacity policy. Defaults fill
  a partial body; it returns the stored policy.

Mutations are actor-stamped: the panel actions carry the acting device on the
emitted events; engine drain decisions carry no actor.
