-- Make notifications general-purpose (not tied to scheduled tasks)
ALTER TABLE notifications ALTER COLUMN task_id DROP NOT NULL;
ALTER TABLE notifications RENAME COLUMN task_name TO title;
ALTER TABLE notifications RENAME COLUMN summary TO message;
ALTER TABLE notifications DROP COLUMN full_result;
