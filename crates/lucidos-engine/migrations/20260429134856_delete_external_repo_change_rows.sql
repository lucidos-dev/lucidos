-- External repo CC threads must never produce Lucidos Changes. The runtime
-- gates (should_propose_change_at_idle, safe_cleanup_worktree, the SessionEnded
-- cleanup path) all skip propose_change for is_external_repo=true, so this
-- DELETE typically affects zero rows. Included as a one-shot cleanup so any
-- rows surviving from before those gates were added are removed.
--
-- Drops every row regardless of status (pending/applied/discarded). The Change
-- abstraction (apply, discard, revert, harden, restart-required) is meaningful
-- only for the Lucidos repo; for external repos the dev pushes/PRs themselves
-- and the thread terminates via the regular Done flow.
DELETE FROM changes
WHERE thread_id IN (
  SELECT thread_id FROM thread_summaries WHERE cc_is_external_repo = true
);
