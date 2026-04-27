-- Migrate legacy 'waiting' section values to 'default'.
-- The 'waiting' section was removed in the thread lifecycle contract simplification.
UPDATE thread_summaries SET section = 'default' WHERE section = 'waiting';
