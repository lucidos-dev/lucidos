# 0192: A thread delete is the one sanctioned removal from the event log

- **Status**: Accepted
- **Date**: 2026-09-16

## Context

A tester asked for a way to throw a thread away. Archive was not it. An archived
thread ranks identically to a live one in both search arms. Everything said in it
is already dissolved into `memory_entries`, and memory recall reads that before
every turn with no archive filter. So the pile of garbage threads stayed
searchable, and stayed in the agent's mouth.

The codebase says events are immutable and append-only, and that `EventBus::emit`
is the only writer. Before this change no production path removed an `events`
row anywhere. The only `DELETE FROM events` in the tree was test cleanup.

That invariant reads as a promise about permanence. It is not one. It says the
**engine** never rewrites history behind the user's back: no compaction, no
redaction, no silent repair of a row somebody did not like. It was never a
promise that the user cannot destroy their own data.

`docs/plans/2026-09-15-deleting-a-thread.md` settles the design.
`docs/plans/2026-09-16-implement-thread-deletion.md` makes it executable.

## Decision

**A thread delete is the single sanctioned removal from the `events` table, and
it is reachable only from the owner's UI.**

One route, `POST /api/v1/threads/delete`. It admits one caller shape, a resolved
`MessageOrigin::Device`, and refuses every other. Five are refused:

- an agent subprocess presenting its thread-bound origin token,
- the machine-local token,
- a caller presenting no credential at all,
- a device id naming no row,
- an app document, by its `Referer`.

There is no LLM tool, no CLI verb, no SDK method, and no path from a thread
acting in its own subtree.

**A device header is attribution, not authentication** (ADR 0050). Straight to
the engine's loopback port it is whatever the caller typed. `/devices/register`
is open by construction, since a device is not in the table until it registers.
So a local process that drops its origin token and registers a device of its own
reaches this route. That residual is the workspace's posture for every mutating
endpoint, Apply and Discard included, and closing it is a workspace-wide change
rather than this one's. What this route does add is that dropping the agent token
buys nothing else: the gate asks the strict probe rather than the fail-open one,
so a database blip cannot promote an invented id.

One event survives the removal. `ThreadsDeleted` sits on aggregate `ops` with
`aggregate_id` `global`, so no later thread sweep can reach it. It records ids
and counts, never anything that was said.

## Rationale

**The user owns their data, and the engine owns its own honesty.** Those are two
different promises, and only the second is what append-only was ever protecting.
A user who deletes a thread is not rewriting history. They are exercising the one
authority the append-only rule was never about.

**Soft delete would have moved the cost onto every future read path.** The bytes
would stay, which is the exact thing the report objected to. Correctness would
become "did you remember the filter", forever, including in paths nobody has
written yet. Hard delete has no filter surface at all.

**Owner-only is the whole safety model, so it is a gate and not a convention.**
There is no undo, so the only defence is that nothing except a person at their
own client can reach the route. That rules out the ordinary thread-reach ladder,
which admits an agent carrying the owner's standing instruction. The gate asks a
narrower question: is the resolved actor a registered device?

**The audit record has to survive the thing it records, and say nothing about
it.** On aggregate `ops` it is out of reach of every thread sweep, including a
later delete. Carrying a title or a first message would re-file the content the
delete just removed, which is why it carries counts.

## Consequences

**A thread's rows are gone, and so is what the workspace learned from it.**
`memory_entries` rows sourced to the family's events go first. Their `source` is
an event id, so the events are the only way to find them.

That drifts both ways, and both are accepted. A fact two threads said survives
under the kept thread's source. A fact a kept thread taught can die with the
deleted one, because dedup superseded the older row. There is no reconciler.

**Delete reclaims the coding-agent worktree and branch itself**, because once the
events are gone nothing can resolve the directory back to a thread. ADR 0035
carries the amendment.

**Four things are deliberately left behind.**

- A surviving parent's own events still name the deleted child in their payloads,
  for example a `ChildThreadCompleted`. Those rows are the parent's record of
  what it did, and removing them would rewrite a thread the user kept. The link
  resolves to "thread no longer exists", which the client already renders.
- Image bytes under `data/blobs/`, which are content-addressed, deduped and carry
  no refcount. Nothing reaches them once the thread is gone. A refcount plus a
  sweeper is its own change.
- Artifacts under `data/artifacts/` and any applied merge commit. Both are
  git-tracked, and policy is that neither is ever auto-deleted.
- A live `apply_all_batches` row naming one of the deleted `changes`. The table
  stores change ids and no thread id, and the driver's recovery already reads a
  missing change row as terminal. Editing the array would be a write with no
  reader.

**Backups still hold the thread.** They are nightly encrypted archives and
nothing in Lucidos can reach them. The confirmation says so rather than implying
the delete reached further than it did.

**The `events` table gains a second writer.** `core/announced_surfaces.rs` now
names `api/threads/delete.rs` beside `engine/event_bus/mod.rs`, and carries the
same entry for every projection table the cascade sweeps. A future removal
path has to pass that scan, which is the forcing function that keeps this
exception single.

## Alternatives considered

**Leave archive as the only exit.** It is what exists, and it is what the report
rejected. Archive hides a thread from one drawer section and changes nothing
about retrievability.

**Soft delete behind a flag.** Cheapest and reversible. Rejected: the bytes stay,
and every read path in the codebase, present and future, becomes responsible for
remembering the filter.

**Redaction: keep the row spine, blank the payload.** Preserves every reference
and the log's shape. Rejected: it leaves a husk thread that every list and read
path must learn to hide, which is the filter surface again with extra steps.

**A trash that empties after N days, or a few seconds of undo.** Rejected for the
same filter-surface reason, plus a sweeper and a screen. The undo variant is
worse. The engine would hold a pending delete in memory, and a restart inside the
window would silently not delete, with the user unable to tell.

**Make it agent-reachable, gated on the owner's standing instruction.** This is
the shape every other clause-4 verb uses, so it was the obvious one. Rejected: a
standing instruction is granted to any thread whose newest turn-start event
carries a `Device` actor. An agent working inside a turn the user opened would
inherit an irreversible delete. Nothing an agent does needs it.

**Rebuild memory after the delete to repair the over-delete.** Correct, and
unaffordable. `sources_indexed` counts a source as indexed only while a row
points at it. So every event ever skipped or superseded already reads as
un-indexed, which makes the incremental rebuild an LLM sweep over the whole
workspace.
