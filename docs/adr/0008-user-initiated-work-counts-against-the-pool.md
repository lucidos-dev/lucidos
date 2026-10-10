# 0008 — User-initiated work shares the one capacity pool: prioritized, but counted and queued (reverses ADR 0007's "user preempts and doesn't count")

- **Status:** Accepted (supersedes the "user chat preempts and doesn't count against capacity" decision of ADR 0007)
- **Date:** 2026-06-14

## Context

ADR 0007 built one *Thread Queue* to gate every **background** spawn against a
shared capacity pool, and deliberately exempted user-initiated chat:
"'Preempts' = bypasses admission **and doesn't count against capacity**."

That exemption re-created the exact failure 0007 set out to kill. Its own
headline rationale is *"One pool, not three — the resources being protected
(model budget, CPU, worktrees) are shared, so the cap must be too. Per-pool caps
would just move the burst to whichever pool is uncapped."* But user-initiated
work — chat responses and user-typed coding-agent threads — burns the **same**
model budget, CPU, and worktrees. By not counting it, 0007 shipped two pools: a
capped background pool and an unbounded user pool the burst can move to. The
title says "one pool"; the system had two, and the panel's "Running" count was a
lie about real load.

The fix is to separate two things 0007 fused into a single "bypass-plus-don't-
count" package: **priority** (who waits for whom) and **accounting** (does it
count toward the ceiling). User work should win on priority without being exempt
from accounting.

## Decision

User-initiated work shares the **one** `max_concurrent_total` pool with
background spawns — it is **prioritized, not exempt**:

- It **counts** against the ceiling. The "Running" panel reflects true load
  (background admits *and* user-initiated, merged).
- It **drains first** — a freed slot goes to a waiting user before a waiting
  background spawn.
- It **ignores the per-kind / per-trigger caps** — those bucket background only.
- It **queues at true pool-max** — when every slot is busy, a person briefly
  waits ("requesting") instead of running unbounded. No eviction of running
  work (same as 0007 — tearing down a half-finished turn loses more than the
  wait saves).
- A new policy field, **`reserved_background`** (default 8), is a floor
  background can *reclaim* ahead of user work so user priority can't starve
  triggers/cron: when a slot frees and background is below the floor with work
  waiting, background takes it before any user waiter; above the floor, user
  wins. `0` = pure user priority (background can be fully starved).

Mechanically, background spawns keep going through `ThreadQueue::submit` (queue
owns execution, persisted in the `thread_queue` projection). User-initiated work
goes through `ThreadQueue::acquire_user_slot` — the chat handler runs it itself;
the queue only gates the start and counts the slot. User slots are **in-memory
only** (see Consequences) and merged into the panel API; a transient
`ThreadQueueChanged` refreshes the panel when only user state moves.

## Rationale

- **Shared resource → shared cap, no exception.** This is 0007's own argument,
  applied consistently. An uncounted lane is an uncapped lane.
- **Priority ≠ exemption.** A person should never wait *behind background work*,
  and almost never wait at all — but "the machine is genuinely full" is a real
  state, and silently running past the ceiling is how you melt it. Priority +
  reclaim-floor delivers the UX guarantee (user waits at most for one background
  task to finish, and only at true pool-max) without the exemption.
- **Reserved floor, not a user cap.** Reserving for background (rather than
  capping the user) lets a user use the *whole* pool when background is idle, and
  only squeezes user down to `max_concurrent_total - reserved_background` when
  background actually has demand. So a person queues only at true pool-max, never
  earlier.
- **No eviction.** Unchanged from 0007 — suspending a half-finished agent turn
  to shave seconds is a worse trade than the queued wait.

## Consequences

- The panel's "Running (N/max)" is finally honest: N includes user-initiated
  work. Queued user work shows in "Queued" (`kind: "user-chat"`, no Run-now /
  Drop — a queued user thread is already prioritized; cancel it from the chat).
- User slots are **not persisted** and **not re-fired** on restart — they are
  ephemeral runtime (a dead response is gone; the person re-sends if they want).
  This is the engine-statelessness rule applied correctly: persist what must
  survive (background entries, for re-fire), keep runtime in memory. Hence the
  in-memory `acquire_user_slot` path + the transient `ThreadQueueChanged` panel
  refresh, rather than writing three `ThreadQueue*` rows on every chat message.
- A person can see "requesting" briefly when the pool is saturated. This is the
  intended, visible back-pressure — the same trade 0007 made for background
  ("turns load spikes into latency, which the panel makes visible").
- Under sustained heavy mixed load, steady state is: background holds its
  reserved floor, user takes the rest with priority, total never exceeds the cap.

## Alternatives considered

- **Keep ADR 0007 (user exempt, uncounted)** — rejected: the inconsistency
  above; an uncounted lane defeats the one-pool guarantee and makes the panel
  lie about load.
- **Count user work but never block it (user uncapped, only background backs
  off)** — rejected: this was the first cut of this change. It keeps the panel
  honest but lets a flood of user work blow past `max_concurrent_total`, so the
  ceiling stops being a real ceiling. The user explicitly wanted "kept in queue
  if pool max reached."
- **Hard-cap user at `max_concurrent_total - reserved_background`** — rejected:
  simpler, but a person would queue at the *user* cap even with free slots the
  reserved zone is holding for absent background demand. The reclaim-floor model
  lets user use the whole pool when background is idle, so a person waits only at
  true pool-max.
- **Persist user slots as `thread_queue` rows (a `user-chat` kind)** — rejected:
  it would reuse the projection/SSE plumbing, but writes three events per chat
  message on the hot path and forces restart-drop / non-re-executable-`request`
  special-casing for work that must never be re-fired. In-memory + a transient
  refresh event is the right home for ephemeral runtime.
- **True preemption (pause background for user work)** — rejected, same as
  ADR 0007: eviction loses more than the queued wait saves.
