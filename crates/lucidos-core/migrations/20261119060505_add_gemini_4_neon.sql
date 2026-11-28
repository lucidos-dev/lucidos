-- Add the global Vertex model while preserving any user-created row.
INSERT INTO models (id, label, routes, vision, default_effort, sort_order, source) VALUES
 ('gemini-4-neon', 'Gemini 4 Neon',
  '[{"provider":"vertex","context_window":1048576}]'::jsonb,
  true, 'high', 26, 'builtin')
ON CONFLICT (id) DO NOTHING;
