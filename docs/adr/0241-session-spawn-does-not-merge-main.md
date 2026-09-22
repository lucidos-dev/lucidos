# 0241: The session spawn path does not merge main

- **Status**: Accepted
- **Date**: 2026-09-21

## Context

The engine used to run `git merge main --no-edit` inside a thread's worktree on
every spawn that reused a branch or worktree. That covered every follow-up turn
and every restart auto-resume. App spawns and external repos already skipped it.

It was added in `3e28061ec` for a concrete incident. A broad-stop bug in
`populate.sh` was fixed on `main`, and it kept recurring because resumed
worktrees still ran the pre-fix copy of the script.

`main` moves in exactly one place, `ff_main_to`, reached only from Apply. Apply
notifies no other worktree. So the spawn-time merge was pull-side compensation
for an event it had no connection to. It ran at whatever moment the engine next
happened to spawn a process.

## Decision

Only Apply may merge `main` into a thread's branch. Nothing on the session spawn
path does.

`catchup_with_main` is private to `git_ops::merge` to keep that true. Its two
callers, `catchup_and_ff_to_main` and `ff_merge_to_main`, are both Apply's.

## Rationale

Two arguments were weighed. One holds and is small, the other does not hold.

**"It keeps the gates current" holds, but describes an edge case.** A stale
worktree runs a stale `scripts/` and a stale `.claude/`, so `/harden` can
certify a change against outdated rules. That needs three things at once: an old
worktree, a changed gate, and a gate that matters for this change. Rescuing that
when it bites is cheaper than paying a merge on every spawn to pre-empt it.

**"It makes the hardened tree the landing tree" does not hold at all.** Apply
merges `main` again after hardening, inside `catchup_and_ff_to_main`, and
re-runs nothing. The hardened tree is therefore never guaranteed to be the
landing tree. A spawn-time merge only shrank that window by a random amount, at
a moment set by the engine's lifecycle. That is not a guarantee.

The cost was not an edge case. When the decision was taken, 3,552 of `main`'s
20,879 commits carried the subject `Merge branch 'main' into …`. Apply is a
fast-forward, so a branch's history becomes `main`'s history verbatim, and
`git log --first-parent main` was unreadable. The merges also rewrote files
under a resumed session whose context predated them. On a conflict the engine
injected a note telling the agent to stop and resolve a merge, ahead of the work
it was interrupted doing.

## Consequences

**Kept.** Apply still catches up, because a fast-forward needs `main` as an
ancestor. Conflicts still surface, at Apply, where the Tier 1/2/3 conflict path
and its resolution session already live. A pending change's file list stays
correct: it is computed from `{base}...{branch}`, the 3-dot form, which ignores
what `main` gained after the branch point.

**Given up.** A long-lived worktree keeps whatever `scripts/` and `.claude/`
it was born with, so a thread can harden against outdated gates. The same
channel also carries safety fixes, and ADR 0025's host-kill guard lives entirely
in `scripts/lib/ports.sh`. We accept that and fix it when it happens.

What bounds the safety case is the engine system prompt, compiled into the
binary rather than read from the worktree. The process-safety prohibition sits
on both surfaces on purpose, and `./scripts/check-prompt-mirror.sh` fails if
either half goes missing. So a stale worktree still gets the warning even while
it holds the old script.

A worktree also keeps an older `package.json` and lockfile while its
`node_modules` can be newer. The link is not per-spawn: `spawn_context.rs` skips
it when the worktree already has an install marker. The trigger is Tier 1 of
`worktree_cleanup`, which strips `node_modules` from a worktree idle past
`TIER_1_IDLE`. The next spawn then links `main`'s current tree beside the
worktree's older lockfile. A dependency removed on `main` leaves the two
disagreeing, and the build says so.

**Changed shape.** A long-lived thread now meets one larger merge at Apply
instead of several small ones, so more changes take the conflict path.

**Still there.** Apply creates a catch-up merge when `main` has moved, and
`is_internal_auto_commit` does not match it, so it reaches the user-facing
commit list. Volume drops from one per spawn to at most one per Apply. The user
weighed filtering it alongside the deletion and chose the deletion alone.

## Alternatives considered

**Move the catch-up to just before hardening.** Rejected. It rests on the second
argument above, which does not survive: Apply merges again afterwards, so this
buys no guarantee either. It also builds machinery to pre-empt an edge case we
chose to rescue instead.

**Carve out the restart case only.** Rejected. The merge was never keyed to
restart. It fired on any spawn reusing a worktree, and a restart resume is one
instance of that. A carve-out would encode the engine's lifecycle into a rule
about `main`, which is the confusion that made the behavior look arbitrary.

**Filter catch-up merges from user-facing commit lists.** Not rejected, just not
in scope here. The deletion stops most of them being created at all, so the
remaining ones are one per Apply. Revisit if that is still noisy.

**Keep it and do nothing.** Rejected. The behavior rewrites a resumed session's
files for a reason unrelated to the session, and 17% of `main`'s history was the
receipt.
