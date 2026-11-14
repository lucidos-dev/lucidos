-- E2E-only push log.
--
-- The migration runs unconditionally so production schemas match e2e ones,
-- but the only code path that writes here is gated behind the
-- `e2e-test-hooks` cargo feature (see
-- crates/lucidos-engine/src/scheduler/push_test_log.rs). Production
-- binaries never insert into this table — the real web-push transport
-- POSTs to APNs/FCM directly. Tests read from it via
-- GET /api/_test/push-log (same feature gate) to assert
-- "OS push WAS sent for this notification on this device" without
-- waiting for actual push-service delivery.
--
-- See system-knowhow/notifications.md §5.4 for the harness this backs.
CREATE TABLE IF NOT EXISTS push_log (
    id              BIGSERIAL PRIMARY KEY,
    device_id       TEXT NOT NULL,
    notification_id UUID NOT NULL,
    sent_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS push_log_sent_at_idx ON push_log (sent_at);
CREATE INDEX IF NOT EXISTS push_log_notification_id_idx ON push_log (notification_id);
