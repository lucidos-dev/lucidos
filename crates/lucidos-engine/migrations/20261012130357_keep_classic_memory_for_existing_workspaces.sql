-- Keep a used workspace on the Classic memory module (ADR 0362).
--
-- The `memory_module` default is now `tree`. Read through that default, a used
-- workspace that never chose would start a paid backfill at upgrade without
-- the confirm Settings shows. So it stores the `classic` it runs today, and a
-- release notice asks it to move. A stored value, either one, is kept.
--
-- "Used" is the test release-notice placement applies (`place_workspace`): a
-- thread other than Home. So every workspace kept here is one shown the
-- notice. A new workspace, or one that only ever booted, reads the default.
INSERT INTO preferences (key, value, device_id)
SELECT 'memory_module', 'classic', NULL
WHERE EXISTS (SELECT 1 FROM thread_summaries WHERE NOT is_home)
ON CONFLICT (key, COALESCE(device_id, '')) DO NOTHING;
