-- Admit 'none' as a tap value. Passive notifications mark themselves read
-- the moment the user could see them (in-app toast shown, OR OS push tapped
-- which just launches the PWA, no deep-link). 'none' requires neither
-- app_id nor thread_id; the existing open_app / open_thread CHECKs already
-- gate only their own values and remain unchanged.
ALTER TABLE notifications
    DROP CONSTRAINT notifications_tap_valid;

ALTER TABLE notifications
    ADD CONSTRAINT notifications_tap_valid
    CHECK (tap IN ('modal', 'open_app', 'open_thread', 'none'));
