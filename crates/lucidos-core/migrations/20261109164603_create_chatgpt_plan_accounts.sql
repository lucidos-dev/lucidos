-- The workspace's Sign in with ChatGPT registration and sign-in, at most one
-- row (ADR 0636).
--
-- Kept apart from `oauth_accounts` on purpose, as `claude_gateway_accounts` is:
-- every reader of that table hands tokens out. A plan token reaches only
-- OpenAI's API and auth hosts.
--
-- The registration outlives a sign-out:
--   host_id    the `ext_agent_host_id`, a `urn:uuid:` minted before the first
--              sign-in. It names no user.
--   client_id  the client id OpenAI issued at the first sign-in. Never the
--              `dynamic_agent_client` entrypoint.
--   issuer, subject
--              the account's `iss` and `sub`, so a later sign-in cannot
--              swap in another account.
--   id_token   the last ID token, sent back as `id_token_hint`.
--
-- The sign-in is `access_token`, `refresh_token` and `signed_in_at`, all set
-- or all NULL. `refresh_due_at` is OpenAI's `earliest_refresh_at`.
--
-- `id_token`, `access_token` and `refresh_token` join
-- core::secret_store::SECRET_COLUMNS (ADR 0509).

CREATE TABLE IF NOT EXISTS chatgpt_plan_accounts (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    host_id TEXT NOT NULL CHECK (host_id LIKE 'urn:uuid:%'),
    client_id TEXT CHECK (client_id <> 'dynamic_agent_client'),
    issuer TEXT,
    subject TEXT,
    email TEXT,
    id_token TEXT,
    access_token TEXT,
    refresh_token TEXT,
    token_expiry TIMESTAMPTZ,
    refresh_due_at TIMESTAMPTZ,
    signed_in_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT chatgpt_plan_accounts_sign_in_whole CHECK (
        (access_token IS NULL) = (refresh_token IS NULL)
        AND (access_token IS NULL) = (signed_in_at IS NULL)
        AND (access_token IS NULL OR client_id IS NOT NULL)
    ),
    CONSTRAINT chatgpt_plan_accounts_id_token_sealed CHECK (id_token LIKE 'lsec1:%'),
    CONSTRAINT chatgpt_plan_accounts_access_token_sealed CHECK (access_token LIKE 'lsec1:%'),
    CONSTRAINT chatgpt_plan_accounts_refresh_token_sealed CHECK (refresh_token LIKE 'lsec1:%')
);
