-- Per-operator read state for Notifications and Review (ADR 0623, decision 11).
--
-- A gateway with fewer than two people keeps using `notifications.read` and
-- `thread_summaries.read_requested` exactly as before. With two or more, a
-- device that an operator holds reads for its person only:
--
-- - `notification_reads` records which device read which notification. A
--   person has read a notification when any of their devices has.
-- - `notifications.audience` holds the devices a notification was sent to.
--   NULL means the whole team. Outside the audience counts as read.
-- - `thread_summaries.reply_seen_devices` holds the devices that saw the
--   reply a pending read request points at. A new request empties it.
--
-- Device ids, never operator ids: membership resolves through the gateway's
-- current mappings at read time.

ALTER TABLE notifications ADD COLUMN audience TEXT[];

CREATE TABLE notification_reads (
    notification_id UUID NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    read_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (notification_id, device_id)
);

ALTER TABLE thread_summaries
  ADD COLUMN reply_seen_devices TEXT[] NOT NULL DEFAULT '{}';
