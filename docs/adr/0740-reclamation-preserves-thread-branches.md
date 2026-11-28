# 0740: Preserve coding-agent branches when reclaiming thread worktrees

- **Status**: Accepted
- **Date**: 2026-11-19

## Context

Disk Usage could remove a finished worktree and delete its merged branch.
On the next turn, the coding agent treated the missing branch as a loss of
session continuity and started fresh. The background worker could do the same.

## Decision

Keep the thread branch when reclaiming its worktree. This applies to recommended
cleanup, per-row removal, and background cleanup of known thread worktrees.

## Rationale

The worktree holds the files that consume disk space. A surviving branch lets
Lucidos recreate the worktree at the same path and reuse the session ID.

## Consequences

Merged refs accumulate until a separate pruning policy is adopted. Threads
whose branches were already removed still follow missing-branch recovery.
Existing disposal policies continue for temporary merge worktrees, orphan paths,
duplicate worktrees, the landing lane, and explicit thread deletion.
This supersedes branch disposal during thread reclamation in ADRs 0035 and 0458.

## Alternatives considered

- Resume after a missing branch. This would require distinguishing reclamation
  from explicit branch deletion or rename and choosing a trustworthy ref.
- Add a grace window to recommended cleanup. That would delay disk recovery
  and leave the background cleanup path able to break continuity.
