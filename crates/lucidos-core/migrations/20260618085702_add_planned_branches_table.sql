-- Durable "Planned" marker for coding-agent branches, modeled on
-- `hardened_branches`. Enforces that the `implementation-plan` skill (or an
-- explicit "simple change" acknowledgment) ran before a Lucidos-source change
-- can be applied. Keyed on (repo_root, branch_name) like the harden marker.
--
-- `state` is 'planned' (a docs/plans/ file was written + recorded) or
-- 'acknowledged_simple' (agent declared a local fix). Both states pass every
-- gate; only the absence of a row (Missing) blocks. `plan_path` is set for
-- 'planned', `reason` for 'acknowledged_simple'. `head_sha` is recorded for
-- diagnostics only — unlike hardening, the plan gate is binary present/absent
-- (a follow-up commit does not invalidate a settled plan decision).
CREATE TABLE planned_branches (
    repo_root TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('planned', 'acknowledged_simple')),
    plan_path TEXT,
    reason TEXT,
    head_sha TEXT NOT NULL,
    planned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repo_root, branch_name)
);
