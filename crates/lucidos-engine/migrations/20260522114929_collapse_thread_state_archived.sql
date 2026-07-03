-- Restore the columns to their original orthogonal design (per
-- 20260503215740_threadcomposestate.sql):
--   `state`         = compose lifecycle only (composing | active | discarded)
--   `archive_state` = the sole archive flag        (inbox    | archived)
--
-- Background: the `ThreadArchived` projection used to also write
-- `state = 'archived'`, duplicating the archive fact across two columns. The
-- contract layer (`thread_lifecycle::resolve_transition`) writes the same fact
-- to `archive_state` via `apply_transition`. When a system-cleanup event such
-- as `ResponseAborted { cause: RecoveryAfterRestart }` landed on a thread the
-- user had archived, the contract's `to_inbox` rule flipped `archive_state`
-- back to `'inbox'` without touching `state`, producing rows like
-- `(state='archived', archive_state='inbox')` — an "impossible state" the
-- user spotted that surfaced as a previously-archived thread reanimating
-- in the inbox on engine restart. See
-- `.claude/plans/indexed-watching-flute.md` for the full investigation.
--
-- Any row currently carrying `state='archived'` had its user-archive intent
-- persisted only to the `state` column. Move that intent to `archive_state`
-- (winning over any divergent `'inbox'` value the contract layer may have
-- written) and reset `state` to `'active'` — the compose-lifecycle position
-- the row would have parked at if `'archived'` had never been a legal
-- `state` value.
--
-- The accompanying Rust code change drops `ThreadState::Archived` and stops
-- the projection from writing `state='archived'`, so no new rows can land
-- in the divergent shape post-migration. `ThreadState::from_db_str` now
-- rejects `'archived'` loudly, so any stray row that slipped through during
-- deploy will surface as a 500 with a clear error message rather than
-- silently routing to a no-longer-existing arm.

UPDATE thread_summaries
   SET state = 'active',
       archive_state = 'archived'
 WHERE state = 'archived';
