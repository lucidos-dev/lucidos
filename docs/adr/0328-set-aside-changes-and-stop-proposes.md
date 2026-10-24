# 0328: A change can be set aside, and a Stop proposes the branch's work

- **Status**: Accepted; "a Stop proposes the branch's work" is superseded by
  [ADR 0400](0400-coding-agent-change-state.md). Set aside still holds.
- **Date**: 2026-09-29

## Context

A pending change had two ways out: Apply or Discard. Until then it sat in the
Review list and in Apply All, and it blocked Archive. So a user who wanted to
keep a change for later, but out of the way, had no move.

A coding-agent thread wrote a plan, committed it, and asked for approval. The
user pressed Stop on the question card and archived the thread. Stop proposed
nothing, because `may_touch_change_state_at_idle` admits only a `Generated`
terminal. With no pending change, Archive went through. `ThreadArchived` then
cleared `coding_agent_has_diff`, and the plan commit dropped off every surface.
The branch survived, but nothing in the product pointed at it.

Plan: `docs/plans/2026-09-29-set-aside-pending-changes.md`.

## Decision

1. **A change can be set aside.** `set_aside` is a change status beside
   `pending`, reached by `ChangeSetAside` and left by `ChangeBroughtBack`. A
   set-aside change is out of Review, attention, Apply All and Discard All, and
   it does not block Archive. Apply refuses it until it is brought back.
2. **A user Stop proposes the branch's net work** as an `incomplete` change,
   on every Stop path. A later clean idle re-proposes the same id and clears
   the flag.
3. **Archive never orphans branch work.** After `ThreadArchived`, a subscriber
   proposes any unproposed net work on the thread's branch and sets it aside.
   Boot runs the same routine over the agent branches.
4. **Bulk applies skip an `incomplete` change.** Only a per-change Apply, which
   confirms, lands one.

## Rationale

**A status, not a flag.** `ChangeState` already makes each status carry only
its own data. A `set_aside` flag beside `pending` would allow a set-aside
change resolving a merge conflict, or held by an apply claim.
`sync_thread_proposal` already counts only `pending` rows. So
`coding_agent_proposed`, and everything it drives, follows the new status with
no new column.

**Bring back before Apply.** Every apply path guards on `pending`: the
per-change route, Apply Now, standing applies and Apply All. Letting them apply
a set-aside change would teach each guard a second status. One tap to bring it
back costs less.

**Stop proposes, reversing the Continue-only recovery.** The held-back sweep
in `has_diff.rs` skipped an interrupted turn, so that Continue would pick it
up. That left an interrupted thread's commits with no Apply, no Discard, and
nothing blocking Archive. The `incomplete` flag and its confirm already exist
for partial work, so proposing is safe to surface.

**The archive net runs after the archive.** `archive_family` deliberately does
no git work: per-member git teardown once made a big archive take about 60
seconds. A subscriber on `ThreadArchived` keeps the request fast. It sets the
work aside rather than blocking, since the user has already said "out of my
way".

## Consequences

- A stopped coding-agent thread with commits shows Apply, with the incomplete
  confirm. Archiving it needs Apply, Discard or Set aside first.
- An archive that finds unproposed work leaves a set-aside change. The Changes
  panel lists it, and the user can bring it back or discard it.
- The boot pass skips any branch with a `changes` row. It never resurrects a
  discarded or reverted change. It can miss work committed after an applied
  change on the same branch.
- A new proposal on a set-aside change's branch brings it back: the agent's new
  work is worth the user's eyes.

## Alternatives considered

**"Park" or "Shelve".** A *parked* thread already means one waiting on a
question or an event wait (ADR 0106, ADR 0293). "Shelve" is IDE jargon; the
user chose plain English. "Restore" was ruled out as the reverse, because it
means restoring a workspace from a backup.

**Add unproposed work to `is_blocking`.** Archive would refuse a thread with
`coding_agent_has_diff` and no proposal. It needs a new projection input, its
SQL mirrors and a backfill. It also leaves a dead end: no change exists to
Apply or Discard. Proposing at Stop removes the state instead.

**Setting aside also archives the thread.** One tap instead of two, but it
couples two actions. The user chose to keep them separate.
