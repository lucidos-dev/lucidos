-- One model row, served by whichever provider the workspace has credentials for.
--
-- Until now `models.provider` named THE backend and `models.id` was the literal
-- string sent to it. "The same model on two providers" needed two rows, and two
-- rows meant two entries in the chat picker. So every Claude Opus and Sonnet row
-- sat on `vertex`, and a workspace holding only an Anthropic API key was offered
-- the Fable rows and nothing else from the Claude family.
--
-- `routes` replaces both `provider` and `context_window` with an ORDERED list of
-- routes. Each route is `{provider, id?, context_window?}`:
--   * `provider` is the backend, same vocabulary the column used.
--   * `id` is what goes on the wire, defaulting to the row's own id. A
--     first-party Claude id is byte-identical on Vertex and on the direct
--     Anthropic API, so only a backend that spells it differently sets this.
--     OpenRouter is that case (`anthropic/claude-opus-5-5`).
--   * `context_window` is that backend's window. NULL keeps the id-shape guess,
--     which reads the ROUTE's id, so a route with no `[1m]` lands on 200k by
--     itself.
--
-- `preferred_provider` is the per-model memory of the last provider picked.
-- NULL means never picked. Resolution is the preferred route when its provider
-- is configured, else the first configured route. An explicit choice is
-- honoured or refused, never substituted.
--
-- See docs/plans/2026-09-22-one-model-any-configured-provider.md.

ALTER TABLE models ADD COLUMN IF NOT EXISTS routes JSONB;
ALTER TABLE models ADD COLUMN IF NOT EXISTS preferred_provider TEXT;

-- Fold the two retiring columns into a one-element route per row. `to_jsonb`
-- on the window keeps a NULL as JSON null, which the reader treats as
-- undeclared, exactly as the column did.
UPDATE models
SET routes = jsonb_build_array(
      jsonb_strip_nulls(
        jsonb_build_object('provider', provider, 'context_window', context_window)
      )
    )
WHERE routes IS NULL;

ALTER TABLE models ALTER COLUMN routes SET NOT NULL;

-- A route list that is empty, malformed, or names one provider twice would make
-- the model silently unreachable or its resolution ambiguous. Worse, one route
-- the engine cannot decode fails the WHOLE registry read, so every model would
-- fall back to the id-shape guess. A CHECK cannot hold a subquery, so the test
-- lives in an IMMUTABLE function; it reads only its argument, never a table.
--
-- Each optional field is tested through a CASE, never a bare OR: SQL does not
-- promise to evaluate OR left to right, and casting a string window to numeric
-- would raise instead of answering false.
CREATE OR REPLACE FUNCTION model_routes_valid(routes JSONB) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
  SELECT jsonb_typeof(routes) = 'array'
     AND jsonb_array_length(routes) > 0
     AND NOT EXISTS (
           SELECT 1
             FROM jsonb_array_elements(routes) AS r
            WHERE jsonb_typeof(r) <> 'object'
               OR COALESCE(jsonb_typeof(r -> 'provider'), 'missing') <> 'string'
               OR length(trim(r ->> 'provider')) = 0
               OR CASE COALESCE(jsonb_typeof(r -> 'id'), 'null')
                    WHEN 'null' THEN false
                    WHEN 'string' THEN length(trim(r ->> 'id')) = 0
                    ELSE true
                  END
               OR CASE COALESCE(jsonb_typeof(r -> 'context_window'), 'null')
                    WHEN 'null' THEN false
                    -- Digits only: `1048576.0` is whole, yet the engine's i32
                    -- decoder refuses it.
                    WHEN 'number' THEN CASE
                      WHEN (r ->> 'context_window') ~ '^[1-9][0-9]{0,9}$'
                        THEN (r ->> 'context_window')::bigint > 2147483647
                      ELSE true
                    END
                    ELSE true
                  END
         )
     AND (SELECT count(DISTINCT r ->> 'provider')
            FROM jsonb_array_elements(routes) AS r) = jsonb_array_length(routes);
$$;

ALTER TABLE models
  ADD CONSTRAINT models_routes_are_a_non_empty_unique_provider_list
  CHECK (model_routes_valid(routes));

ALTER TABLE models DROP COLUMN provider;
ALTER TABLE models DROP COLUMN context_window;

-- Seed the direct Anthropic route on the current-generation Claude rows. Their
-- ids are byte-identical on both backends, so neither route spells one.
--
-- Vertex stays FIRST, so a workspace already reaching these through Vertex keeps
-- doing so. A workspace with only an Anthropic key now reaches them at all,
-- which is the whole point.
--
-- Guarded on the provider not already being present, so re-running is a no-op
-- and a user who added the route by hand is not given a duplicate.
--
-- Fable is absent on purpose: it is not published on Vertex, so its rows keep
-- their single `anthropic` route. The retired Opus 4.x and Sonnet 4.6 rows are
-- absent too. Their ids were never probed against the direct API, and seeding an
-- unverified id trades a clean "not configured" refusal for a vendor 404.
--
-- The `@default` Opus 5 rows are absent as well. `@default` is Vertex-only, and
-- the re-spell may leave a row under it when its bare id is taken. The re-spell
-- seeds their Anthropic route once the rename has actually happened.
UPDATE models
SET routes = routes || '[{"provider": "anthropic"}]'::jsonb,
    updated_at = NOW()
WHERE source = 'builtin'
  AND id IN (
    'claude-opus-5-5',     'claude-opus-5-5[1m]',
    'claude-opus-5',       'claude-opus-5[1m]',
    'claude-sonnet-5',     'claude-sonnet-5[1m]'
  )
  AND NOT EXISTS (
        SELECT 1 FROM jsonb_array_elements(routes) AS r
         WHERE r ->> 'provider' = 'anthropic'
      );
