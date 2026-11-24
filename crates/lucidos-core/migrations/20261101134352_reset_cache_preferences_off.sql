-- The owner decided everyone starts with cache keep-alive and cache cards off on upgrade to v0.55.1.
UPDATE preferences SET value = 'off'
  WHERE key = 'cache_keep_alive_mode'
    AND value IN ('on', 'break-even', 'tuned', 'workspace-data');

UPDATE preferences SET value = 'false'
  WHERE key = 'cache_cards' AND value <> 'false';
