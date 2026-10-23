-- Add cc_repo_id to track which external repository a CC thread is bound to.
-- Without this, follow-up messages in external repo threads revert to the
-- CognOS workspace because the repo_id isn't persisted anywhere.
ALTER TABLE thread_summaries ADD COLUMN cc_repo_id TEXT;
