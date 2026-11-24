-- The thread whose agent created this draft (ADR 0665). NULL for a draft a
-- person opened in the composer. Projected from `ThreadStarted.agent_draft`.
ALTER TABLE thread_summaries ADD COLUMN drafted_by_thread_id UUID;
