-- Remove the `scopes` key from every stored OAuth client credential.
--
-- The credential form once had a "Default Scopes" field that saved this key.
-- No flow ever read it: `prepare_oauth_flow` takes scopes only from the caller
-- that starts a connection. A value typed there was silently ignored, so the
-- field is gone and this drops what it left behind.
--
-- Same shape as `20260805104126_backfill_dropbox_offline_authorize_params.sql`.
--
-- Safety:
--
--   * Only `auth_type = 'oauth_client'` rows are read as JSON, and only after
--     `IS JSON OBJECT` has passed. The guard sits in its own MATERIALIZED CTE so
--     the cast below never runs on a row that failed it. A cast error would
--     abort the migration and engine startup with it.
--   * Only rows that carry the key are written. Every other key is kept.
--   * No `CredentialUpdated` event: a migration runs before the EventBus exists,
--     and removing a key nothing reads changes no behavior.

WITH oauth_rows AS MATERIALIZED (
    SELECT id, auth_value
    FROM credentials
    WHERE auth_type = 'oauth_client'
      AND auth_value IS JSON OBJECT
),
parsed AS MATERIALIZED (
    SELECT id, auth_value::jsonb AS blob
    FROM oauth_rows
)
UPDATE credentials AS c
SET auth_value = (p.blob - 'scopes')::text,
    updated_at = NOW()
FROM parsed AS p
WHERE c.id = p.id
  AND p.blob ? 'scopes';
