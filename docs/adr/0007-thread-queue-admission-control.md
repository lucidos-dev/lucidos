# 0007 — One Thread Queue gates all background spawns; user chat preempts; restart re-fires, never replays

- **Status:** Accepted; the "user chat preempts and **doesn't count against
  capacity**" point is **superseded by [ADR 0008](0008-user-initiated-work-counts-against-the-pool.md)** (user-initiated work now shares the one
  pool — prioritized, but counted and queued at the ceiling, with a reserved
  background floor). Everything else here still holds.
- **Date:** 2026-06-12

## Context

Before this, three independent pools spawned background work with no shared
capacity: chat threads (unlimited, serialized only per-thread via
`register_thread_queued`), CC sessions (a `Semaphore(2)` on subprocess
*startup* only — unlimited total), and trigger/cron tasks (unlimited, with an
informational `ACTIVE_TASK_COUNT`). A burst of events could fan out into
unbounded concurrent trigger fires, each holding tool slots, model budget, and
worktree state (work-tracker `backpressure-event-triggers-thread-queue`).

## Decision

One *Thread Queue* (`engine/thread_queue/`) gates every background spawn path
— event-trigger fires, cron fires, `run_thread` / `run_claude` tool spawns,
and agent-mode `chat/submit` POSTs that start a new thread (cross-workspace
tasks, `lucidos spawn-thread`). Capacity is a configurable *capacity policy*
(global + per-kind + per-trigger caps); over capacity, spawns queue FIFO
(strict per trigger, best-effort across triggers). Queue + active set are
persisted in the event-sourced `thread_queue` projection. User-initiated chat
never routes through the queue.

## Rationale

- **One pool, not three** — the resources being protected (model budget, CPU,
  worktrees) are shared, so the cap must be too. Per-pool caps would just move
  the burst to whichever pool is uncapped.
- **Queue, don't drop** — a trigger fire represents user intent ("notify me
  when X"). Dropping silently turns load spikes into missed work; queueing
  turns them into latency, which the panel and notifications make visible.
- **User chat preempts** — a person typing must never wait behind background
  work. "Preempts" = bypasses admission and doesn't count against capacity;
  it does NOT evict running background work (eviction would tear down
  half-finished agent turns to save seconds, a worse trade). **[Superseded by
  ADR 0008:** user-initiated work now shares the one pool — still prioritized
  and never evicting, but it *counts* and *queues at the ceiling*, with a
  reserved background floor so priority can't starve triggers/cron. The
  exemption re-created the uncapped lane this ADR set out to kill.**]**
- **Strict FIFO per trigger via `max_concurrent_per_trigger: 1` default** —
  "preserve fire order per trigger" is only meaningful if the same trigger's
  fires don't run concurrently. Cross-trigger order stays best-effort so one
  saturated trigger can't convoy everyone else.
- **Restart re-fires admitted-but-dead entries** (rather than resuming them
  mid-flight or dropping them): re-execution is the engine's existing crash
  philosophy (missed-cron catch-up "record after execution so crash mid-task
  re-executes"). Exception: sub-thread / coding-agent entries whose thread
  already materialized hand off to thread-level recovery (CC auto-resume /
  chat settle) — re-firing those would duplicate the thread.

## Consequences

- Every background spawn now emits `ThreadQueued` + `ThreadQueueAdmitted` (+
  `ThreadQueueCompleted`), making the events table a complete audit of
  background work — at the cost of three extra event rows per spawn.
- The cron task loop awaits its fire's completion through the queue, so a
  saturated system back-pressures a trigger's schedule instead of stacking
  concurrent runs of it.
- `scheduler::ACTIVE_TASK_COUNT` is now informational-only bookkeeping for
  the shutdown drain; enforcement lives in the queue.
- Capacity caps of 0 are a feature ("hold all background work"), not an
  error.

## Alternatives considered

- **Per-pool caps (separate semaphores per spawn path)** — rejected: doesn't
  bound the shared resource; three knobs that all have to be right instead of
  one policy.
- **Coalescing duplicate event fires** (ten identical events → one fire) —
  deferred, not rejected: useful but separable; the per-trigger queue ceiling
  + drop-oldest covers the runaway case it was meant for. Revisit if real
  queues show heavy duplication.
- **In-memory-only queue** — rejected: violates engine statelessness; a
  restart under load would silently shed exactly the work backpressure was
  holding.
- **Gating inside `process_message_with_steps`** (single choke point) —
  rejected: by that depth the callers have already emitted placeholder titles
  / `MessageReceived`, so a queued spawn would leave a half-created thread
  visible as "running". Gating at the spawn sites keeps queued work invisible
  until admitted.
- **True preemption (pause background work for user chat)** — rejected:
  killing or suspending a half-finished agent turn loses more than the queued
  wait saves; bypass-plus-don't-count achieves the user-facing guarantee.
