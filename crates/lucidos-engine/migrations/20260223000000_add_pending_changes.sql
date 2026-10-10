CREATE TABLE IF NOT EXISTS pending_changes (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id       UUID NOT NULL,
    branch_name      TEXT NOT NULL,
    repo_root        TEXT NOT NULL,
    description      TEXT NOT NULL DEFAULT '',
    file_count       INT NOT NULL DEFAULT 0,
    files            TEXT[] NOT NULL DEFAULT '{}',
    has_rust_changes BOOLEAN NOT NULL DEFAULT FALSE,
    status           TEXT NOT NULL DEFAULT 'pending',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at      TIMESTAMPTZ
);

CREATE INDEX idx_pending_changes_status ON pending_changes (status);
