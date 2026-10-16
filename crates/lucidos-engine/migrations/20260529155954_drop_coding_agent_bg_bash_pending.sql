-- Drop the coding_agent_bg_bash_pending projection column.
--
-- It backed the "CC waiting on background tasks" wait state: the engine
-- refused to propose a change at idle while a CC background bash task was
-- (thought to be) pending, surfacing this column → a passive banner with no
-- Apply button until a 5-minute nudge fired. That gate was removed — changes
-- now propose the instant CC idles, and correctness is covered by
-- harden-at-apply — so the column has no remaining consumer.
ALTER TABLE thread_summaries DROP COLUMN IF EXISTS coding_agent_bg_bash_pending;
