-- Add devices table and link push_subscriptions to devices.
-- Devices track per-browser identity for chat message provenance
-- and per-device push notification preferences.

CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    name TEXT,
    user_agent TEXT,
    push_enabled BOOLEAN NOT NULL DEFAULT false,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Add device_id FK to push_subscriptions (existing table).
-- Conditional to handle fresh installs where push_subscriptions
-- might not exist yet (created by init_schema after migrations).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'push_subscriptions') THEN
        IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name = 'push_subscriptions' AND column_name = 'device_id') THEN
            ALTER TABLE push_subscriptions ADD COLUMN device_id TEXT REFERENCES devices(id);
        END IF;
    END IF;
END $$;
