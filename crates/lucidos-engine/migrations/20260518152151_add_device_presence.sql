-- Tracks which devices currently have a visible Lucidos tab.
-- One row per device. The frontend posts visible/hidden + heartbeats every 30s
-- (mirrors thread_presence). Used by cross-device push suppression: if any
-- device is currently visible, skip the push to ALL devices — the active
-- device receives the in-app toast via the NotificationCreated SSE channel.
--
-- Distinct from thread_presence: that one is scoped to a specific thread
-- (only suppresses push when the user is viewing the thread that produced
-- the notification). device_presence is "is the user looking at Lucidos at
-- all on any tab, anywhere", independent of which thread is focused.
CREATE TABLE IF NOT EXISTS device_presence (
    device_id TEXT PRIMARY KEY,
    visible_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_device_presence_visible_at ON device_presence (visible_at);
