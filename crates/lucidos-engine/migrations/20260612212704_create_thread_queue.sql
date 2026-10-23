-- Thread Queue projection: one row per background spawn the admission
-- control accepted (status = 'admitted', the persisted active-session
-- record) or is holding until capacity frees (status = 'queued').
-- Maintained by the EventBus projection from ThreadQueued /
-- ThreadQueueAdmitted / ThreadQueueDropped / ThreadQueueCompleted.
-- Rows are deleted on drop/completion — the events table keeps the audit
-- trail; this table holds only the live queue + active set.
CREATE TABLE thread_queue (
    id UUID PRIMARY KEY,
    -- Capacity bucket: 'event-trigger' | 'cron' | 'sub-thread' | 'coding-agent'
    kind TEXT NOT NULL,
    -- Set for trigger kinds; used for per-trigger caps and FIFO.
    trigger_id TEXT,
    trigger_name TEXT,
    -- Bound thread (sub-thread / coding-agent kinds know it at submit time;
    -- trigger kinds never bind — each fire creates its own thread).
    thread_id UUID,
    -- Human preview for the Thread Queue panel.
    summary TEXT NOT NULL DEFAULT '',
    -- Full re-executable spawn request (ThreadQueueRequest serde JSON), so a
    -- restart can re-fire entries that never ran to completion.
    request JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'admitted')),
    queued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    admitted_at TIMESTAMPTZ,
    -- FIFO order within the queue (global arrival order).
    sequence BIGSERIAL
);

CREATE INDEX idx_thread_queue_status ON thread_queue (status);
CREATE INDEX idx_thread_queue_trigger ON thread_queue (trigger_id) WHERE trigger_id IS NOT NULL;
