-- Serve thread text search from indexes instead of a scan of every message.
--
-- `search_threads_by_text` matched each token with ILIKE over every message
-- payload. On a large workspace the message rows are spread over a multi-GB
-- `events` table, so one search read tens of thousands of heap pages and ran
-- past the client's 10 s timeout.
--
-- The trigram index serves tokens of 3 or more characters. Its expression must
-- match `MESSAGE_TEXT` in `core/store/threads/search.rs` exactly, or the
-- planner cannot use it. The thread index serves the shorter tokens, which are
-- checked only inside threads the longer ones matched. Both are partial on the
-- message events the search reads.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX IF NOT EXISTS idx_events_message_text_trgm
    ON events USING gin (
        (COALESCE(payload->>'text', '') || ' ' || COALESCE(payload->>'content', '')) gin_trgm_ops
    )
    WHERE event_type IN ('MessageReceived', 'ResponseGenerated') AND thread_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_events_message_thread
    ON events (thread_id)
    WHERE event_type IN ('MessageReceived', 'ResponseGenerated') AND thread_id IS NOT NULL;
