-- Phase 10.1 Step B: backfill `ChangeHardened` and `MergeResolutionStarted`
-- events for legacy `changes` rows whose state was set via the deleted
-- `set_hardened` / `set_merge_worktree` UPDATE paths and never produced a
-- corresponding event. Without these synthetic events the in-memory
-- ChangesProjection misses `hardened=true` and active merge worktrees on
-- engine restart, breaking apply/restart-toast/merge-cleanup paths.
--
-- Append-only: per CLAUDE.md, events are immutable. We INSERT new rows,
-- never UPDATE existing ones. Idempotent: NOT EXISTS guards prevent
-- duplicate emits if the migration re-runs.
--
-- Once the legacy `changes` table is dropped (follow-up release), this
-- migration's INSERTs become no-ops and can stay as historical record.

INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id)
SELECT gen_random_uuid(),
       'ChangeHardened',
       jsonb_build_object('change_id', c.id::text),
       COALESCE(c.resolved_at, c.created_at),
       'thread',
       c.thread_id::text,
       c.thread_id
FROM changes c
WHERE c.hardened = true
  AND c.thread_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM events e
      WHERE e.event_type = 'ChangeHardened'
        AND e.payload->>'change_id' = c.id::text
  );

INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id)
SELECT gen_random_uuid(),
       'MergeResolutionStarted',
       jsonb_build_object(
           'change_id', c.id::text,
           'worktree_path', c.merge_worktree_path,
           'temp_branch', c.merge_temp_branch
       ),
       c.created_at,
       'thread',
       c.thread_id::text,
       c.thread_id
FROM changes c
WHERE c.merge_worktree_path IS NOT NULL
  AND c.merge_temp_branch IS NOT NULL
  AND c.thread_id IS NOT NULL
  AND c.status = 'pending'
  AND NOT EXISTS (
      SELECT 1 FROM events e
      WHERE e.event_type = 'MergeResolutionStarted'
        AND e.payload->>'change_id' = c.id::text
  );

-- The legacy `changes` table is intentionally NOT dropped here. The
-- ChangesProjection JOINs against it during `rebuild_from_events` to
-- backfill branch_name/repo_root/hardened/pre_merge_sha/post_merge_sha
-- for pre-Step-A `ChangeProposed` events whose payloads predate those
-- fields. Drop in a follow-up release once a rollout has confirmed no
-- read site escaped migration.
