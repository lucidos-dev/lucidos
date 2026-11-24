-- A branch holds at most one open change: pending or set aside (ADR 0328).
-- The propose path reuses a set-aside row instead of inserting beside it, and
-- this index makes a second open row unstorable rather than merely unwritten.
DROP INDEX IF EXISTS idx_changes_unique_pending_branch;
CREATE UNIQUE INDEX idx_changes_unique_open_branch
ON changes (branch_name)
WHERE status IN ('pending', 'set_aside');
