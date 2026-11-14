-- The owner's standing instruction to apply a change once its thread settles.
--
-- ADR 0168 clause 5: a thread acts on the owner's authority only while carrying
-- a standing instruction. The Apply button is one press; this table is the same
-- press, recorded so the engine can carry it out later. No thread waits on it,
-- and no thread reaches sideways to deliver it.
--
-- One row per armed thread, so re-arming replaces rather than stacks. A row is
-- one-shot: the settle resolver deletes it the moment it fires, drops it, or
-- finds it unfulfillable.
--
-- `change_id` set means the arm is bound to exactly that change, which is what
-- keeps a standing apply off a SECOND change the thread proposes later. NULL
-- means the sweep armed a thread that was still working with nothing proposed
-- yet, so it applies whatever that thread has pending when it settles.
--
-- `batch_id` names the Apply All sweep that armed the row, so cancelling the
-- batch drops everything it armed. NULL for a single arm.
--
-- Rows exist only while an arm is live: the events table
-- (`StandingApplyArmed` / `StandingApplyDropped`) keeps the audit trail.
CREATE TABLE standing_applies (
    thread_id  UUID PRIMARY KEY,
    change_id  UUID,
    batch_id   UUID,
    actor      JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The sweep-cancel path deletes by batch.
CREATE INDEX idx_standing_applies_batch_id ON standing_applies (batch_id)
    WHERE batch_id IS NOT NULL;
