-- Retire the passive `{"kind":"none"}` notification tap kind so every
-- notification is openable (modal or navigate). See
-- docs/plans/2026-07-02-remove-notification-tap-none.md.
--
-- 1. Rewrite existing passive rows to the openable modal default. (Historical
--    `NotificationCreated` EVENTS keep `{"kind":"none"}` forever — event-sourcing
--    immutability — but the engine's custom `Tap` Deserialize coerces those to
--    Modal on any replay, so a projection rebuild never re-introduces a `none`
--    row after this.)
UPDATE notifications
    SET tap = '{"kind":"modal"}'::jsonb
    WHERE tap->>'kind' = 'none';

-- 2. Tighten the CHECK so no NEW row can carry `none`. Sub-field validity
--    (e.g. `to.target`) is still enforced by the page-side router, not here.
ALTER TABLE notifications
    DROP CONSTRAINT IF EXISTS notifications_tap_valid;

ALTER TABLE notifications
    ADD CONSTRAINT notifications_tap_valid
    CHECK (tap ? 'kind' AND tap->>'kind' IN ('modal', 'navigate'));
