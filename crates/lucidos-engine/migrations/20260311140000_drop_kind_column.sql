-- Drop legacy kind column — all tasks have been migrated to skill_id.
-- Make skill_id NOT NULL now that migration is complete.
UPDATE scheduled_tasks SET skill_id = 'unknown' WHERE skill_id IS NULL;
ALTER TABLE scheduled_tasks ALTER COLUMN skill_id SET NOT NULL;
ALTER TABLE scheduled_tasks DROP COLUMN IF EXISTS kind;
