ALTER TABLE planned_branches DROP CONSTRAINT planned_branches_state_check;
ALTER TABLE planned_branches ADD CONSTRAINT planned_branches_state_check
    CHECK (state IN ('planned', 'acknowledged_simple', 'proposed', 'bounded_security_fix', 'plan_only'));
ALTER TABLE planned_branches ADD COLUMN kind TEXT NOT NULL DEFAULT 'implementation'
    CHECK (kind IN ('design', 'implementation'));
