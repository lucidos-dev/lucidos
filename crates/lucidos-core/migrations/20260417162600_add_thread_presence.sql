-- Tracks which device is currently focused on which thread.
-- One row per device — a device can only be focused on one thread at a time.
-- Used by notification suppression: if a device is currently viewing the
-- relevant thread, skip the push to that device.
CREATE TABLE IF NOT EXISTS thread_presence (
    device_id TEXT PRIMARY KEY,
    thread_id UUID NOT NULL,
    focused_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_thread_presence_thread ON thread_presence (thread_id);
