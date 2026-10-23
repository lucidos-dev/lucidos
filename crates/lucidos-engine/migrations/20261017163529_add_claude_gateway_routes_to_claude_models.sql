-- Route every builtin Claude model the direct Anthropic API serves to the
-- Claude gateway as well.
--
-- A Claude gateway is the Anthropic Messages API at an organisation's own
-- origin, so its route copies the row's `anthropic` route: the same wire id and
-- the same window. It goes right after that route. First party stays first, so
-- a workspace with Vertex or an Anthropic key keeps the backend it has, and a
-- gateway still comes before OpenRouter.
--
-- The route only says the gateway may serve the model. The gateway's own model
-- listing decides, per account, before any request is sent.
-- See docs/plans/2026-10-17-claude-gateway-provider.md.
--
-- User rows are left alone. A re-run is a no-op, since a row that already
-- routes to the gateway is skipped.
UPDATE models m
SET routes = (
  SELECT jsonb_agg(r.elem ORDER BY r.ord, r.inserted)
  FROM (
    SELECT t.elem, t.ord, 0 AS inserted
      FROM jsonb_array_elements(m.routes) WITH ORDINALITY AS t(elem, ord)
    UNION ALL
    SELECT t.elem || '{"provider": "claude-gateway"}'::jsonb, t.ord, 1
      FROM jsonb_array_elements(m.routes) WITH ORDINALITY AS t(elem, ord)
     WHERE t.elem ->> 'provider' = 'anthropic'
  ) AS r
)
WHERE m.source = 'builtin'
  AND m.routes @> '[{"provider": "anthropic"}]'::jsonb
  AND NOT m.routes @> '[{"provider": "claude-gateway"}]'::jsonb;
