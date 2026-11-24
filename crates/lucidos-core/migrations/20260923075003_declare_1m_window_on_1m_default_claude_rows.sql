-- Declare the 1M window on the bare builtin rows of the Claude families whose
-- default window is 1M: Opus 5, Opus 5.5, Fable 5 and Fable 5.1.
--
-- A declared window describes the request Lucidos actually makes. A bare id
-- sends no `context-1m` beta, and for older families that meant 200k. These
-- four run 1M by default with no beta, so 1000000 is the window of the bare
-- request too. Left undeclared, the prefix map's 200k trims their history about
-- five times too early.
--
-- The window lives on each route (20260922215055). Only a route that sends the
-- row's own id to Vertex or the direct Anthropic API is changed: a route with its
-- own id (OpenRouter's `anthropic/...`) is another backend's window to declare.
-- A route that already declares a window keeps it, and a user-created row under
-- one of these ids is never touched. `claude-opus-5@default` is listed for the
-- builtin the re-spell (20260922222019) leaves when a user owns the bare id.
--
-- Sonnet 5 and the Opus 4.x rows stay undeclared: their 1M window is not
-- documented as the default, and over-declaring makes the provider reject the
-- request.
UPDATE models
SET routes = (
      SELECT jsonb_agg(
               CASE
                 WHEN COALESCE(jsonb_typeof(r -> 'context_window'), 'null') = 'null'
                  AND COALESCE(jsonb_typeof(r -> 'id'), 'null') = 'null'
                  AND r ->> 'provider' IN ('vertex', 'anthropic')
                 THEN r || '{"context_window": 1000000}'::jsonb
                 ELSE r
               END
               ORDER BY ord)
        FROM jsonb_array_elements(routes) WITH ORDINALITY AS t(r, ord)
    ),
    updated_at = NOW()
WHERE source = 'builtin'
  AND id IN (
    'claude-opus-5', 'claude-opus-5@default', 'claude-opus-5-5',
    'claude-fable-5', 'claude-fable-5-1'
  )
  AND EXISTS (
        SELECT 1 FROM jsonb_array_elements(routes) AS r
         WHERE COALESCE(jsonb_typeof(r -> 'context_window'), 'null') = 'null'
           AND COALESCE(jsonb_typeof(r -> 'id'), 'null') = 'null'
           AND r ->> 'provider' IN ('vertex', 'anthropic')
      );
