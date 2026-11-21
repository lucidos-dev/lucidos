# 0076: Coding-agent branch names are unique by construction and the create decides, superseding 0041's accepted allocation race

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

ADR 0041 named coding-agent branches for their thread and closed with two
clauses this entry reverses. It accepted a race ("A millisecond-wide allocation
race is accepted") and rejected pre-creating the ref to close it.

The race fired for real. Six coding-agent children spawned in one response,
against one repo, with six prompts opening on the same words. Two survived. Four
died before doing any work on `fatal: a branch named '...' already exists`, and
two of those four had already reached the `-2` suffix.

Two assumptions in 0041 turned out to be wrong.

**The window is not milliseconds.** `allocate_coding_agent_branch` reads
`for-each-ref` and returns a name; `git worktree add -b` runs much later, after
`resolve_worktree_path`'s database query and a `git worktree list`.

**The slug is not a discriminator.** It is the prompt's opening words, and
parallel partitions of one job legitimately share them. Fanning out N children
on one instruction is a normal thing to do here, so the collision is the
expected case, not an unlucky one.

## Decision

Two changes, both required, neither sufficient alone.

**The name carries the thread's short id.** The shape becomes
`lucidos-<agent>-<app|repo>-<name>-<slug>-<short thread id>`, reusing the 8 hex
chars that already name the worktree directory (`thread-<id>`). Two threads
cannot derive one name however alike their prompts are.

**`git worktree add -b` is the allocator of record.** A create that loses the
race re-derives the next free name and retries, bounded at 10 attempts, with a
clear error only on exhaustion. Allocation is a proposal, not a reservation.

0041's naming decision stands otherwise: same prefix, same scope segment, same
slug rules and cap, same `-<n>` numbering, and existing branches are still never
renamed.

## Rationale

**Retry alone would have been enough for correctness, and still wrong.** Six
siblings would serialize into `-2`, `-3`, `-4`, `-5`, `-6`, each paying a failed
`worktree add` and a re-listing. Worse, the numbering carries no meaning: which
partition is `-4` is unanswerable. The id makes the common case contention-free
and the branch legible.

**Uniqueness alone would not have been enough.** A thread can mint twice, and a
`for-each-ref` that times out falls back to a name it did not verify. More
fundamentally, no pre-check can be authoritative: the code that reads and the
code that creates are separated by other work.

**Git's own pre-flight check proves the point.** The regression test surfaced a
third failure message, `cannot lock ref 'refs/heads/x': reference already
exists`, which is git losing the same race one layer down. Its "does this branch
exist" check is itself a check-then-act. So an engine-side pre-check could never
have been made authoritative, whatever care it took.

**The id is the thread's, not random.** A random suffix would also be unique.
But a branch would stop pairing with its worktree directory by eye, and
re-minting one thread would produce an unrelated name each time.

## Consequences

- **Concurrent spawns from one prompt no longer collide.** Two regression tests
  cover it. One runs 8 spawns with distinct ids and an identical title. The
  other runs 8 sharing one id and title, which defeats the suffix so only the
  retry can save them. Both fail against the pre-fix behavior.
- **Names grow by 9 characters** (`-` plus 8 hex). The 48-char slug cap is
  unchanged.
- **`-<n>` numbering now means one thread minting twice**, never two threads
  colliding. It is kept, since a thread whose branch survives an Apply can mint
  again.
- **`branch_name_is_taken` is the one place that knows git's name-taken
  messages**, and it stays deliberately narrow. A worktree *path* that already
  exists must not match it: re-deriving the branch name would burn all 10
  attempts on a failure the name has nothing to do with.
- **The app sparse-checkout path gains no retry, and needs none.**
  `create_sparse_app_worktree` deliberately *adopts* an existing branch, which is
  what makes resume work. That was quietly dangerous before: two app threads
  racing on one name would have had the loser adopt the winner's branch rather
  than fail. Uniqueness closes it, and a retry there would break resume.
- **One `SHORT_THREAD_ID_LEN` and one `short_thread_id`**, in `git_ops`, now feed
  both the branch and the worktree directory. They were two literal 8s before and
  would have drifted.
- **Failure-path cleanup is unaffected.** A `-b` that loses the race creates no
  ref. A retry therefore leaves nothing behind and never touches the winner's
  branch, and `branch_created` still means "this attempt created the branch it
  returned".

## Alternatives considered

**Retry only, keeping the slug as the discriminator.** Correct, and rejected on
cost and legibility: see Rationale.

**Unique id only, no retry.** Rejected: a pre-check cannot be authoritative, and
git's own pre-flight check losing the race is the proof.

**Pre-create the ref during allocation.** Still rejected, for 0041's original
reason. It blurs "did this attempt create the branch", which is exactly what
`cleanup_failed_spawn` needs in order to decide what it may delete. The retry
gets the same guarantee without touching that answer.

**Serialize spawns behind a lock or a queue.** Rejected on two counts. It builds
an engine-wide bottleneck for a problem a nine-character suffix removes. And a
lock cannot span the process boundary that git itself races over.

**A random suffix instead of the thread id.** Rejected: see Rationale.

**Renaming existing branches to the new shape.** Rejected, as in 0041. A live
branch name is recorded in `changes.branch_name` and in every `SessionStarted`
event, and may be checked out in a live worktree.
