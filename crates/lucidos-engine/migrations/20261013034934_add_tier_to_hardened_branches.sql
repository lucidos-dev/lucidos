-- A harden marker records which tier of `/harden` ran. An integrated
-- sub-thread runs the fast tier, every other branch the full one. Every
-- marker written before this was a full run.

ALTER TABLE hardened_branches
    ADD COLUMN tier TEXT NOT NULL DEFAULT 'full',
    ADD CONSTRAINT hardened_branches_tier_check CHECK (tier IN ('fast', 'full'));
