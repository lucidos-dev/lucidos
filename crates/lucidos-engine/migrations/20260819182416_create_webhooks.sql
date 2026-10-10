-- Inbound webhooks: one row per publicly reachable endpoint.
--
-- The hook socket forwards a delivery here by id, and the engine verifies it
-- before emitting `event_type` as an ordinary domain event.
--
-- Two columns are not what they look like:
--   * `token_hash` is a SHA-256 of the bearer token. The token is shown once,
--     at create, and is unrecoverable afterwards.
--   * `hmac` names a credential by `service_name`. The secret stays in the
--     `credentials` table, which is its only home.
CREATE TABLE IF NOT EXISTS webhooks (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    -- The event this hook emits. Pinned, so a caller can never widen a public
    -- endpoint's reach to another event name.
    event_type TEXT NOT NULL,
    -- NULL when the hook authenticates by signature alone, which is the only
    -- shape a sender like GitHub can produce. At least one verifier is
    -- required, and the CHECK below is the floor under that rule.
    token_hash TEXT,
    hmac JSONB,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT webhooks_name_key UNIQUE (name),
    CONSTRAINT webhooks_needs_a_verifier CHECK (token_hash IS NOT NULL OR hmac IS NOT NULL)
);
