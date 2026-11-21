-- A pending change is "incomplete" when the originating CC turn ended in
-- ResponseFailed (e.g. mid-stream API drop): the worktree state is whatever
-- CC happened to dirty before the failure, not a deliberate completion.
-- The frontend reads this column to surface a confirm-before-Apply warning
-- so the user knows they're about to land partial work.
ALTER TABLE changes
    ADD COLUMN incomplete BOOLEAN NOT NULL DEFAULT FALSE;
