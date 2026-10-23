-- Rescue OAuth client registrations stranded outside the `oauth:` namespace.
--
-- An `oauth_client` credential is readable under exactly one service name,
-- `oauth:<provider>`: `prepare_oauth_flow`, `refresh_oauth_if_needed`,
-- `connect_oauth_account` and the re-auth API all resolve that form (now via
-- `oauth::client_service_name`). A row stored under any other name is therefore
-- unreachable by every code path in the engine. It looks configured in
-- Settings > Accounts and does nothing.
--
-- They existed because the name came from the caller. An agent passing a bare
-- provider name to `request_credential` produced one; the following
-- `connect_oauth_account` found no `oauth:<provider>` and opened a second modal,
-- and the user ended up holding two credentials for one provider with no way to
-- tell them apart (2026-08-05). Both write entry points now canonicalize the
-- name, so no new ones can appear. This repairs the ones already on disk.
--
-- Safety, in order of what it refuses to do:
--
--   * Only `auth_type = 'oauth_client'` rows are touched. Every other type keeps
--     its name verbatim, because that name is what `CRED_<NAME>` env injection
--     and `data/config/apis.json` service lookups resolve. Renaming one of those
--     would break live scripts.
--   * A row is renamed ONLY when the target name is free. Where both
--     `<provider>` and `oauth:<provider>` exist, the `oauth:` one is the live
--     connection and the bare one is the dead duplicate; clobbering the live row
--     would break a working account, so a collision is left alone for the user
--     to delete. That is the exact shape the incident above produced.
--   * At most ONE row may move to a given canonical name. `service_name` is
--     UNIQUE, so two case variants (`Dropbox` and `dropbox`) both resolving to
--     `oauth:dropbox` would abort the migration on a 23505 and take engine
--     startup down with it. The `winners` CTE picks the oldest deterministically
--     and leaves the rest for the user, which is the same answer the
--     already-taken case gives.
--   * The canonical form is computed EXACTLY as `client_service_name` computes
--     it: trim, lowercase, then test the prefix. Testing the prefix first is
--     what the Rust helper's own test caught, and SQL `LIKE` is case-sensitive,
--     so a prefix-first test here leaves `oauth:Dropbox` unreachable (every read
--     path lowercases) and turns `OAuth:Dropbox` into `oauth:oauth:dropbox`.
--
-- Env var note: this changes the row's default injected variable from
-- `CRED_<NAME>` to `CRED_OAUTH_<NAME>`. Safe, because the value is a JSON client
-- blob (`{client_id, client_secret?, auth_url, ...}`) that nothing but the OAuth
-- flow consumes, and that flow reads the credentials table by service name
-- rather than the environment. A row carrying an explicit custom `env_var_name`
-- keeps it either way.
--
-- Deliberately emits no `CredentialUpdated`: a migration runs before the
-- EventBus exists, and this is a repair of an unreachable row rather than a
-- user-visible change to a working one.

WITH canonical AS (
    -- Mirrors `client_service_name`: trim + lowercase FIRST, then the prefix
    -- test, so a mixed-case prefix is recognized instead of re-prefixed.
    SELECT
        id,
        service_name,
        created_at,
        CASE
            WHEN lower(btrim(service_name)) LIKE 'oauth:%'
                THEN lower(btrim(service_name))
            ELSE 'oauth:' || lower(btrim(service_name))
        END AS target
    FROM credentials
    WHERE auth_type = 'oauth_client'
),
movers AS (
    SELECT c.id, c.created_at, c.target
    FROM canonical AS c
    -- Already canonical: nothing to do.
    WHERE c.service_name <> c.target
      -- Canonical name already occupied (by any row, of any auth type): that
      -- row is the live one. Leave the stray for the user to delete.
      AND NOT EXISTS (
          SELECT 1 FROM credentials AS taken
          WHERE taken.service_name = c.target
      )
),
winners AS (
    -- One row per target. Oldest wins, id breaks a same-timestamp tie, so the
    -- choice is deterministic and the UNIQUE constraint cannot be violated.
    SELECT DISTINCT ON (target) id, target
    FROM movers
    ORDER BY target, created_at, id
)
UPDATE credentials AS c
SET service_name = w.target,
    updated_at = NOW()
FROM winners AS w
WHERE c.id = w.id;
