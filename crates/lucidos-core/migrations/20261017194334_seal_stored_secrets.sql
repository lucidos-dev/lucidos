-- Stored secrets are sealed at rest (ADR 0509). core::secret_store::SECRET_COLUMNS
-- lists every secret column, and each gets a CHECK named <table>_<column>_sealed.
--
-- The CHECKs are NOT VALID: they bind every new write now, while the rows
-- already here stay plain until the engine's boot seals them with the
-- workspace key and runs VALIDATE CONSTRAINT. SQL cannot seal: the key never
-- enters the database.

-- The VAPID keypair leaves the generic preferences table, so no generic table
-- holds a secret, and the agent role's grants can leave its key column out.
CREATE TABLE IF NOT EXISTS vapid_keys (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    public_key TEXT NOT NULL,
    private_key_pem TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO vapid_keys (public_key, private_key_pem)
SELECT value::jsonb ->> 'public_key', value::jsonb ->> 'private_key_pem'
FROM preferences
WHERE key = 'vapid_keys'
  AND value::jsonb ->> 'public_key' IS NOT NULL
  AND value::jsonb ->> 'private_key_pem' IS NOT NULL
ON CONFLICT (singleton) DO NOTHING;

DELETE FROM preferences WHERE key = 'vapid_keys';

ALTER TABLE credentials ADD CONSTRAINT credentials_auth_value_sealed
    CHECK (auth_value LIKE 'lsec1:%') NOT VALID;
ALTER TABLE oauth_accounts ADD CONSTRAINT oauth_accounts_access_token_sealed
    CHECK (access_token LIKE 'lsec1:%') NOT VALID;
ALTER TABLE oauth_accounts ADD CONSTRAINT oauth_accounts_refresh_token_sealed
    CHECK (refresh_token LIKE 'lsec1:%') NOT VALID;
-- An empty password means the mailbox has none.
ALTER TABLE email_accounts ADD CONSTRAINT email_accounts_password_sealed
    CHECK (password = '' OR password LIKE 'lsec1:%') NOT VALID;
ALTER TABLE push_subscriptions ADD CONSTRAINT push_subscriptions_auth_sealed
    CHECK (auth LIKE 'lsec1:%') NOT VALID;
ALTER TABLE vapid_keys ADD CONSTRAINT vapid_keys_private_key_pem_sealed
    CHECK (private_key_pem LIKE 'lsec1:%') NOT VALID;
-- Every value of the env object is a sealed string; the names stay readable.
ALTER TABLE mcp_servers ADD CONSTRAINT mcp_servers_env_sealed
    CHECK (NOT jsonb_path_exists(env, '$.* ? (@.type() != "string" || !(@ starts with "lsec1:"))'))
    NOT VALID;
