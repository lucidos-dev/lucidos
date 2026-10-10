CREATE TABLE drafts (
    id            uuid PRIMARY KEY,
    thread_id     uuid NULL,
    mode          text NOT NULL,
    text          text NOT NULL,
    images        jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at    timestamptz NOT NULL,
    updated_at    timestamptz NOT NULL,
    deleted_at    timestamptz NULL
);

CREATE INDEX drafts_updated_at_idx
    ON drafts (updated_at DESC)
    WHERE deleted_at IS NULL;

-- At most one live followup per thread
CREATE UNIQUE INDEX drafts_one_followup_per_thread
    ON drafts (thread_id)
    WHERE thread_id IS NOT NULL AND deleted_at IS NULL;
