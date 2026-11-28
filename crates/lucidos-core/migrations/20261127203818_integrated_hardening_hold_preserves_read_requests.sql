-- An integrated hardening hold routes to the parent and leaves the child's
-- Blocked group. The child's direct reply to the user can still need a read.
-- Keep the two-argument function for engines running across this migration.

CREATE FUNCTION change_replaces_read_request(
    change_state TEXT, unproposed_reason TEXT, integrated BOOLEAN
)
RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
    SELECT change_state = 'proposed'
        OR (NOT integrated AND change_replaces_read_request(change_state, unproposed_reason))
$$;

CREATE OR REPLACE FUNCTION thread_summaries_read_request_yields() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF change_replaces_read_request(
        NEW.coding_agent_change_state, NEW.coding_agent_unproposed_reason, NEW.coding_agent_integrated
    ) THEN
        NEW.read_requested := FALSE;
    END IF;
    RETURN NEW;
END
$$;
