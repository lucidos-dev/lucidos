-- How far the Claude gateway telemetry exporter has read the event store, at
-- most one row.
--
-- `sent_through` is an events `sequence`: every gateway capture at or below it
-- was sent, or skipped because no telemetry applied to it. It advances only
-- after a collector answered 2xx, so a restart resends nothing and drops
-- nothing. The exporter seeds it at the committed horizon on its first run.

CREATE TABLE IF NOT EXISTS claude_gateway_telemetry_cursor (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    sent_through BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
