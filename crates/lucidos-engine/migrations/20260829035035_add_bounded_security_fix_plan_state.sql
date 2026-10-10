-- Add the 'bounded_security_fix' plan-marker state and the file list it is
-- bounded to.
--
-- The lane exists because two rules deadlocked the nightly security pass: the
-- orchestrator must never pause for input, and the plan gate needs a human
-- decision before a security change is edited. An unattended run may now commit
-- a security fix confined to a small, named set of files. The state satisfies
-- the gate, like 'planned' and 'acknowledged_simple', but it is its own value
-- so a reviewer can tell the three apart. See
-- docs/plans/2026-08-29-unattended-security-fixes-get-a-bounded-lane.md.
--
-- `files` is the recorded bound. The Apply floor re-derives what the branch
-- actually changed and refuses anything outside this list, so the column is
-- load-bearing rather than diagnostic. It stays NULL for every other state.
--
-- Widen the CHECK constraint the same way 20260619112446 did. The constraint
-- was created inline in 20260618085702, so Postgres auto-named it
-- `planned_branches_state_check`.
ALTER TABLE planned_branches DROP CONSTRAINT IF EXISTS planned_branches_state_check;
ALTER TABLE planned_branches
    ADD CONSTRAINT planned_branches_state_check
    CHECK (state IN ('planned', 'acknowledged_simple', 'proposed', 'bounded_security_fix'));

ALTER TABLE planned_branches ADD COLUMN IF NOT EXISTS files TEXT[];

-- A bounded fix without its bound would satisfy the gate with nothing to check
-- against, which is the whole lane defeated. Enforce the pairing in the schema
-- so no write path can skip it.
--
-- `cardinality`, NOT `array_length(files, 1)`. The latter returns NULL for an
-- empty array rather than 0, and a CHECK whose result is NULL passes, so
-- `files = '{}'` would have slipped through the guarantee this constraint
-- exists for.
ALTER TABLE planned_branches DROP CONSTRAINT IF EXISTS planned_branches_bounded_files_check;
ALTER TABLE planned_branches
    ADD CONSTRAINT planned_branches_bounded_files_check
    CHECK (
        (state = 'bounded_security_fix' AND files IS NOT NULL AND cardinality(files) > 0)
        OR (state <> 'bounded_security_fix' AND files IS NULL)
    );
