-- Rename scheduled_tasks → trigger_crons (taxonomy overhaul Phase 1)
ALTER TABLE scheduled_tasks RENAME TO trigger_crons;
