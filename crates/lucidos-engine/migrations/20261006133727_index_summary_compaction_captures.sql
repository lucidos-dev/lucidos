-- The Tree backfill estimate averages real compactor usage per model from
-- these captures. Without the index each read scanned every event. The
-- predicate must match MEASURED_USAGE_SQL in engine/summary_tree/estimate.rs,
-- or the planner cannot use it.
CREATE INDEX idx_events_summary_compaction_captures
    ON events ((payload->>'model'))
    WHERE event_type = 'ContextCaptured'
      AND payload->>'purpose' = 'summary_compaction';
