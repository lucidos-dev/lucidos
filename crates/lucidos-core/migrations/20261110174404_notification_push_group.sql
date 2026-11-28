-- The push group of a notification (ADR 0649), as `PushGroup` serializes it.
--
-- A notification about a people-only message groups with the others from the
-- same thread: one push tag per thread, so a burst replaces one banner, and an
-- Unwatch action. NULL is an ordinary notification with its own tag. The wake
-- push rebuilds from this row, so the group lives here rather than in memory.

ALTER TABLE notifications ADD COLUMN push_group JSONB;
