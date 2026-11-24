-- Drop the saved_contexts table. The feature was never wired up to any
-- consumer (frontend, SDK, LLM tool, docs) so the four endpoints and the
-- table only ever stored test rows in dev workspaces.
DROP TABLE IF EXISTS saved_contexts;
