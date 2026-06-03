-- Consolidates the CREATE TABLE / CREATE INDEX statements that used to
-- live inline in *Store::init_schema methods scattered across core/ and
-- scheduler/. Per .claude/rules/rust.md, schema changes go through
-- migrations — having init_schema run them on every engine boot was
-- inconsistent with that rule and made the actual schema invisible to
-- anyone reading the migration log.
--
-- Every statement here mirrors the corresponding init_schema body
-- verbatim, IF NOT EXISTS — so fresh installs land the schema via this
-- migration, existing installs already have the tables (created by older
-- init_schema runs) and the migration is a no-op for them.
--
-- The init_schema bodies stay in place for one release cycle as a
-- defensive double-write; they're slated for removal in
-- harden-init-schema-tables-vs-migrations-pattern-finish (the follow-up
-- work-tracker entry that owns the next phase).

-- core::store::EventStore — the event store table + indexes
CREATE TABLE IF NOT EXISTS events (
    id UUID PRIMARY KEY,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_events_type_created
ON events (event_type, created ASC);

CREATE INDEX IF NOT EXISTS idx_events_request_id
ON events ((payload->>'request_id'))
WHERE payload->>'request_id' IS NOT NULL;

-- Legacy payload thread_id index — replaced by thread_id column + idx_events_thread_seq.
DROP INDEX IF EXISTS idx_events_thread_id;

CREATE INDEX IF NOT EXISTS idx_events_created
ON events (created ASC);

-- core::credentials::CredentialStore
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

-- core::devices::DeviceStore
CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    name TEXT,
    user_agent TEXT,
    push_enabled BOOLEAN NOT NULL DEFAULT false,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- core::pinned_apps::PinnedAppStore
CREATE TABLE IF NOT EXISTS pinned_apps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id TEXT NOT NULL,
    ui_id TEXT NOT NULL DEFAULT 'main',
    device_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (app_id, ui_id, device_id)
);

-- core::preferences::PreferenceStore
CREATE TABLE IF NOT EXISTS preferences (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- scheduler::notifications::NotificationStore
CREATE TABLE IF NOT EXISTS notifications (
    id UUID PRIMARY KEY,
    task_id UUID,
    app_id TEXT,
    thread_id UUID,
    event_id UUID,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    read BOOLEAN DEFAULT false,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notifications_unread
ON notifications (read, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_notifications_created_at
ON notifications (created_at DESC);

-- scheduler::push::PushSubscriptionStore
CREATE TABLE IF NOT EXISTS push_subscriptions (
    endpoint TEXT PRIMARY KEY,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- runtime::browser::HeadlessBlocklist
CREATE TABLE IF NOT EXISTS headless_blocked (
    domain TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- runtime::browser::BrowserLogins
CREATE TABLE IF NOT EXISTS browser_logins (
    domain TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    logged_in_at TIMESTAMPTZ DEFAULT NOW()
);
