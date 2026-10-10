-- Add branch_has_diff to thread_summaries: true iff `git diff main..branch`
-- is non-empty for the CC thread's worktree. Drives the WaitingBanner Diff
-- button visibility (separate from cc_has_changes, which tracks CC's idle
-- snapshot, and from the existence of a `changes` row, which is the
-- "ready to apply" workflow signal).
ALTER TABLE thread_summaries
  ADD COLUMN branch_has_diff BOOLEAN NOT NULL DEFAULT FALSE;
