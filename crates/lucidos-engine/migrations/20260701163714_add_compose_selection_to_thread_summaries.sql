-- Per-draft compose dropdown selections (target/scope, coding agent, Lucidos
-- model + reasoning, coding-agent model + reasoning) stored alongside the
-- draft's text/images/mode, so a stored draft rehydrates its picks on reload
-- and syncs across devices (see docs/plans/2026-07-01-compose-selection-db-persistence.md).
--
-- Shape mirrors the frontend `ComposeSelectionOverride`: a partial override,
-- e.g. { "scope": {"kind":"app","appId":"..."}, "codingAgent":"codex",
-- "model":"...", "reasoningEffort":"...", "ccModel":null, "ccReasoningEffort":null }.
-- NULL column = no per-draft selection; the client resolves each unset field to
-- its account default. The compose PUT COALESCEs a NULL bind to preserve, like
-- compose_images.
ALTER TABLE thread_summaries
  ADD COLUMN IF NOT EXISTS compose_selection JSONB;
