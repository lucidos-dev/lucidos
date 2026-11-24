-- Durable mention intent. Only an existing message event releases the notice.
CREATE TABLE people_message_notices (
    event_id UUID PRIMARY KEY,
    notice JSONB NOT NULL,
    deliveries JSONB,
    retry_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    retry_attempts INTEGER NOT NULL DEFAULT 0,
    completed BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX people_message_notices_pending ON people_message_notices (event_id) WHERE NOT completed;
-- Notification identity and completion survive a worker or engine restart.
CREATE TABLE people_message_dispatches (
    notification_id UUID PRIMARY KEY REFERENCES notifications(id) ON DELETE CASCADE,
    push_allowed BOOLEAN,
    completed BOOLEAN NOT NULL DEFAULT false
);
-- Each accepted transport target stays completed when another target retries.
CREATE TABLE people_message_push_receipts (
    notification_id UUID NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    target TEXT NOT NULL,
    PRIMARY KEY (notification_id, target)
);
