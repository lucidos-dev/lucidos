-- The workspace's sign-in to its Claude gateway, at most one row.
--
-- Kept apart from `oauth_accounts` on purpose. Every reader of that table hands
-- tokens out: script env vars, the `http_request` tool, the app proxy and the
-- access-token endpoint. A gateway token must reach only the gateway.
--
-- `issuer` is the gateway origin that issued the tokens. The engine presents
-- them to that origin only.

CREATE TABLE IF NOT EXISTS claude_gateway_accounts (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    issuer TEXT NOT NULL,
    email TEXT,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    token_expiry TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
