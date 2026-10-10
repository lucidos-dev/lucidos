-- Whether someone decided this credential's scope, an empty set included.
--
-- The startup scope inference (ADR 0144 decision 4) fills an EMPTY scope from
-- apis.json, which an app can write (ADR 0156). Emptiness alone cannot tell a
-- legacy row from a user who narrowed a key to "send nowhere", so the
-- inference re-ran on every start and could hand that key to an app's host.
--
-- A row inserted from now on is declared by the column default, and every
-- write path declares too. The next start gets one pass over what is left
-- undeclared, then declares every row.
ALTER TABLE credentials ADD COLUMN scope_declared BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE credentials ALTER COLUMN scope_declared SET DEFAULT true;

-- Only a legacy row stays undeclared. Two kinds of existing row were decided:
--   * One edited since it was created. Every edit path wrote the scope, and the
--     old inference never left a scope empty, so an empty one here was chosen.
--   * One created once ADR 0144 was accepted. From then on every way of saving
--     a credential named its scope, so an empty one was chosen too.
-- Declaring a legacy row by mistake fails loudly: the proxy refuses the key
-- and names Settings. Leaving a chosen one undeclared would leak it silently.
UPDATE credentials
SET scope_declared = true
WHERE updated_at > created_at
   OR created_at >= TIMESTAMPTZ '2026-08-27 00:00:00+00';
