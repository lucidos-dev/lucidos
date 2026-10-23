-- Give an already-connected Dropbox client the parameter that makes its tokens
-- renewable.
--
-- `authorize_params` (see `core::oauth::AuthorizeParams`) is what carries a
-- provider's own spelling of "issue a refresh token". Dropbox reads
-- `token_access_type=offline`; without it the token endpoint returns a
-- four-hour access token and NO refresh token, so `refresh_oauth_if_needed` can
-- only report "OAuth token expired but no refresh token available" and a
-- scheduled backup fails from the next morning onward.
--
-- Every credential stored before the field existed omits it, which means the
-- default applies, and the default is Google's spelling. That is correct for a
-- credential that has always used it, and useless for Dropbox. The gap matters
-- most on the path built to rescue exactly these users: pressing *Grant access*
-- on the Backup page re-authorizes through the STORED credential, so without
-- this backfill the reconnect fixes the scopes and still yields an
-- unrefreshable token.
--
-- Keyed on the stored `auth_url` rather than the credential's name, because the
-- name is a user-chosen alias (a dedicated connection can be `dropbox2`) while
-- the endpoint is what makes it Dropbox. Provider knowledge belongs in
-- `system-knowhow/oauth-providers.md`, not in engine code, and it is there;
-- this is a dated one-off repair of rows already on disk, the same shape as
-- `20260805085054_normalize_oauth_client_credential_names.sql`.
--
-- Safety:
--
--   * A row that already carries `authorize_params` is left alone. Whatever is
--     there was chosen deliberately, including the `none` opt-out.
--   * Only `auth_type = 'oauth_client'` rows are read as JSON, and only after
--     `IS JSON OBJECT` has passed. The guard sits in its own MATERIALIZED CTE so
--     the cast below cannot be evaluated against a row that failed it: a cast
--     error here would abort the migration and take engine startup with it.
--   * No `CredentialUpdated` event: a migration runs before the EventBus exists,
--     and this adds a missing endpoint detail rather than changing a secret.

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
SET auth_value = jsonb_set(
        p.blob,
        '{authorize_params}',
        '"token_access_type=offline"'::jsonb,
        true
    )::text,
    updated_at = NOW()
FROM parsed AS p
WHERE c.id = p.id
  AND NOT (p.blob ? 'authorize_params')
  AND (p.blob ->> 'auth_url') LIKE '%dropbox.com%';
