-- A change that already lists the thread replaces a read request (ADR 0421).
--
-- Two change states list a thread without a read request: a proposed change
-- (Review) and work held for a missing harden (Blocked). A request beside
-- either would outlive the Apply as an unread dot, so `read_requested` is then
-- FALSE. Other held work lists the thread nowhere, so it keeps its request.
--
-- `change_replaces_read_request` is the one definition of those two states.
-- The trigger applies it for every writer, some of which emit no event, and
-- the turn-end gate reads it to decide no without a model call.
--
-- The trigger's name sorts before `thread_summaries_version_update`, so it
-- runs first and the version trigger sees the final row (ADR 0329).

CREATE FUNCTION change_replaces_read_request(change_state TEXT, unproposed_reason TEXT)
RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
    SELECT change_state = 'proposed'
        OR (change_state = 'unproposed' AND unproposed_reason IS NOT DISTINCT FROM 'hardening_missing')
$$;

UPDATE thread_summaries
   SET read_requested = FALSE
 WHERE read_requested
   AND change_replaces_read_request(coding_agent_change_state, coding_agent_unproposed_reason);

CREATE FUNCTION thread_summaries_read_request_yields() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF change_replaces_read_request(NEW.coding_agent_change_state, NEW.coding_agent_unproposed_reason) THEN
        NEW.read_requested := FALSE;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER thread_summaries_read_request_yields
    BEFORE INSERT OR UPDATE ON thread_summaries
    FOR EACH ROW EXECUTE FUNCTION thread_summaries_read_request_yields();
