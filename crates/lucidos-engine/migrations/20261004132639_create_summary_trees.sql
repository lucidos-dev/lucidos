-- Summary trees (ADR 0362): binary trees of summary lines over a log the
-- events table already holds. Only the nodes are new storage, and every node
-- is a projection the compactor can rebuild from events.
--
-- A scope is 'workspace' or one thread id. A node covers the log entries
-- [start, start + span), where span is a power of 2 and start is a multiple
-- of it, as OptChat addresses them.

CREATE TABLE summary_tree_nodes (
    scope TEXT NOT NULL,
    start BIGINT NOT NULL,
    span BIGINT NOT NULL,
    text TEXT NOT NULL,
    -- The model that wrote the line. NULL for a free node: a source that
    -- already fit the target size and became the node verbatim.
    model TEXT,
    -- Leaves only: the event the entry came from. A thread leaf names its
    -- message, a workspace leaf its settled turn or artifact write.
    source_event_id UUID,
    -- Workspace turn leaves only: the thread that turn belongs to, which is
    -- how a thread delete finds them.
    source_thread_id UUID,
    built_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope, span, start),
    CONSTRAINT summary_tree_nodes_scope CHECK (
        scope = 'workspace'
        OR scope ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    ),
    CONSTRAINT summary_tree_nodes_address CHECK (
        span > 0 AND (span & (span - 1)) = 0 AND start >= 0 AND start % span = 0
    ),
    CONSTRAINT summary_tree_nodes_leaf_source CHECK ((span = 1) = (source_event_id IS NOT NULL)),
    CONSTRAINT summary_tree_nodes_turn_thread CHECK (
        source_thread_id IS NULL OR (scope = 'workspace' AND span = 1)
    )
);

CREATE INDEX summary_tree_nodes_source_thread
    ON summary_tree_nodes (source_thread_id)
    WHERE source_thread_id IS NOT NULL;

-- The compactor's durable progress. A scope is complete for every event with
-- a sequence at or below reflected_through, so a restart resumes from here.
-- backfilled_at is the per-workspace ready flag, set once on the workspace row.
CREATE TABLE summary_tree_scopes (
    scope TEXT PRIMARY KEY,
    reflected_through BIGINT NOT NULL DEFAULT 0,
    backfilled_at TIMESTAMPTZ,
    CONSTRAINT summary_tree_scopes_scope CHECK (
        scope = 'workspace'
        OR scope ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    ),
    CONSTRAINT summary_tree_scopes_ready_flag CHECK (backfilled_at IS NULL OR scope = 'workspace')
);

-- The workspace log reads its leaf events by sequence. The list must match
-- WORKSPACE_LEAF_EVENT_TYPES in engine/summary_tree/workspace_log.rs, or the
-- planner cannot use this index.
CREATE INDEX idx_events_summary_tree_workspace_leaves
    ON events (sequence)
    WHERE event_type IN (
        'ResponseGenerated', 'ResponseCanceled', 'ResponseAborted', 'ResponseFailed',
        'CodingAgentIdled', 'ArtifactCreated', 'ArtifactUpdated', 'ArtifactImported'
    );
