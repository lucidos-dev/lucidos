-- User-managed non-secret environment variables.
--
-- A parallel mechanism to `credentials`: plaintext key/value pairs the user (or
-- the LLM) defines once, injected as real env vars into every subprocess the
-- engine spawns (run_python, run_bash, background, scheduled scripts, triggers,
-- Claude Code, Codex), alongside the CRED_*/OAUTH_* injection. Deliberately NOT
-- secret — the value is broadcast in SystemEvents and may appear in tool-call
-- payloads / logs / the event store. That is the whole point and why these are
-- separate from `credentials`.
CREATE TABLE IF NOT EXISTS environment_variables (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT UNIQUE NOT NULL,
    value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
