ALTER TABLE planned_branches
    ADD COLUMN plan_version TEXT,
    ADD COLUMN carried_without_plan BOOLEAN NOT NULL DEFAULT FALSE;
