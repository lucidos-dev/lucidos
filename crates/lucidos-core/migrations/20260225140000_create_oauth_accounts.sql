-- Add oauth_accounts table for generic OAuth 2.0 integration.
-- Stores access/refresh tokens for connected OAuth providers.

CREATE TABLE IF NOT EXISTS oauth_accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider TEXT NOT NULL,
    email TEXT,
    display_name TEXT,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    token_expiry TIMESTAMPTZ,
    scopes TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(provider, email)
);

-- Partial unique index: only one account per provider when email is NULL
CREATE UNIQUE INDEX IF NOT EXISTS oauth_accounts_provider_no_email
    ON oauth_accounts (provider) WHERE email IS NULL;
