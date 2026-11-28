-- Permanent retry receipts. Queue membership is live; these survive execution
-- and later compose edits. The accepted event is the immutable payload/audit.
CREATE TABLE draft_submissions (
    retry_key UUID PRIMARY KEY,
    thread_id UUID NOT NULL,
    revision BIGINT NOT NULL CHECK (revision >= 0),
    epoch BIGINT NOT NULL CHECK (epoch >= 0),
    event_id UUID NOT NULL UNIQUE REFERENCES events(id) ON DELETE CASCADE,
    queue_entry_id UUID NOT NULL UNIQUE,
    UNIQUE (thread_id, revision, epoch)
);
