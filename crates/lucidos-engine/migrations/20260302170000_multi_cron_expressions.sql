-- Migrate scheduled_tasks from single cron_expression TEXT to multi cron_expressions JSONB.
-- Each existing cron_expression is wrapped in a JSON array: "0 0 8 * * *" → ["0 0 8 * * *"]

ALTER TABLE scheduled_tasks ADD COLUMN IF NOT EXISTS cron_expressions JSONB;

UPDATE scheduled_tasks
SET cron_expressions = jsonb_build_array(cron_expression)
WHERE cron_expressions IS NULL AND cron_expression IS NOT NULL;

-- Set a default for new rows and make NOT NULL
ALTER TABLE scheduled_tasks ALTER COLUMN cron_expressions SET DEFAULT '[]'::jsonb;
ALTER TABLE scheduled_tasks ALTER COLUMN cron_expressions SET NOT NULL;

-- Drop the old column
ALTER TABLE scheduled_tasks DROP COLUMN IF EXISTS cron_expression;
