# 0345: /harden merges main into the branch before it reviews, so one hardening run certifies the work and the merge; amends 0241

- **Status**: Accepted
- **Date**: 2026-10-02

## Context

The user reported that merge conflicts take a lot of time, and asked whether
running agents could merge in changes as they are applied.

Over the 8 days before this decision, the dev workspace saw:

| Measure | Value |
|---|---|
| Applies | about 600 |
| Applies that hit a conflict | 34 (about 5%) |
| Conflict to landed, median | 20 min |
| Conflict to landed, p90 | 81 min |
| Total time in conflict resolution | about 20 h |
| Proposed to applied, median | 2.4 min |

Tool-call timelines of resolver sessions show the merge itself done in about
two minutes. The rest is the `/harden` run that `build_merge_prompt` requires
afterwards. So a conflict costs a **second hardening run**, done by a resolver
with no context, after the thread's own agent already hardened the same work.

A change sits pending only minutes. So most conflicts come from `main` moving
while the thread worked, before its own `/harden`.

ADR 0241 had removed the engine's spawn-time merge of `main` and predicted
this: "more changes take the conflict path".

## Decision

`/harden` merges `main` into the branch at its start (Phase 0.3), once a run is
needed and before any review or suite. The session's own agent resolves any
conflict. The engine still never merges `main` outside Apply: ADR 0241's
spawn-path decision stands, and `catchup_with_main` stays private.

## Rationale

- **It removes the second hardening run.** One run now certifies the work and
  the merge together. Apply's catch-up then finds `main` moved by minutes at
  most, so it is almost always clean, and no resolver spawns.
- **The resolver with the most context does the merge.** The thread's own agent
  wrote the change and knows its intent. A detached resolver has to rebuild it.
- **ADR 0241's objection does not reach this.** It rejected a catch-up before
  hardening because Apply merges again afterwards, so the hardened tree is never
  guaranteed to be the landing tree. That stays true. The gain claimed here is
  not a guarantee but a hardening run saved.
- **ADR 0241's other costs do not apply either.** That merge fired whenever the
  engine spawned a process, and it rewrote a resumed session's files behind its
  back. This one is a step the agent runs itself, at a point it chose, inside
  work that will be tested next.

## Consequences

- A conflicted change normally meets its conflict inside its own `/harden`,
  not at Apply.
- Apply's catch-up and the Tier 1/2/3 conflict path stay. They are the net for
  `main` moving between hardening and Apply.
- An already-hardened branch skips the merge, because Phase 0 stops first. Its
  marker stays fresh.
- `/harden` takes slightly longer when the start merge conflicts. That time was
  previously spent at Apply, on top of a second run.
- Merge commits: one per hardening run where `main` moved, and Apply adds one
  only if `main` moved again. So a change carries up to two, where it carried at
  most one before. That is the accepted price, and still far below ADR 0241's
  per-spawn rate.
- A resolver's own `/harden` runs the start merge as a no-op, because the
  resolver just merged.

## Alternatives considered

**Merge `main` into every running agent on each Apply.** The user's first
proposal. Rejected on four grounds:

- The harden marker is keyed on HEAD, and Apply trusts a stale marker. So a
  merge into a hardened, pending branch lands code that no hardening run ever
  tested together.
- About 100 applies a day would rewrite files under live turns in every
  worktree. That is the hazard ADR 0060 names and leaves open.
- It brings back ADR 0241's merge-commit volume.
- A merge that is clean now is almost always clean at Apply, so a clean-only
  variant saves almost nothing.

**Rebase onto `main` instead of merging.** No merge commits. Rejected because a
rebase rewrites the shas: the hardened commit leaves HEAD's history, and
`harden-scope.sh` answers `FULL` on every re-run. It also replays each commit,
so one conflict can surface several times.

**Make the resolver's hardening cheaper.** For example, test only the conflicted
files. Rejected: `CLAUDE.md` says a narrow filter cannot certify a merge that
mixes sides, and the suites are where the resolver catches a semantic conflict.
