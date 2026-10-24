-- Durable membership record for in-flight Apply All batches.
--
-- The Apply-All batch state machine (`engine::apply_all_batches::ApplyAllRegistry`)
-- lives in memory and is advanced by the driver task as each member's
-- `ChangeApplied` / `ChangeApplyFailed` lands. Before this table the registry was
-- in-memory ONLY, so an engine restart mid-batch (an earlier member required a
-- restart, or a conflict-resolution apply landed a restart-requiring change) lost
-- the batch: the eventual terminal event found no batch in the registry, no
-- `ApplyAllBatchCompleted` was ever emitted, and the frontend's "Applying
-- changes…" toast stayed stuck forever.
--
-- This projection stores batch MEMBERSHIP only (the change ids + who clicked
-- Apply All). Per-member resolution is NOT stored here — recovery reconstructs it
-- from the authoritative `changes.status` at boot (applied → applied, discarded /
-- missing → terminal, pending → re-drive). The driver remains the single live
-- authority for batch state; this row is its durable mirror.
--
-- Rows exist only while a batch is live: inserted on `ApplyAllBatchStarted`
-- (start_apply_all_batch), deleted on `ApplyAllBatchCompleted` (driver complete,
-- cancel, or startup recovery). The set is therefore self-bounding — a clean boot
-- finds zero rows; the working set is at most the batches in flight when the
-- previous process died. The `events` table keeps the full audit trail.
CREATE TABLE apply_all_batches (
    batch_id   UUID PRIMARY KEY,
    change_ids UUID[] NOT NULL,
    actor      JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
