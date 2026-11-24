-- The continue key a trigger fire claimed for this thread (ADR 0623).
-- Written by the TriggerStarted projection and cleared by ThreadArchived,
-- so every row that holds a key is an open thread. The unique index makes a
-- second open thread with the same key impossible.
ALTER TABLE thread_summaries ADD COLUMN continue_key TEXT;

CREATE UNIQUE INDEX thread_summaries_continue_key_unique
    ON thread_summaries (continue_key)
    WHERE continue_key IS NOT NULL;
