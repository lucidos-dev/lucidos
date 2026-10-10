-- Backfill OAuth endpoint URLs into existing oauth_client credential blobs.
--
-- Until this change the engine resolved google/microsoft/github/dropbox/spotify
-- endpoints from a hardcoded `known_provider()` table at runtime, so credentials
-- for those providers were stored as just {client_id, client_secret}. The
-- registry now lives in `system-knowhow/oauth-providers.md` (read & maintained by
-- the agent) and the per-credential JSON is the single source of truth for
-- endpoints. Without this backfill, every already-connected account would fail
-- token refresh / re-auth with "Missing token_url in oauth:<provider>".
--
-- This is a one-time data migration, NOT runtime logic — the provider-specific
-- URLs intentionally live here (historical artifact) and nowhere in engine code.
--
-- `<defaults> || auth_value::jsonb` puts the existing row on the right so any
-- value already present wins: a user who customized an endpoint keeps it, and
-- only absent keys are filled. The `NOT (... ? 'token_url')` guard skips rows
-- that already carry the URLs so completed credentials are left untouched.

UPDATE credentials SET auth_value = (
  jsonb_build_object(
    'auth_url', 'https://accounts.google.com/o/oauth2/v2/auth',
    'token_url', 'https://oauth2.googleapis.com/token',
    'userinfo_url', 'https://www.googleapis.com/oauth2/v2/userinfo'
  ) || auth_value::jsonb
)::text
WHERE service_name = 'oauth:google' AND auth_type = 'oauth_client'
  AND NOT (auth_value::jsonb ? 'token_url');

UPDATE credentials SET auth_value = (
  jsonb_build_object(
    'auth_url', 'https://login.microsoftonline.com/common/oauth2/v2.0/authorize',
    'token_url', 'https://login.microsoftonline.com/common/oauth2/v2.0/token',
    'userinfo_url', 'https://graph.microsoft.com/v1.0/me'
  ) || auth_value::jsonb
)::text
WHERE service_name = 'oauth:microsoft' AND auth_type = 'oauth_client'
  AND NOT (auth_value::jsonb ? 'token_url');

UPDATE credentials SET auth_value = (
  jsonb_build_object(
    'auth_url', 'https://github.com/login/oauth/authorize',
    'token_url', 'https://github.com/login/oauth/access_token',
    'userinfo_url', 'https://api.github.com/user'
  ) || auth_value::jsonb
)::text
WHERE service_name = 'oauth:github' AND auth_type = 'oauth_client'
  AND NOT (auth_value::jsonb ? 'token_url');

-- Dropbox has no userinfo endpoint (was None in the old registry).
UPDATE credentials SET auth_value = (
  jsonb_build_object(
    'auth_url', 'https://www.dropbox.com/oauth2/authorize',
    'token_url', 'https://api.dropboxapi.com/oauth2/token'
  ) || auth_value::jsonb
)::text
WHERE service_name = 'oauth:dropbox' AND auth_type = 'oauth_client'
  AND NOT (auth_value::jsonb ? 'token_url');

UPDATE credentials SET auth_value = (
  jsonb_build_object(
    'auth_url', 'https://accounts.spotify.com/authorize',
    'token_url', 'https://accounts.spotify.com/api/token',
    'userinfo_url', 'https://api.spotify.com/v1/me'
  ) || auth_value::jsonb
)::text
WHERE service_name = 'oauth:spotify' AND auth_type = 'oauth_client'
  AND NOT (auth_value::jsonb ? 'token_url');
