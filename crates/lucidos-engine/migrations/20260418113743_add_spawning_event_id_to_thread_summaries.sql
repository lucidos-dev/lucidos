-- Track which event in the parent thread triggered a system-spawned thread.
-- Set when MessageReceived has sender='system' and spawning_event_id provided.
-- Always NULL for user-spawned threads.
ALTER TABLE thread_summaries ADD COLUMN spawning_event_id UUID;
