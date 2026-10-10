-- Rename skill_id to app_id in notifications table (consistent with pinned_apps rename)
ALTER TABLE notifications RENAME COLUMN skill_id TO app_id;
