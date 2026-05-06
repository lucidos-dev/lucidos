-- Track whether a child thread has already sent its completion callback
-- to the parent. Prevents duplicate callbacks when CC emits multiple
-- ClaudeCodeIdled events (auto-harden, background agents, etc.).
ALTER TABLE thread_summaries ADD COLUMN IF NOT EXISTS parent_callback_sent BOOLEAN NOT NULL DEFAULT FALSE;
