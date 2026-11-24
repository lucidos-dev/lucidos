-- Hard-cut: notifications.tap moves from TEXT enum ('modal'|'open_app'|
-- 'open_thread'|'none') to JSONB carrying a discriminated union
-- (`{"kind": "modal" | "none" | "navigate", "to"?: NavigateUi}`).
--
-- The new shape collapses the open_app / open_thread special-cases into
-- `{"kind":"navigate","to":...}`, which delegates to the same target/sub-field
-- router the `navigate_ui` LLM tool uses. Future nav targets (file, settings,
-- url, etc.) become reachable from a notification tap without another column
-- migration.
--
-- Rewrites existing rows:
--   'modal'       -> {"kind":"modal"}
--   'none'        -> {"kind":"none"}
--   'open_app'    -> {"kind":"navigate","to":{"target":"app","app_id":<row.app_id>}}
--   'open_thread' -> {"kind":"navigate","to":{"target":"thread","id":<row.thread_id>,
--                                              "event_id"?:<row.event_id>}}
-- (jsonb_strip_nulls drops `event_id` from the open_thread case when null.)

-- Old CHECK constraints reference the TEXT shape; drop before retyping.
ALTER TABLE notifications
    DROP CONSTRAINT IF EXISTS notifications_tap_valid;
ALTER TABLE notifications
    DROP CONSTRAINT IF EXISTS notifications_open_app_requires_app_id;
ALTER TABLE notifications
    DROP CONSTRAINT IF EXISTS notifications_open_thread_requires_thread_id;

-- Drop the TEXT-typed default so ALTER TYPE doesn't refuse on the cast.
ALTER TABLE notifications
    ALTER COLUMN tap DROP DEFAULT;

ALTER TABLE notifications
    ALTER COLUMN tap TYPE JSONB
    USING (
        CASE
            WHEN tap = 'modal' THEN '{"kind":"modal"}'::jsonb
            WHEN tap = 'none' THEN '{"kind":"none"}'::jsonb
            -- open_app needs app_id for the new shape to point anywhere.
            -- Pre-migration CHECK enforced this, but defend against a
            -- constraint-violating row that slipped in: fall back to Modal
            -- so the tap at least opens the inbox instead of becoming a
            -- silent no-op (`{kind:'navigate',to:{target:'app'}}` would
            -- pass the new CHECK but have nowhere to navigate).
            WHEN tap = 'open_app' AND app_id IS NOT NULL THEN jsonb_build_object(
                'kind', 'navigate',
                'to', jsonb_build_object(
                    'target', 'app',
                    'app_id', app_id
                )
            )
            -- open_thread needs thread_id for the same reason. event_id is
            -- optional (jsonb_strip_nulls drops it when null).
            WHEN tap = 'open_thread' AND thread_id IS NOT NULL THEN jsonb_build_object(
                'kind', 'navigate',
                'to', jsonb_strip_nulls(jsonb_build_object(
                    'target', 'thread',
                    'id', thread_id::text,
                    'event_id', event_id::text
                ))
            )
            -- Defensive fallback: open_app/open_thread without their target
            -- id, or any historical value (shouldn't exist given the old
            -- CHECK) lands as Modal so we don't produce a dead navigate tap.
            ELSE '{"kind":"modal"}'::jsonb
        END
    );

ALTER TABLE notifications
    ALTER COLUMN tap SET DEFAULT '{"kind":"modal"}'::jsonb;

ALTER TABLE notifications
    ALTER COLUMN tap SET NOT NULL;

-- New CHECK enforces the discriminator. Sub-field validity (e.g. `to.target`
-- enum membership, `app_id` required for `navigate{target=app}`) is enforced
-- by the engine's serde decode + the page-side router, not by SQL — keeps
-- the JSONB column open to additive nav-target additions without a migration
-- on every change.
ALTER TABLE notifications
    ADD CONSTRAINT notifications_tap_valid
    CHECK (tap ? 'kind' AND tap->>'kind' IN ('modal', 'none', 'navigate'));
