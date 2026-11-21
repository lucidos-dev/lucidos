-- Add native-window models without changing existing rows or preferences.
-- Limits and routes: provider documentation linked in the implementation plan.
-- Gemini uses non-streaming GenerateContent; output is capped in the wire builder.
INSERT INTO models (id, label, routes, vision, default_effort, sort_order, source) VALUES
 ('claude-fable-5-2', 'Fable 5.2',
  '[{"provider":"anthropic","context_window":1000000},{"provider":"claude-gateway","context_window":1000000}]'::jsonb,
  true, 'high', -3, 'builtin'),
 ('gemini-4-argon', 'Gemini 4 Argon',
  '[{"provider":"vertex","context_window":1048576}]'::jsonb,
  true, 'medium', 28, 'builtin'),
 ('gemini-4-carbon', 'Gemini 4 Carbon',
  '[{"provider":"vertex","context_window":1048576}]'::jsonb,
  true, 'medium', 27, 'builtin')
ON CONFLICT (id) DO NOTHING;
