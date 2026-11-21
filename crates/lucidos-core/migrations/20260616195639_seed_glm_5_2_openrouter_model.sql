-- Seed GLM 5.2 (Z.ai / Zhipu) on the OpenRouter provider into the chat model
-- registry. Served over OpenRouter's OpenAI-compatible Chat Completions API
-- (slug `z-ai/glm-5.2`, 1M context). Builtin = disable-only (never deletable),
-- enabled by default like the other builtins; it shows in the picker and
-- surfaces a clear "OpenRouter not configured" error at call time if no
-- OpenRouter credential (Settings → Providers) / LUCIDOS_OPENROUTER_API_KEY is
-- set. ON CONFLICT DO NOTHING so re-running against a populated table is a no-op.
--
-- No `local`-provider model is seeded: local model ids are workspace-specific
-- (the Ollama / LM Studio / vLLM model the user pulled), so users add those as
-- `source = 'user'` rows themselves.
INSERT INTO models (id, label, provider, sort_order, source) VALUES
  ('z-ai/glm-5.2', 'GLM 5.2', 'openrouter', 50, 'builtin')
ON CONFLICT (id) DO NOTHING;
