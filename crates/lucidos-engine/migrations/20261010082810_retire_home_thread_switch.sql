-- Retire the home thread switch (ADR 0411).
--
-- Every workspace now has Home, so `home_thread_enabled` no longer means
-- anything. A stored row, `false` included, would only be dead data. The
-- home thread itself is left alone: its row, its id and its events stay, and
-- with no switch left to hide it, it shows.
DELETE FROM preferences WHERE key = 'home_thread_enabled';
