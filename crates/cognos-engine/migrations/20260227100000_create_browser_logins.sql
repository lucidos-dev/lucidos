CREATE TABLE IF NOT EXISTS browser_logins (
    domain TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    logged_in_at TIMESTAMPTZ DEFAULT NOW()
);
