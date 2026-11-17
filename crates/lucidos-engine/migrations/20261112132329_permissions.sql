-- Permissions (ADR 0664): what an owner's answer to a permission request
-- granted. A projection of PermissionGranted, PermissionSpent,
-- PermissionRevoked and ChangeApplied. A deleted thread takes the permissions
-- granted in it along.
CREATE TABLE IF NOT EXISTS permissions (
    permission_id UUID PRIMARY KEY,
    action TEXT NOT NULL,
    breadth TEXT,
    lifetime TEXT NOT NULL
        CHECK (lifetime IN ('once', 'this_thread', 'until_plan_lands', 'standing')),
    holder_kind TEXT NOT NULL CHECK (holder_kind IN ('thread', 'trigger', 'workspace')),
    holder_id TEXT,
    granted_in_thread_id UUID NOT NULL
        REFERENCES thread_summaries (thread_id) ON DELETE CASCADE,
    request_tool_use_id TEXT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ,
    end_reason TEXT CHECK (end_reason IN ('spent', 'revoked', 'plan_landed'))
);

CREATE INDEX IF NOT EXISTS permissions_live_by_action
    ON permissions (action) WHERE ended_at IS NULL;

-- A once permission covers one action. Two gates may both find it live; only
-- the first spend lands, and the second emit fails on this index.
CREATE UNIQUE INDEX IF NOT EXISTS events_permission_spent_unique
    ON events (((payload->>'permission_id')))
    WHERE event_type = 'PermissionSpent';

-- One answer grants at most once.
CREATE UNIQUE INDEX IF NOT EXISTS events_permission_granted_unique
    ON events ((thread_id), ((payload->>'request_tool_use_id')))
    WHERE event_type = 'PermissionGranted';
