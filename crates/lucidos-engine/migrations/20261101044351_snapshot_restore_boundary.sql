-- Observed external restore journal identity, installed before any dispatcher.
-- One current generation survives later process crashes without its marker.
CREATE TABLE snapshot_restore_boundary (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    restore_id UUID NOT NULL,
    snapshot TEXT NOT NULL,
    restored_at TIMESTAMPTZ NOT NULL,
    event_sequence BIGINT NOT NULL CHECK (event_sequence >= 0)
);
