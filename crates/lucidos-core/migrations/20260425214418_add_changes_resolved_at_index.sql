-- Supports list_recently_applied / list_for_repo / requires_restart_since /
-- client_update_since / restart_groups_since — all of which sort or filter
-- applied/reverted rows by resolved_at on the /api/changes hot path.
CREATE INDEX IF NOT EXISTS idx_changes_resolved_at_applied
  ON changes (resolved_at DESC)
  WHERE status IN ('applied', 'reverted');
