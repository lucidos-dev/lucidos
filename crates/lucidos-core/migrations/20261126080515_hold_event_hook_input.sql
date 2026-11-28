CREATE TABLE pending_event_hook_inputs (
    message_event_id UUID PRIMARY KEY,
    thread_id UUID NOT NULL,
    actor JSONB NOT NULL,
    hook_slug TEXT NOT NULL
);
