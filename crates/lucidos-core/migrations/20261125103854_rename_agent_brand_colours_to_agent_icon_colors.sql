-- The drawer-only brand colour switch becomes the three-value
-- 'agent-icon-colors' setting. On ('true') is Brand and Off ('false') is
-- Muted (docs/plans/2026-11-25-agent-icon-colors.md).

-- A row already under the new key on the same device wins over the old one.
DELETE FROM preferences f
  USING preferences t
  WHERE f.key = 'agent-brand-colours'
    AND t.key = 'agent-icon-colors'
    AND COALESCE(f.device_id, '') = COALESCE(t.device_id, '');
UPDATE preferences
  SET key = 'agent-icon-colors',
      value = CASE WHEN value = 'false' THEN 'muted' ELSE 'brand' END
  WHERE key = 'agent-brand-colours';
