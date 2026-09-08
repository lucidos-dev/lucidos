-- Seed OpenAI's GPT-6 Astra into the chat model registry. Served over the
-- direct `openai` provider, through the Responses API. Builtin = disable-only
-- (never deletable), enabled by default like the other builtins.
--
-- sort_order 36 places Astra at the top of the OpenAI cluster, just above
-- GPT-5.6 Sol at 37, so the newest frontier model surfaces first. 33 to 36 were
-- free (Gemini ends at 32), so nothing is renumbered.
--
-- The window is DECLARED, because the id-shape fallback guesses 400k for every
-- `gpt-` id of major 5 or newer and understates this one. The figure comes from
-- launch coverage and is unconfirmed against a billed call.
--
-- WHY DO UPDATE RATHER THAN THE USUAL DO NOTHING
--
-- At least one workspace already has this id in `models` as `source = 'user'`,
-- added by hand through `manage_models` before the model was a builtin. A user
-- row is fully deletable, so DO NOTHING would leave that workspace outside the
-- disable-only protection every other builtin has, and on whatever label,
-- provider and window the row was typed with. DO UPDATE promotes it instead.
--
-- `enabled` is deliberately absent from the SET list. Whether a model is on is
-- the user's decision, and a promotion must not switch a row back on that they
-- switched off. A fresh insert gets the column's own DEFAULT TRUE.
INSERT INTO models (id, label, provider, sort_order, source, context_window) VALUES
  ('gpt-6-astra', 'GPT-6 Astra', 'openai', 36, 'builtin', 1050000)
ON CONFLICT (id) DO UPDATE SET
  label          = EXCLUDED.label,
  provider       = EXCLUDED.provider,
  sort_order     = EXCLUDED.sort_order,
  source         = EXCLUDED.source,
  context_window = EXCLUDED.context_window,
  updated_at     = NOW();
