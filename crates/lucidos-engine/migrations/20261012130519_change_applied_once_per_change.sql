-- A change is applied at most once. The apply claim makes a second apply
-- impossible within one engine (ADR 0442). This index makes a second
-- ChangeApplied fail at the DB even when two engines share the database.
--
-- Rows written before this migration may hold duplicates from before the claim
-- existed. Events are append-only, so the index covers only rows after the
-- highest sequence present when it is created.
DO $$
DECLARE
    floor_sequence BIGINT;
BEGIN
    SELECT COALESCE(MAX(sequence), 0) INTO floor_sequence FROM events;
    EXECUTE format(
        'CREATE UNIQUE INDEX IF NOT EXISTS events_change_applied_once '
        'ON events ((payload->>''change_id'')) '
        'WHERE event_type = ''ChangeApplied'' AND sequence > %s',
        floor_sequence
    );
END
$$;
