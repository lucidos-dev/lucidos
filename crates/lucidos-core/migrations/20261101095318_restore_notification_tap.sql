-- An Undo restore tap names a recovery snapshot and enters normal admission.
-- It grants no authority to restore and never executes on delivery.
ALTER TABLE notifications DROP CONSTRAINT notifications_tap_valid;
ALTER TABLE notifications ADD CONSTRAINT notifications_tap_valid
    CHECK (
        tap ? 'kind' AND (
            tap->>'kind' IN ('modal', 'navigate')
            OR (
                tap->>'kind' = 'restore-snapshot'
                AND jsonb_typeof(tap->'workspace_id') = 'string'
                AND length(tap->>'workspace_id') > 0
                AND jsonb_typeof(tap->'handle') = 'string'
                AND tap->>'handle' ~ '^snap_[0-7][0123456789ABCDEFGHJKMNPQRSTVWXYZ]{25}$'
            ) IS TRUE
        )
    );
