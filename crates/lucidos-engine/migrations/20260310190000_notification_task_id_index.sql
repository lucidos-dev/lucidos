-- Index for dedup query: has_recent_error_notification(task_id, created_at)
CREATE INDEX IF NOT EXISTS idx_notifications_task_created
ON notifications (task_id, created_at DESC);
