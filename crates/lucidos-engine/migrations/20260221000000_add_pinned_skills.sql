-- Add pinned_skills table for per-device skill pinning to mobile menu.

CREATE TABLE IF NOT EXISTS pinned_skills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    skill_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (skill_id, device_id)
);
