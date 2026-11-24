-- Enforce at most one pending change per branch at the database level.
-- This prevents TOCTOU races where concurrent sessions both check and insert.
CREATE UNIQUE INDEX idx_changes_unique_pending_branch
ON changes (branch_name)
WHERE status = 'pending';
