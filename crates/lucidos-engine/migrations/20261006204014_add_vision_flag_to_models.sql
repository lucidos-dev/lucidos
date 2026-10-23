-- The *vision flag*: whether a model reads images.
--
-- Image description sends the user's images to its model. Since ADR 0375 its
-- picker offers every registry model, so it could be pointed at a text-only
-- one, and every description then failed with only a log line to show for it.
-- The flag lets the picker, the auxiliary default and the engine tell the two
-- apart. See docs/plans/2026-10-06-vision-flag-for-image-description.md.
--
-- Per model, not per route: reading images is a property of the model, and a
-- route is only a backend serving that same model.
--
-- FALSE means "not known to read images", so a user row starts there. The
-- user flips it in Settings -> Models, the API, `manage_models` or the CLI.

ALTER TABLE models ADD COLUMN IF NOT EXISTS vision BOOLEAN NOT NULL DEFAULT FALSE;

-- Seed ONLY the builtin families published as image-reading. The seed errs
-- low on purpose: over-declaring sends images to a model that rejects them,
-- which is the bug this column exists to stop. Under-declaring only hides a
-- model from the image description picker until somebody flips it.
--
-- Left FALSE as unverified: gpt-5.3-codex-spark, z-ai/glm-5.2, the grok-*
-- rows and the OpenCode free rows. `source = 'builtin'` leaves a user row
-- under a matching id alone, and `NOT vision` makes a re-run a no-op.
UPDATE models SET vision = TRUE, updated_at = NOW()
WHERE source = 'builtin'
  AND NOT vision
  AND (
        id LIKE 'claude-%'
     OR id LIKE 'gemini-%'
     OR ((id LIKE 'gpt-5%' OR id LIKE 'gpt-6%') AND id <> 'gpt-5.3-codex-spark')
  );
