-- Bootstrap migration: create all tables that later migrations ALTER/UPDATE.
-- On existing databases these tables already exist (IF NOT EXISTS is a no-op).
-- On fresh databases this ensures tables exist before ALTER migrations run.
-- Schemas here reflect the ORIGINAL form before any subsequent migrations.

CREATE TABLE IF NOT EXISTS events (
    id UUID PRIMARY KEY,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS preferences (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS push_subscriptions (
    endpoint TEXT PRIMARY KEY,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS scheduled_tasks (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT '',
    cron_expression TEXT NOT NULL DEFAULT '',
    timezone TEXT NOT NULL DEFAULT 'UTC',
    enabled BOOLEAN DEFAULT true,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    last_run TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS notifications (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL,
    task_name TEXT NOT NULL,
    summary TEXT NOT NULL,
    full_result TEXT,
    read BOOLEAN DEFAULT false,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS credentials (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_name TEXT UNIQUE NOT NULL,
    base_url TEXT NOT NULL,
    auth_type TEXT NOT NULL,
    auth_value TEXT NOT NULL,
    auth_header TEXT DEFAULT 'Authorization',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
