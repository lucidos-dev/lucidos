-- Only EventBus's diagnostic inbox writer mutates this table. Its event and
-- payload commit atomically. Payload reads are withheld from the agent role.
CREATE TABLE diagnostic_bundles (
    id UUID PRIMARY KEY,
    origin TEXT NOT NULL CHECK (octet_length(origin) BETWEEN 1 AND 256),
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    payload BYTEA NOT NULL CHECK (octet_length(payload) BETWEEN 1 AND 33554432),
    received TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX diagnostic_bundles_received ON diagnostic_bundles(received);
