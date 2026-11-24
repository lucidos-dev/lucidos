-- Route every builtin GPT model the OpenAI API serves to the ChatGPT plan as
-- well (ADR 0636).
--
-- The plan route copies the row's `openai` route: the same wire id and the
-- same window. It goes right BEFORE that route. Signing in with ChatGPT is an
-- explicit act, so a workspace with both the plan and an API key gets the plan
-- by default. A stored route preference still wins.
--
-- The route only says the plan may serve the model. The plan's own model
-- listing decides, per account, before any request is sent.
-- See docs/plans/2026-11-09-chatgpt-plan-provider.md.
--
-- User rows are left alone. A re-run is a no-op, since a row that already
-- routes to the plan is skipped.
UPDATE models m
SET routes = (
  SELECT jsonb_agg(r.elem ORDER BY r.ord, r.inserted)
  FROM (
    SELECT t.elem, t.ord, 1 AS inserted
      FROM jsonb_array_elements(m.routes) WITH ORDINALITY AS t(elem, ord)
    UNION ALL
    SELECT t.elem || '{"provider": "chatgpt-plan"}'::jsonb, t.ord, 0
      FROM jsonb_array_elements(m.routes) WITH ORDINALITY AS t(elem, ord)
     WHERE t.elem ->> 'provider' = 'openai'
  ) AS r
)
WHERE m.source = 'builtin'
  AND m.id LIKE 'gpt-%'
  AND m.routes @> '[{"provider": "openai"}]'::jsonb
  AND NOT m.routes @> '[{"provider": "chatgpt-plan"}]'::jsonb;
