-- Add the 'proposed' (awaiting-approval) plan-marker state.
--
-- A plan now starts in 'proposed' (recorded by the `implementation-plan` skill)
-- and is flipped to 'planned' only once the human approves it (the coding agent
-- runs `lucidos planned approve` after the user's chat approval). 'proposed' does
-- NOT satisfy the cc-plan-gate hook or the Apply floor — implementation stays
-- blocked until approval. 'planned' (approved, or any legacy skill-recorded row)
-- and 'acknowledged_simple' (local fix) continue to satisfy every gate.
--
-- Widen the existing CHECK constraint to admit the new state. The constraint was
-- created inline in 20260618085702_add_planned_branches_table.sql, so Postgres
-- auto-named it `planned_branches_state_check`.
ALTER TABLE planned_branches DROP CONSTRAINT IF EXISTS planned_branches_state_check;
ALTER TABLE planned_branches
    ADD CONSTRAINT planned_branches_state_check
    CHECK (state IN ('planned', 'acknowledged_simple', 'proposed'));
