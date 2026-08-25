---
name: Thread Queue
description: System-wide admission control for ALL thread work: one shared capacity pool gating event triggers, cron fires, run_thread, run_coding_agent and user chat, queueing the rest. Load when a trigger, spawn or chat is waiting, when changing concurrency limits, or when debugging back-pressure.
---

# Thread Queue

System-wide **admission control for the shared thread pool**. One capacity pool
gates every path that creates running work; over capacity, work waits in a queue
instead of running unbounded.

User-initiated work (chat responses, user-typed coding-agent threads) is
**prioritized, not exempt** (ADR 0008, superseding ADR 0007's "user preempts and
doesn't count"): it counts against `max_concurrent_total`, drains ahead of
background, ignores the per-kind / per-trigger caps, and queues when the pool is
genuinely full — a person briefly sees "requesting" only at true pool-max.
`reserved_background` is a floor background can reclaim ahead of user work so
priority can't starve triggers/cron.

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

NOT gated at all: mid-flight injections into an already-running thread (they feed
an existing response, they don't start a new one) and engine recovery resumes.

**A parked thread occupies ZERO slots.** A thread that called `await_event` has
ended its turn: no tokio task, no response in flight, and `reconcile_user_slot`
reads its `waiting_for_event` status and releases the slot. This is not an
optimization, it is what makes the primitive safe. If a parked thread held its
slot, N threads waiting on events would fill the pool while the very work that
would emit those events queued behind them, which is a deadlock rather than
mere waste.

The wake is admitted as `user-chat`, the same kind as a child-thread callback
and for the same reason: it resumes work that was already admitted once, rather
than starting new work, and a woken thread the user is watching must not sit
behind a saturated per-trigger cap. Prioritized but still counted (ADR 0008). A
parked *trigger* thread's wake therefore bypasses that trigger's concurrency
cap, which is correct: the fire was admitted when it started.

**Two mechanics, one pool:**

- **Background spawns** go through `ThreadQueue::submit`: the queue owns their
  execution (via the executor) and persists them in the `thread_queue`
  projection, so a restart re-queues work that never ran and drains it as
  capacity frees.
- **User-initiated work** goes through `ThreadQueue::acquire_user_slot`: the chat
  handler runs it itself; the queue only **gates the start** (back-pressure —
  awaiting a slot at true pool-max) and counts it. These slots are **in-memory
  only** — a dead response is gone on restart and is never re-fired (the person
  re-sends if they want), so they are not persisted as rows. The panel API
  merges them in, and a transient `ThreadQueueChanged` refreshes the panel when
  only user-slot state moves.

  The user-half of the pool is a **faithful mirror of `thread_summaries.status`**
  — the authoritative "is this thread running?" that the thread list reads. After
  the gate seeds the slot, a single bus subscriber **reconciles** the slot on
  every status-changing event (`ThreadQueue::reconcile_user_slot`): a
  user-initiated thread that is `running` occupies exactly one slot; once it
  parks on the user (`waiting_for_user_answer`), idles, completes, errors, or is
  canceled/aborted, the slot is removed; when it resumes (the user answers a
  question / resolves a permission prompt), is continued, or auto-resumes after a
  restart, the slot is re-added. Because reconcile reads the *real, committed*
  status (events are observed post-commit), the pool can't drift from reality —
  there is **one place** the user-half moves in and out, so the panel's "Running"
  set always matches actual thread status. (The earlier design released the slot
  on park but never restored it on resume, so an answered thread kept running yet
  showed as "Nothing running"; reconciling against status fixes that — and the
  same resume, continuation, and post-restart paths — in one stroke.)

## Capacity policy

Configurable caps, persisted event-sourced (the latest `CapacityPolicyChanged`
event IS the policy). Defaults in parentheses:

- `max_concurrent_total` (32) — the hard ceiling: threads running at once across
  ALL kinds, background **and** user-initiated.
- `reserved_background` (8) — slots background can always *reclaim* ahead of
  user-initiated work. When a slot frees and background is below this floor with
  work waiting, background takes it before any user waiter; above the floor, user
  waiters win. `0` = pure user priority (background can be starved). Clamped to
  `max_concurrent_total`.
- `max_concurrent_event_trigger` (8) / `max_concurrent_cron` (8) /
  `max_concurrent_sub_thread` (16) / `max_concurrent_coding_agent` (24) —
  per-kind caps (background kinds only; user-initiated isn't bucketed by kind).
- `max_concurrent_per_trigger` (1) — concurrent runs of one trigger. 1 keeps a
  trigger's fires strictly in arrival order (FIFO per trigger). Governs event
  triggers; cron coalesces (see below).
- `max_queued_per_trigger` (25) — hard ceiling on one trigger's backlog (event
  triggers; cron never reaches it — it coalesces to one).
- `overflow` (`drop-oldest`) — what happens at the ceiling:
  `drop-oldest` drops the trigger's oldest waiting fire + notifies;
  `pause-trigger` pauses the trigger + notifies (its queued fires wait for a
  manual resume).

**Cron coalescing (cron kind only).** A cron fire carries no distinct payload
(`Cron { trigger_id }` and nothing else), so a cron trigger never needs more than
one pending fire — it holds **at most one entry** (active + queued ≤ 1). A cron
submission while one of its fires is already active *or* queued is **coalesced**:
dropped as redundant (no persisted queue event), with its scheduler `await`
resolved immediately so the cron loop / missed-grace catch-up proceeds to the
next occurrence rather than hanging. This is intrinsic to the cron kind — not a
configurable knob — so `max_queued_per_trigger` / `overflow` never apply to cron.
Event triggers carry a per-fire `event_payload` and keep strict FIFO (the caps
above govern them); they are never coalesced. This is what stops a restart storm
(each boot re-queuing the in-flight fire + re-firing the missed occurrence) from
stacking dozens of identical cron fires.

An **off-schedule run** (`triggers(action="run")`, the trigger row's *Run once*
button) is a third submitter of the same cron kind, so it coalesces by the same
rule. That is fine for the scheduler, which wants the redundant fire dropped,
but wrong to report to a person who just asked for a run, so `SubmitOutcome`
carries a `coalesced` flag and the run action answers "already running, nothing
started" instead of claiming it began one. The two scheduler submit sites ignore
the flag.

Concurrency caps of **0 mean "hold"** — admission pauses and the queue
accumulates (e.g. `max_concurrent_total: 0` freezes all work — including new
user responses). `max_queued_per_trigger` must be ≥ 1.

## Ordering

A freed slot is filled in three passes: **(1)** background reclaims up to
`reserved_background` first, **(2)** user-initiated waiters take priority (FIFO),
**(3)** background fills whatever capacity remains. Within background, FIFO is
**strict per trigger** (a trigger's fires run in arrival order — a new fire
queues behind the trigger's existing backlog even when capacity is free) and
**best-effort across triggers** (an entry blocked by its own trigger's cap
doesn't hold up other triggers' entries). A new background `submit` also yields a
free slot to a waiting user — unless background is still below its reserved floor.
Cron fires don't queue behind each other at all — they coalesce to a single
entry per trigger (see *Cron coalescing* above).

## Persistence & restart

**Background** entries live in the `thread_queue` projection (event-sourced from
`ThreadQueued` / `ThreadQueueAdmitted` / `ThreadQueueDropped` /
`ThreadQueueCompleted`). Coding-agent requests persist the selected backend
(`claude-code` default, `codex` when requested), so queue drain and restart
requeue do not silently fall back to Claude Code. Both spawn kinds
(`sub-thread`, `coding-agent`) also persist their attribution (`origin`), so a
spawn that waited behind capacity or was re-fired after a restart still names
its *spawning thread* in the message route popover. It is separate from
`parent_thread_id` because a *top-thread* has an origin and no callback linkage;
entries queued before the field existed simply carry none. On engine restart:

- `queued` entries are reloaded as-is.
- `admitted` entries are work that died with the old process: trigger fires
  **re-queue and re-fire** (same re-execution semantics as missed-cron
  catch-up); sub-thread / coding-agent spawns whose thread already
  materialized are handed off to thread-level recovery (CC auto-resume / chat
  settle) instead of re-spawning a duplicate.
- **Cron recovery is idempotent.** Duplicate cron rows for one trigger (left by
  a restart storm) collapse to a single entry on reload — the oldest is kept and
  re-queued, the rest emit `ThreadQueueDropped` (reason "coalesced on recovery")
  to clear their projection rows. So repeated reboots can never re-stack a cron
  backlog.

**User-initiated** slots are in-memory only — they are NOT persisted and NOT
re-fired. A dead response is simply gone on restart (the person re-sends if they
want); the pool count resets to its live background occupancy.

Draining starts after the scheduler has replayed trigger configs, so paused /
deleted triggers are honored from the first admission decision.

## The panel

**Thread Queue** under **Settings → System** (a subpanel tab alongside Backup,
Memory, Disk Usage, Environment Variables) shows the Running set (with the total
cap — counting background **and** user-initiated work), the Queued backlog, and
the capacity policy editor. Per **background** queued entry: **Run now** (force-admit,
ignoring every cap) and **Drop** (discard without running). Running entries can't
be dropped — cancel the thread itself instead. User-initiated entries
(`kind: "user-chat"`) carry no Run now / Drop — a queued user thread is already
prioritized; cancel it from the chat, not here.

## Notifications

- A trigger's backlog reaches 10 waiting fires, or its oldest waiting fire is
  older than 5 minutes → "`<trigger>` is significantly delayed" (10-minute
  cooldown per trigger).
- The pool hits `max_concurrent_total` (background + user) → one "Lucidos is at
  capacity" notification (10-minute cooldown), not one per queued entry.
- A per-trigger queue overflow → notification naming what was dropped (or
  that the trigger was paused).

Every Thread Queue notification taps through to the **Thread Queue** panel
(`Tap::Navigate { target: thread-queue }`) so the user lands on the backlog
they're being told about.

## HTTP API

- `GET /api/v1/thread-queue` → `{ entries, policy }`. `entries` are background
  rows (FIFO) plus the in-memory user-initiated occupants (`kind: "user-chat"`);
  `status` is `queued` or `admitted`. This endpoint and the `list_thread_queue`
  LLM tool both read the SAME merged view (`ThreadQueue::snapshot`), so the panel
  and the tool can never disagree about who occupies the pool.
- `POST /api/v1/thread-queue/run-now` `{ entry_id }` — force-admit a background
  entry (user-initiated entries aren't backed by a queue row).
- `POST /api/v1/thread-queue/drop` `{ entry_id }` — drop a queued background
  entry.
- `PUT /api/v1/thread-queue/policy` — replace the capacity policy (partial
  bodies are filled with defaults; returns the stored policy).

Mutations are actor-stamped: the panel actions carry the acting device on the
emitted events; engine drain decisions carry no actor.
