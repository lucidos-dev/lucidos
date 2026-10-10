-- Add the repository's git root-commit SHA — the natural key behind the
-- deterministic `repositories.id` (uuidv5(namespace, root_commit_sha)). Nullable
-- and NOT backfilled here on purpose: computing it needs a git shell-out, which
-- SQL can't do. At startup `RepositoryStore::ensure_exists`/`add` fill this column
-- and rewrite the default repo row from its legacy random id to the deterministic
-- one; the separate marker-guarded `EventStore::backfill_cc_repo_id_to_deterministic`
-- step re-points the orphaned `thread_summaries.cc_repo_id` bindings.
ALTER TABLE repositories ADD COLUMN IF NOT EXISTS root_commit_sha TEXT;
