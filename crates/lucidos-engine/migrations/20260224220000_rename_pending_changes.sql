ALTER TABLE pending_changes RENAME TO changes;
ALTER TABLE changes RENAME COLUMN has_rust_changes TO requires_restart;
DROP INDEX IF EXISTS idx_pending_changes_status;
CREATE INDEX idx_changes_status ON changes (status);
