-- Database-backed registry of chat models for the Lucidos Agent picker.
-- Plain config table (authoritative, like mcp_servers / credentials); CRUD via
-- the HTTP API emits audit Model* SystemEvents but this table is the source of
-- truth. Drives ONLY the chat (Lucidos Agent) model picker + RoutingProvider's
-- provider selection — the Claude Code /model picker stays hand-maintained in
-- runtime/cc_menu_options.json.
--
-- `id` is the value sent in the API request (e.g. 'claude-fable-5',
-- 'claude-opus-4-8@default[1m]'); `[1m]` variants are separate rows, mirroring
-- the previous hardcoded store/models.ts MODELS array.
-- `provider` names the backend that serves the model: 'vertex' | 'anthropic'
-- | 'openai'. `source` is 'builtin' (seeded here, disable-only) or 'user'
-- (added in Settings, fully deletable).

CREATE TABLE IF NOT EXISTS models (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    provider    TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    source      TEXT NOT NULL DEFAULT 'user',
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Seed builtins = the previous store/models.ts MODELS array verbatim, plus the
-- two new Fable 5 rows (served by the direct Anthropic provider). ON CONFLICT
-- DO NOTHING so re-running against a populated table is a no-op.
INSERT INTO models (id, label, provider, sort_order, source) VALUES
  ('claude-fable-5',              'Fable 5',          'anthropic', 0,  'builtin'),
  ('claude-fable-5[1m]',          'Fable 5 (1M)',     'anthropic', 1,  'builtin'),
  ('claude-opus-4-8@default',     'Opus 4.8',         'vertex',    10, 'builtin'),
  ('claude-opus-4-8@default[1m]', 'Opus 4.8 (1M)',    'vertex',    11, 'builtin'),
  ('claude-opus-4-7',             'Opus 4.7',         'vertex',    12, 'builtin'),
  ('claude-opus-4-7[1m]',         'Opus 4.7 (1M)',    'vertex',    13, 'builtin'),
  ('claude-opus-4-6',             'Opus 4.6',         'vertex',    14, 'builtin'),
  ('claude-opus-4-6[1m]',         'Opus 4.6 (1M)',    'vertex',    15, 'builtin'),
  ('claude-sonnet-4-6',           'Sonnet 4.6',       'vertex',    16, 'builtin'),
  ('claude-sonnet-4-6[1m]',       'Sonnet 4.6 (1M)',  'vertex',    17, 'builtin'),
  ('claude-opus-4-5@20251101',    'Opus 4.5',         'vertex',    18, 'builtin'),
  ('gemini-3.1-pro-preview',      'Gemini 3.1 Pro',   'vertex',    30, 'builtin'),
  ('gemini-3.5-flash',            'Gemini 3.5 Flash', 'vertex',    31, 'builtin'),
  ('gemini-3-flash-preview',      'Gemini 3 Flash',   'vertex',    32, 'builtin'),
  ('gpt-5.5-pro',                 'GPT-5.5 Pro',      'openai',    40, 'builtin'),
  ('gpt-5.5',                     'GPT-5.5',          'openai',    41, 'builtin'),
  ('gpt-5.4',                     'GPT-5.4',          'openai',    42, 'builtin'),
  ('gpt-5.3-codex',               'GPT-5.3 Codex',    'openai',    43, 'builtin'),
  ('gpt-5.3-codex-spark',         'Codex Spark',      'openai',    44, 'builtin'),
  ('gpt-5.2-codex',               'GPT-5.2 Codex',    'openai',    45, 'builtin')
ON CONFLICT (id) DO NOTHING;
