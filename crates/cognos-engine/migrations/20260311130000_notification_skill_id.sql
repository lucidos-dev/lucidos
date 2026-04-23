-- Add skill_id to notifications so clicking a notification can deep-link
-- to the skill that produced it (e.g. morning dashboard).
ALTER TABLE notifications ADD COLUMN IF NOT EXISTS skill_id TEXT;
