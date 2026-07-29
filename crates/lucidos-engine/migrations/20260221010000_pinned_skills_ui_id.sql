-- Rename pinned_skills → pinned_skill_uis and add ui_id column.

ALTER TABLE pinned_skills ADD COLUMN ui_id TEXT NOT NULL DEFAULT 'main';
ALTER TABLE pinned_skills DROP CONSTRAINT pinned_skills_skill_id_device_id_key;
ALTER TABLE pinned_skills RENAME TO pinned_skill_uis;
ALTER TABLE pinned_skill_uis ADD CONSTRAINT pinned_skill_uis_skill_id_ui_id_device_id_key UNIQUE (skill_id, ui_id, device_id);
