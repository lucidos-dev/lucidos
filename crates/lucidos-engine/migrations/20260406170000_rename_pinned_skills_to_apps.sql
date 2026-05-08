-- Rename pinned_skill_uis table to pinned_apps and update column names
ALTER TABLE IF EXISTS pinned_skill_uis RENAME TO pinned_apps;
ALTER TABLE IF EXISTS pinned_apps RENAME COLUMN skill_id TO app_id;
