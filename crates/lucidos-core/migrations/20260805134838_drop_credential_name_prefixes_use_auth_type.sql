-- Make `auth_type` the credential discriminator and drop the two name prefixes.
--
-- `service_name` carried a namespace prefix for exactly two auth types:
-- `oauth:<provider>` for `oauth_client` and `email:<account>` for
-- `email_password`. Both facts were already recorded, typed, one column over.
-- The prefix was the same fact spelled a second time into an untyped field, and
-- it drifted: on 2026-08-05 an agent wrote a correctly-typed `oauth_client` row
-- named `dropbox`, unreachable because every read path resolved the string
-- rather than the type, and the user ended up holding two credentials for one
-- provider. `20260805085054_normalize_oauth_client_credential_names.sql` (which
-- this supersedes) repaired those rows; this removes the reason they existed.
--
-- The prefix was load-bearing for ONE reason: `service_name TEXT UNIQUE` was the
-- table's only uniqueness constraint, so without it a `google` API key and the
-- Google app registration were the same row. That is replaced below by two
-- constraints rather than one, and the asymmetry is the point.
--
--   * `UNIQUE (service_name, auth_type)` is the new identity.
--   * `UNIQUE (service_name) WHERE auth_type <> 'oauth_client'` keeps a name
--     GLOBALLY unique for every type that is injected as `CRED_<NAME>` or
--     resolved by bare name, and lets `oauth_client` alone shadow a name.
--
-- The second one is what makes this safe. Five existing reads take a bare name
-- and expect one row: the four LLM provider keys in `llm/provider_build.rs`
-- (`anthropic`, `openai`, `openrouter`, `local`), and the `apis.json` resolver
-- `api/proxy.rs::fetch_required_credential`. Under a plain composite key an
-- `oauth_client` row named `openai` could be handed back instead of the provider
-- API key, breaking chat auth, and the proxy would send a `{client_id, ...}`
-- blob as an `Authorization` header. With the partial index plus a matching
-- `AND auth_type <> 'oauth_client'` in `CredentialStore::get`, all five stay
-- correct without changing.
--
-- Order is load-bearing: the old constraint is dropped FIRST, because stripping
-- `oauth:google` to `google` while a `google` API key exists is exactly the
-- shadowing the new model permits and the old one forbade.

-- Step 1. Drop the constraint that forced the prefixes to exist.
ALTER TABLE credentials DROP CONSTRAINT IF EXISTS credentials_service_name_key;

-- Step 2a-i. Move a DEAD same-type duplicate out of the live row's way.
--
-- The shape the 2026-08-05 incident produced, and which
-- `20260805085054_normalize_oauth_client_credential_names.sql` deliberately left
-- in place for the user to resolve: BOTH `oauth:dropbox` and a bare `dropbox`
-- exist, both `oauth_client`.
--
-- Which is which is not a guess. Before this migration every OAuth read path
-- resolves `oauth:<provider>`, so the prefixed row IS the live registration and
-- the bare one is unreachable by every code path in the engine. Leaving the pair
-- alone would therefore INVERT them: 2a below could not move the prefixed row
-- (its target is occupied), while `get_oauth_client` now resolves the bare name,
-- so the engine would start reading the dead duplicate and the working
-- connection would break on the next refresh. Stranding is only an acceptable
-- outcome when it preserves the status quo, and here it does the opposite.
--
-- So the dead row is renamed aside rather than deleted (a migration must not
-- destroy a secret the user typed), which frees the bare name for the live row
-- and leaves the duplicate visible in Settings under a name that says what it
-- is. Matched by EXACT name against the live row's target, the same comparison
-- 2a's occupancy check uses.
WITH targets AS (
    SELECT
        CASE
            WHEN auth_type = 'oauth_client'
                THEN substring(lower(btrim(service_name)) FROM 7)
            ELSE substring(service_name FROM 7)
        END AS target,
        auth_type
    FROM credentials
    WHERE (auth_type = 'oauth_client' AND lower(btrim(service_name)) LIKE 'oauth:%')
       OR (auth_type = 'email_password' AND service_name LIKE 'email:%')
),
dead AS (
    SELECT DISTINCT c.id, c.service_name
    FROM credentials AS c
    JOIN targets AS t
      -- Same type AND the exact name the live row is about to take. A bare
      -- `dropbox` API key is NOT matched: a different type is a different
      -- credential, and shadowing it is the point of the new constraints.
      ON t.auth_type = c.auth_type
     AND t.target = c.service_name
    WHERE t.target <> ''
)
-- The rename ALWAYS happens. Skipping it when the archival name is occupied
-- would put the live row straight back where it started: 2a could not move it,
-- and the engine would read the dead duplicate. So an occupied archival name
-- falls back to one suffixed with the row's own primary key, which is unique by
-- construction and therefore cannot collide with another dead row. (At most one
-- dead row per name exists anyway: `service_name` was UNIQUE before this
-- migration, so the only way the plain form is taken is an unrelated credential
-- the user happened to name that.)
UPDATE credentials AS c
SET service_name = CASE
        WHEN NOT EXISTS (
            SELECT 1 FROM credentials AS taken
            WHERE taken.service_name = d.service_name || ' (unreachable duplicate)'
        )
            THEN d.service_name || ' (unreachable duplicate)'
        ELSE d.service_name || ' (unreachable duplicate ' || c.id::text || ')'
    END,
    updated_at = NOW()
FROM dead AS d
WHERE c.id = d.id;

-- Step 2a. Strip `oauth:` from `oauth_client` rows.
--
-- Case and whitespace are folded exactly as the deleted `client_service_name`
-- folded them (trim + lowercase, THEN the prefix test), so a legacy
-- `OAuth:Google` is recognized instead of being left behind. A row moves only
-- when no OTHER `oauth_client` row already holds the target name, and at most
-- one row may move to a given target: the composite constraint is added below,
-- and a double move would abort the migration and block engine startup.
--
-- After 2a-i the only thing that can still occupy a target is a SECOND prefixed
-- row (two case variants), which is a genuine ambiguity rather than a dead row.
WITH targets AS (
    SELECT
        id,
        created_at,
        substring(lower(btrim(service_name)) FROM 7) AS target
    FROM credentials
    WHERE auth_type = 'oauth_client'
      AND lower(btrim(service_name)) LIKE 'oauth:%'
),
movable AS (
    SELECT t.id, t.created_at, t.target
    FROM targets AS t
    WHERE t.target <> ''
      -- Only SAME-TYPE occupancy blocks the move. A bare `google` API key is
      -- allowed to coexist from here on, which is the whole point.
      AND NOT EXISTS (
          SELECT 1 FROM credentials AS taken
          WHERE taken.auth_type = 'oauth_client'
            AND taken.service_name = t.target
      )
),
winners AS (
    SELECT DISTINCT ON (target) id, target
    FROM movable
    ORDER BY target, created_at, id
)
UPDATE credentials AS c
SET service_name = w.target,
    updated_at = NOW()
FROM winners AS w
WHERE c.id = w.id;

-- Step 2b. Strip `email:` from `email_password` rows.
--
-- Case-sensitive, matching the `strip_prefix("email:")` this replaces: the
-- remainder is an `email_accounts.name` and must survive byte for byte or the
-- credential detaches from its mailbox row. Occupancy is checked against every
-- NON-oauth row, because `email_password` lives under the globally-unique arm
-- of the new constraints, not the shadowing one.
WITH targets AS (
    SELECT
        id,
        created_at,
        substring(service_name FROM 7) AS target
    FROM credentials
    WHERE auth_type = 'email_password'
      AND service_name LIKE 'email:%'
),
movable AS (
    SELECT t.id, t.created_at, t.target
    FROM targets AS t
    WHERE t.target <> ''
      AND NOT EXISTS (
          SELECT 1 FROM credentials AS taken
          WHERE taken.auth_type <> 'oauth_client'
            AND taken.service_name = t.target
      )
),
winners AS (
    SELECT DISTINCT ON (target) id, target
    FROM movable
    ORDER BY target, created_at, id
)
UPDATE credentials AS c
SET service_name = w.target,
    updated_at = NOW()
FROM winners AS w
WHERE c.id = w.id;

-- Step 3. Name the rows that kept a prefix, so a skip is visible in the startup
-- log instead of reading as a clean run. `CredentialStore::get_email_password`
-- still resolves a stranded `email:` row (a registered temporary measure); a
-- stranded `oauth:` row needs the user to delete whichever duplicate is dead.
DO $$
DECLARE
    stranded TEXT;
BEGIN
    SELECT string_agg(service_name, ', ' ORDER BY service_name)
    INTO stranded
    FROM credentials
    WHERE (auth_type = 'oauth_client' AND lower(btrim(service_name)) LIKE 'oauth:%')
       OR (auth_type = 'email_password' AND service_name LIKE 'email:%');

    IF stranded IS NOT NULL THEN
        RAISE NOTICE '[Credentials] kept a name prefix because the unprefixed name was already taken by a credential of another type: %. Delete whichever is redundant, then rename by hand.', stranded;
    END IF;

    SELECT string_agg(service_name, ', ' ORDER BY service_name)
    INTO stranded
    FROM credentials
    WHERE service_name LIKE '%(unreachable duplicate)';

    IF stranded IS NOT NULL THEN
        RAISE NOTICE '[Credentials] renamed aside as unreachable duplicates of a live credential: %. Safe to delete once you have confirmed the live one works.', stranded;
    END IF;
END $$;

-- Step 4. The replacement constraints.
ALTER TABLE credentials
    ADD CONSTRAINT credentials_service_name_auth_type_key UNIQUE (service_name, auth_type);

CREATE UNIQUE INDEX credentials_service_name_not_oauth_key
    ON credentials (service_name)
    WHERE auth_type <> 'oauth_client';
