-- A definite refusal can restore each spent once grant at most once.
CREATE UNIQUE INDEX events_permission_restored_unique
    ON events ((payload->>'spent'))
    WHERE event_type = 'PermissionRestored';
