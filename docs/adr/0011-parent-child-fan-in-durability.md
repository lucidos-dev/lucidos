# 0011 — A blocking child's completion is durably delivered to its parent: the persisted `ChildThreadCompleted` is the source of truth, the in-memory wake is a cache rebuilt on boot

- **Status:** Accepted
- **Date:** 2026-06-16

## Context

A parent coding-agent thread spawned a child thread, idled after reporting
"spawned successfully", and then **never resumed** to process the child's
completion — even though the child finished successfully (dev-workspace thread
`276f5580`, child `af6793c3`). The parent's worktree had already been torn down
by an earlier Apply, so when the child completed there was nothing to resume
into; later the background worktree-cleanup removed the residual stub. A separate
frontend fix already stopped the resulting phantom "Working" badge; this ADR is
the backend root cause.

### The fan-in mechanism (verified against current code)

A *child thread* is a thread spawned with `Relation::Child` — `run_thread` /
`run_coding_agent` with `relation: "child"` (the default), or `lucidos thread
spawn --relation child`. The contract the tool descriptions advertise:

> "when the spawned thread finishes, this thread automatically resumes with its
> result."

When a child reaches a terminal event,
`notify_parent_if_child` (`event_bus/parent_callback.rs`), in the PostCommit
phase of `EventBus::emit`, does two things:

1. **Persists** a typed `ChildThreadCompleted` event on the *parent* thread (the
   durable record — its projection arm also flips the child's
   `parent_callback_pending` in the same transaction).
2. **Fires** an in-memory `ParentCallback` over an unbounded mpsc channel
   (`parent_callback_tx`). The consumer
   (`start_parent_callback_listener` → `notify_parent_of_child_completion`)
   resumes the parent's agent session (`process_message_with_steps`, anchored to
   the `ChildThreadCompleted` event id) so the parent reacts to the completion
   card.

The channel is **in-memory only**. On engine restart `parent_callback_rx` is
recreated empty, so an un-consumed wake is lost. There was **no boot-recovery
sweep** that re-derives the wake from the persisted `ChildThreadCompleted`
(`notify_parent_of_child_completion` had exactly one caller — the live
listener). The child's `parent_callback_pending` flag means the callback for
this run is still owed: it is cleared when the wake reaches the channel, not
when the parent processes it.

### Two weaknesses

- **B1 (durability).** The wake lives only in memory with no recovery. An engine
  restart between child-complete and parent-resume (e.g. an Apply that restarts
  the engine) strands the fan-in permanently — violating the engine-statelessness
  rule in CLAUDE.md ("no critical state only in memory"). This is the deepest
  root cause and is *not* addressed by any recent commit.
- **B2 (cleanup guard).** The worktree-cleanup worker
  (`worktree_cleanup.rs`, `try_tier_0` / `try_tier_2`) gates removal on
  idle > grace + clean git + branch-at-main + no-pending-change + not-active. It
  has **no awareness of fan-in obligations**, so a parent's worktree can be
  removed before it processes a child completion — the parent then has nothing to
  resume into.

### Re-baseline against recent commits

Three commits merged just before this work narrow, but do not close, the gap:

- **`975296a82`** (keep worktrees warm while disk is comfortable) — gates all
  reclamation tiers on `archived OR free-disk-below-soft`. A non-archived
  thread's worktree is now kept fully warm regardless of idle age. This is the
  single biggest mitigation of the *original* incident (pre-`975296a82`, Tier 0
  removed a worktree one hour after Apply). **Remaining B2 gap:** once the user
  *archives* the parent, or free disk drops below the soft threshold, the gate
  opens and a parent with an unprocessed completion can still be reclaimed.
- **`d819482c6`** + **`93db58de9`** (never resume into a stranded worktree; gate
  the stranded-clear on positive evidence) — make the resume path *self-heal*: a
  missing or stranded worktree is recreated from the branch (`is_live_worktree_at`)
  rather than running Claude Code against the enclosing data repo. This means a
  re-fired wake (B1) will actually succeed even if the worktree was reclaimed.
  **This does not deliver the wake** — it only ensures that *if* a wake fires,
  the resume lands somewhere valid.

So: B1 is a genuine, uncovered gap (the wake is never re-delivered after
restart). B2 is a genuine, narrowed gap (archived / disk-pressure reclamation
can still race the fan-in). Neither is WONTFIX.

## The design question: which spawns resume the parent?

> Should a parent ALWAYS resume when a child completes, or only when it spawned a
> "blocking" child it is waiting on?

**Resolved by the code: only blocking children resume the parent, and the intent
is already recorded** — by the presence of `parent_thread_id` on the child:

- `Relation::Child` → child's spawning `MessageReceived` carries
  `parent_thread_id = spawning_thread_id`; the projection increments the parent's
  `active_children_count`; `notify_parent_if_child` fires the wake.
- `Relation::Top` (fire-and-forget, the `lucidos thread spawn` default) →
  `parent_thread_id = NULL`; no counter increment;
  `notify_parent_if_child` early-returns on the NULL parent lookup. The tool
  descriptions say so explicitly ("fire-and-forget … this thread does not resume
  when it finishes"), and `test_top_relation_thread_does_not_callback_or_increment_count`
  pins it.

There is **no separate "blocking" flag** — `parent_thread_id IS NOT NULL` *is*
the discriminator, and a `ChildThreadCompleted` is only ever persisted for a
child that had one. So both B1 and B2 scope **automatically** to blocking
spawns: B1 keys off persisted `ChildThreadCompleted` events (which exist only for
children with a parent), and B2 keys off `active_children_count` /
`ChildThreadCompleted` (both NULL/absent for fire-and-forget).

## The durability guarantee

**A blocking child's completion is delivered to its parent exactly once, and the
delivery survives an engine restart.** The persisted `ChildThreadCompleted`
event on the parent is the single source of truth; the in-memory `ParentCallback`
is a cache of "this parent owes a resume" that the engine rebuilds from the event
store on boot. A parent has an *outstanding fan-in obligation* while its latest
persisted event is a `ChildThreadCompleted` it has not yet reacted to; that
obligation keeps its worktree alive and is re-fired on the next boot if the
in-memory wake was lost.

## Decision

### B1 — Boot-recovery sweep that re-derives the lost wake

Add `EventBus::refire_unprocessed_child_completions`, a boot-recovery sweep
mirroring `propose_held_back_changes_on_startup`. It selects every parent whose
**latest persisted event is a `ChildThreadCompleted`** — i.e. nothing came after
the completion card, so the parent never reacted — and re-injects a
`ParentCallback` onto the live `parent_callback_tx`. The already-running listener
drains it through the exact same `notify_parent_of_child_completion` path as a
live completion. `main.rs` calls it once on boot, after the existing recovery
sweeps and before the HTTP server binds.

**Idempotency** is structural, via the event-id anchor: the moment the listener
resumes the parent, the parent emits a fresh terminal event (a higher sequence
than the card), so on the next boot the card is no longer the latest event and
the sweep skips it. A resume that dies mid-flight leaves the parent's last event
as a `SessionStarted` / streamed token — handled by the existing CC auto-resume
recovery, not re-fired here (no double-handling). Re-injecting onto the channel
(rather than calling `notify_parent_of_child_completion` directly) means the
recovery path and the live path are byte-for-byte the same downstream — the
recovery is purely "re-deliver the lost wake", with zero duplicated resume logic.

### B2 — Cleanup skips a parent with an outstanding fan-in obligation

`try_tier_0` and `try_tier_2` (the two full-removal tiers) skip a thread that has
a pending fan-in obligation, detected by `has_pending_fan_in(thread_id)`:

- `active_children_count > 0` — a direct child is still running; the parent will
  resume when it finishes, so it must keep its worktree.
- the thread's **latest persisted event is a `ChildThreadCompleted`** — a child
  has completed but the parent hasn't processed it yet (the exact incident
  window). This is the same predicate B1's sweep selects on.

On any DB error the guard returns `true` (keep the worktree) — reclaiming on
unverifiable state is the unsafe direction, matching the existing tier-0
"treat as pending on error" stance. Tier 1 (strip regenerable build artifacts)
is deliberately **not** gated: it leaves the worktree, branch, and CC-session
resumability intact, so a parent can still resume — it just re-installs deps.

## Rationale

- **Events are the source of truth; in-memory is cache (CLAUDE.md).** The wake
  was the one piece of fan-in state that lived only in memory with no recovery.
  B1 makes the persisted `ChildThreadCompleted` authoritative and the channel a
  rebuildable cache — the same shape as `propose_held_back_changes_on_startup`
  (committed diff is truth; the proposal is rebuilt) and the Thread Queue
  ("restart re-fires admitted-but-dead entries", ADR 0007).
- **`active_children_count` is decremented *before* the parent resumes.** The
  child's terminal event decrements the parent's count in the same transaction
  that persists `ChildThreadCompleted`. So at the instant the parent owes a
  resume, the count is already `0` — which is exactly why B2 needs the
  *unprocessed-completion* predicate in addition to the count: the count guards
  the "children still running" window, the card guards the
  "completed-but-not-processed" window (the incident).
- **`active_children_count`, not `blocking_descendant_count`.** The parent
  resumes on a *direct* child's completion; `active_children_count` is the direct
  signal. `blocking_descendant_count` is transitive (grandchildren) and broader
  than the fan-in obligation — it would retain worktrees for threads that have no
  pending resume of their own.
- **Re-baselined, not duplicated.** `975296a82` already keeps non-archived
  worktrees warm, so B2 only adds retention for the archived / disk-pressure
  reclamation path — precisely the residual gap. `d819482c6` already makes the
  re-fired resume self-heal a missing worktree, so B1 and B2 compose: B1
  re-delivers the wake, the self-healing resume lands it, and B2 keeps the
  worktree alive in the first place when the gate is open.

## Consequences

- One extra read-only sweep at boot (`refire_unprocessed_child_completions`),
  bounded by the number of parents with a trailing completion card — typically
  zero. It re-injects onto the existing channel, so there is no new resume code
  path to maintain.
- The worktree-cleanup worker now does one extra cheap query per
  tier-0/tier-2 candidate (`active_children_count` + latest-event-type). In the
  warm-disk steady state the reclamation tiers don't run at all, so the cost is
  only paid for archived / disk-pressure candidates.
- No event-schema change: no new `ThreadEvent`/`SystemEvent` variant, no payload
  change. `ChildThreadCompleted` already carries everything the re-fire needs.
  The `system-knowhow/*` event surfaces are therefore unchanged; the
  `worktree_cleanup.rs` module docs gain the B2 retention note.
- A parent with an outstanding fan-in obligation keeps its worktree even when
  archived or under disk pressure — a deliberate, bounded retention. It clears
  the instant the parent processes the completion (its next terminal event drops
  both predicates).

## Alternatives considered

- **Persist the wake as a Thread Queue entry** (the explicit alternative).
  Rejected: the `ChildThreadCompleted` event is *already* the durable record;
  adding a second persisted representation of the same fact invites the two to
  disagree (the exact class of bug the in-tx `parent_callback_pending` marker was
  introduced to kill). The boot sweep re-derives the wake from the one source of
  truth with no second write. The Thread Queue earns its persistence because a
  background spawn has no other durable home (ADR 0007); a fan-in wake does.
- **Call `notify_parent_of_child_completion` directly from the boot sweep**
  (skip the channel). Rejected: it duplicates the listener's spawn + error-log
  body and creates a second entry point into the resume path that can drift from
  the live one. Re-injecting onto `parent_callback_tx` keeps exactly one
  consumer of the wake.
- **Gate the reclamation decision once in `run_once` (`may_reclaim`) instead of
  per-tier.** Rejected: it would also suppress Tier 1, which is safe to run on a
  parent awaiting fan-in (it only strips regenerable artifacts). Gating the two
  removal tiers is the minimal correct scope and is testable per tier.
- **Always resume the parent on any child terminal, including fire-and-forget.**
  Rejected: contradicts the documented `Relation::Top` contract and would resurrect
  top-level threads the user detached on purpose. The `parent_thread_id`
  discriminator already encodes the intent; honour it.
**Amended 2026-08-05.** The marker was renamed from `parent_callback_sent` to
`parent_callback_pending` and its polarity inverted (migration
`20260805123727_rename_parent_callback_sent_to_pending.sql`), because the old
name stated a permanent historical fact while the semantics are per-run: the
marker is written again every time the child is revived, so what it actually
tracks is whether the parent has been told about the child's *current* turn. The
three mentions above are updated in place. The rename also makes the
missing-parent retry write honest: there the card IS persisted and only the wake
was skipped, so "no callback was sent" was false while "the parent callback is
still pending" is exactly true. Two further alternatives were weighed and
rejected for the same reason this section already rejects a second persisted
representation of the fan-in:

- **A counter (`outstanding_callbacks` as an integer).** Rejected: a child never
  owes two cards at once, since callbacks are serialized by the child's own turn,
  so any per-child count is 0 or 1. The count that genuinely matters already
  exists on the other side of the edge as the parent's `active_children_count`,
  recomputed from ground truth by `reconcile_parent_active_children_count`. A
  second numeric copy of the same quantity is a thing that can drift from the
  authoritative one.
- **A generation marker (`run_seq` on the child plus `callback_run`, dedup by
  equality).** Rejected: it would make a missed clear structurally impossible
  rather than merely tested against, which is a real gain, but it does not pay
  for itself. A missed clear is not a silent failure mode in this repo, because a
  pending change is reviewed before it merges. Against that it costs a wider
  migration (two columns, one of them monotonic) and an edit to every start arm
  to bump the sequence.

- **WONTFIX — already covered by `975296a82` / `d819482c6` / `93db58de9`.**
  Rejected with evidence: those commits keep worktrees warm and make resume
  self-heal, but **none re-delivers a wake lost to an engine restart** (B1) and
  the warm-worktree gate still **opens on archive / disk pressure** (B2). The gap
  is real on both halves.
