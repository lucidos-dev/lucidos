-- Discriminate coding-agent threads by what they edit:
--   'lucidos'  — edits Lucidos source (NULL on legacy rows; consumers default to this)
--   'app'      — edits a data/apps/<id>/ folder in the workspace (NEW)
--   'external' — edits a registered external repo (NULL on legacy rows;
--                kind-aware consumers can join `repositories` for the
--                authoritative external-vs-Lucidos answer)
--
-- The projection writes these columns from SessionStarted.coding_agent_kind and
-- SessionStarted.coding_agent_folder. The Apply path reads coding_agent_kind to
-- decide whether to short-circuit /harden, skip engine restart, and emit
-- AppUiRefreshRequested. The WIP-preview app-ui route reads coding_agent_folder
-- to locate the worktree's app folder when ?thread_id=<id> is passed.
--
-- Nullable + no default so the projection sees NULL on rows from the legacy
-- pre-cc_kind era; consumers fall back to "lucidos" when reading NULL.
ALTER TABLE thread_summaries
    ADD COLUMN coding_agent_kind TEXT,
    ADD COLUMN coding_agent_folder TEXT;

-- Intentionally NOT backfilling kind here. The engine ALWAYS writes a
-- non-NULL cc_repo_id (Lucidos-source threads get the Lucidos repo's UUID
-- via RepositoryStore::get_by_name("Lucidos") in run_session.rs), so a
-- naive `cc_repo_id IS NOT NULL → external` rule would mislabel every
-- legacy Lucidos-source thread as External. We don't know the Lucidos
-- UUID at migration time either. Consumers default NULL to 'lucidos'
-- (CodingAgentKind::parse + the apply path's load_apply_kind_context),
-- which is the right answer for the overwhelming majority of legacy rows
-- — pre-migration there were no app threads, and external-repo rows still
-- carry their cc_repo_id so any kind-aware UI label can join repositories
-- and decide. The first SessionStarted on each thread post-migration
-- writes the authoritative kind via the projection arm.
