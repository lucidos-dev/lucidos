-- Add skill_id and args columns to scheduled_tasks
ALTER TABLE scheduled_tasks ADD COLUMN IF NOT EXISTS skill_id TEXT;
ALTER TABLE scheduled_tasks ADD COLUMN IF NOT EXISTS args JSONB;
