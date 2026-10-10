-- Record the push payload bytes alongside the delivery row so e2e tests can
-- assert the on-wire JSON shape (specifically the Declarative Web Push
-- envelope `{web_push: 8030, notification: {…}}`) instead of just delivery.
-- Same gating as the rest of push_log — only the `e2e-test-hooks` feature
-- writes to it.
ALTER TABLE push_log ADD COLUMN IF NOT EXISTS payload TEXT;
