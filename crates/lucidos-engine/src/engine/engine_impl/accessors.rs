use super::super::*;
use crate::core::contained_path::{join_contained, join_data_contained};
use crate::llm::judgment::{system_one_or_default_for, JudgmentProvider, JudgmentSite};

/// INVARIANT: Release the apply claim that the session behind `claimant` took.
///
/// An apply task can outlive the session it claimed, and a replacement on the
/// same thread takes its own claim. Clearing by thread id alone would release
/// that one, and a second apply could then merge beside it. `msg_tx` is the
/// session's identity, as in `SessionEntryGuard`.
pub fn release_change_claim(
    sessions: &mut std::collections::HashMap<Uuid, crate::engine::AgentSession>,
    thread_id: Uuid,
    claimant: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
) {
    if let Some(s) = sessions
        .get_mut(&thread_id)
        .filter(|s| s.msg_tx.same_channel(claimant))
    {
        s.change_claim = None;
    }
}

impl LucidosEngine {
    pub async fn judgment_system_one(
        &self,
        site: JudgmentSite,
        timeout: std::time::Duration,
    ) -> Option<Box<dyn JudgmentProvider>> {
        let served = self
            .current_provider()
            .configured_providers()
            .unwrap_or_default();
        system_one_or_default_for(&self.pool, site, timeout, &served).await
    }

    pub fn agent_host(&self) -> &dyn agent_host::AgentHost {
        self.agent_host.as_ref()
    }

    pub(crate) fn remembers_cc_system_prompt(&self, thread_id: Uuid) -> bool {
        self.cc_system_prompts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&thread_id)
    }

    pub async fn is_agent_running_for(&self, thread_id: Uuid) -> bool {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(|s| s.is_live())
    }

    pub async fn upsert_cc_commands_cache(&self, repo_key: String, info: CcCommandsInfo) {
        let json = {
            let mut cache = self.cc_commands_cache.write().await;
            if cache.get(&repo_key) == Some(&info) {
                return;
            }
            cache.insert(repo_key, info);
            match serde_json::to_string(&*cache) {
                Ok(json) => Some(json),
                Err(e) => {
                    log_legacy!("[ClaudeCode] Failed to serialize CC commands cache: {}", e);
                    None
                }
            }
        };
        if let Some(json) = json {
            let path = self.workspace_path.join(".lucidos/cc-commands.json");
            if let Err(e) = std::fs::write(&path, &json) {
                log_legacy!("[ClaudeCode] Failed to write CC commands cache: {}", e);
            }
        }
    }

    pub fn vertex_project_id(&self) -> &str {
        &self.vertex_project_id
    }

    pub fn vertex_location(&self) -> &crate::llm::vertex::LocationHandle {
        &self.vertex_location
    }

    pub fn vertex_token_cache(&self) -> Option<crate::llm::vertex::TokenCache> {
        self.vertex_token_cache.clone()
    }

    pub fn resolve_data_path(
        &self,
        relative_path: &str,
    ) -> Result<(String, std::path::PathBuf), String> {
        let normalized = normalize_data_path(relative_path)?;

        if let Some(rel) = normalized.strip_prefix("system-knowhow/") {
            let dir = self
                .system_knowhow_dir
                .as_deref()
                .ok_or_else(|| "System knowhow is not available".to_string())?;
            let full_path = join_contained(dir, rel).ok_or_else(|| symlink_escape(&normalized))?;
            return Ok((normalized, full_path));
        }

        if let Some(full_path) = self.app_manager.built_in_data_path(&normalized) {
            return Ok((normalized, full_path));
        }

        if crate::core::is_tmp_path(&normalized) {
            let full_path = resolve_tmp_path(&self.workspace_path, &normalized)?;
            return Ok((normalized, full_path));
        }

        let full_path = join_data_contained(
            &self.workspace_path.join(crate::core::DATA_DIR),
            &normalized,
        )
        .ok_or_else(|| symlink_escape(&normalized))?;

        if normalized.starts_with("knowhow/") && !full_path.exists() {
            let kh_relative = normalized.strip_prefix("knowhow/").unwrap();
            let shared_path = self
                .shared_knowhow_dir()
                .and_then(|shared_dir| join_contained(&shared_dir, kh_relative));
            if let Some(shared_path) = shared_path.filter(|p| p.exists()) {
                return Ok((normalized, shared_path));
            }
        }

        Ok((normalized, full_path))
    }

    /// INVARIANT: Set the self-reference after wrapping in Arc. Must be called once after Arc::new.
    pub fn set_self_arc(&self, arc: &Arc<LucidosEngine>) {
        if self.self_arc.set(Arc::downgrade(arc)).is_err() {
            log_legacy!("[Engine] set_self_arc called twice: keeping the first Arc");
        }
        // INVARIANT: The Thread Queue executes admitted entries through the engine —
        // wire it the moment the Arc exists so no submission can race a
        // missing executor.
        self.thread_queue.attach_engine(arc);
    }

    pub fn clone_arc(&self) -> Arc<LucidosEngine> {
        self.self_arc
            .get()
            .expect("self_arc not initialized")
            .upgrade()
            .expect("engine dropped while in use")
    }

    pub fn try_clone_arc(&self) -> Option<Arc<LucidosEngine>> {
        self.self_arc.get().and_then(std::sync::Weak::upgrade)
    }

    pub fn workspace_path(&self) -> &std::path::Path {
        &self.workspace_path
    }

    pub async fn resolve_thread_app_worktree(
        &self,
        thread_id: uuid::Uuid,
    ) -> Option<std::path::PathBuf> {
        let sessions = self.agent_sessions.lock().await;
        let live = sessions
            .get(&thread_id)
            .and_then(|s| s.worktree_path.clone());
        drop(sessions);
        if let Some(p) = live {
            if p.exists() {
                return Some(p);
            }
        }
        let resolved = crate::engine::session_queries::resolve_worktree_path(
            self.pool(),
            thread_id,
            self.workspace_path(),
            self.workspace_path(),
            None,
        )
        .await;
        if resolved.exists() && resolved.join(".git").exists() {
            Some(resolved)
        } else {
            None
        }
    }

    /// INVARIANT: Hold across any write+commit on the workspace repo. See
    /// [`crate::core::repository_lock::WORKSPACE_REPO_MUTEX`] for what else holds it.
    pub async fn lock_workspace_repo(&self) -> crate::engine::git_ops::WorkspaceRepoLock {
        crate::engine::git_ops::lock_workspace_repo().await
    }

    pub fn repo_root(&self) -> &std::path::Path {
        &self.repo_root
    }

    pub fn proxy_token_cache(&self) -> &lucidos_core::engine::proxy_token_cache::ProxyTokenCache {
        &self.proxy_token_cache
    }

    pub fn proxy_token_cache_arc(
        &self,
    ) -> Arc<lucidos_core::engine::proxy_token_cache::ProxyTokenCache> {
        self.proxy_token_cache.clone()
    }

    pub fn proxy_modules(
        &self,
    ) -> &Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<
                String,
                Arc<crate::engine::proxy_wasm_signer::CompiledModule>,
            >,
        >,
    > {
        &self.proxy_modules
    }

    /// INVARIANT: Shared wasmtime engine. Use this for any module compilation or
    /// instantiation — wasmtime forbids cross-engine instantiation.
    pub fn wasm_engine(&self) -> &Arc<wasmtime::Engine> {
        &self.wasm_engine
    }

    pub fn shared_knowhow_dir(&self) -> Option<PathBuf> {
        self.user_dir.as_ref().map(|ud| ud.join("knowhow"))
    }

    pub fn system_knowhow_dir(&self) -> Option<&std::path::Path> {
        self.system_knowhow_dir.as_deref()
    }

    pub fn knowhow_dirs(&self) -> crate::core::knowhow::KnowhowDirs {
        crate::core::knowhow::KnowhowDirs {
            shared: self.shared_knowhow_dir(),
            local: self.workspace_path.join(crate::core::KNOWHOW_DIR),
            apps: Some(self.workspace_path.join(crate::core::APPS_DIR)),
            triggers: Some(self.workspace_path.join(crate::core::TRIGGERS_DIR)),
        }
    }

    pub fn system_knowhow_summaries(&self) -> Vec<crate::core::KnowhowSummary> {
        self.system_knowhow_dir()
            .map(crate::core::SystemKnowhowStore::load_summaries)
            .unwrap_or_default()
    }

    /// INVARIANT: The docs the Know-how routing list names: the workspace's own, then
    /// each app's, keyed by app id. One loader for the turn and the footprint
    /// report, so the two count the same docs.
    pub fn workspace_knowhow_summaries(
        &self,
    ) -> (
        Vec<crate::core::KnowhowSummary>,
        Vec<(String, crate::core::KnowhowSummary)>,
    ) {
        let summaries = crate::core::KnowhowStore::load_merged_summaries(&self.knowhow_dirs());
        let apps_dir = self.workspace_path.join(crate::core::APPS_DIR);
        let app_summaries = crate::core::KnowhowStore::load_app_summaries(&apps_dir);
        (summaries, app_summaries)
    }

    pub fn user_dir(&self) -> Option<&std::path::Path> {
        self.user_dir.as_deref()
    }

    pub fn grants_dir(&self) -> PathBuf {
        crate::core::grants_dir(&self.workspace_path)
    }

    pub fn workspace_name(&self) -> String {
        self.workspace_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.workspace_path.to_string_lossy().to_string())
    }

    pub fn app_manager(&self) -> &AppManager {
        &self.app_manager
    }

    pub fn shared_app_manager(&self) -> Arc<AppManager> {
        self.app_manager.clone()
    }

    pub fn summary_tree(&self) -> &crate::engine::summary_tree::Runtime {
        &self.summary_tree
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// INVARIANT: The engine's hot copy of `artifacts/user_profile.md`. Exposed so the
    /// data API's write and delete routes can keep it coherent: they write the
    /// file through their own `ArtifactManager`, which has no way back to the
    /// engine that serves the profile to every chat turn.
    pub fn user_profile_cache(&self) -> &crate::engine::user_profile::UserProfileCache {
        &self.user_profile
    }

    pub async fn user_timezone(&self) -> String {
        self.user_timezone.read().await.clone()
    }

    pub async fn record_hardened(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
        head_sha: &str,
        tier: crate::engine::git_ops::HardenTier,
    ) -> Result<(), sqlx::Error> {
        crate::engine::git_ops::record_hardened(&self.pool, repo_root, branch_name, head_sha, tier)
            .await
    }

    pub async fn harden_marker(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> crate::engine::git_ops::HardenMarker {
        crate::engine::git_ops::harden_marker(&self.pool, repo_root, branch_name).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_planned(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
        kind: crate::engine::git_ops::PlanMarkerKind,
        plan_path: Option<&str>,
        reason: Option<&str>,
        files: &[String],
        head_sha: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        crate::engine::git_ops::record_planned(
            &self.pool,
            repo_root,
            branch_name,
            kind,
            plan_path,
            reason,
            files,
            head_sha,
        )
        .await
    }

    pub async fn plan_marker_state(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> crate::engine::git_ops::PlanMarkerState {
        crate::engine::git_ops::plan_marker_state(&self.pool, repo_root, branch_name).await
    }

    pub async fn plan_marker_snapshot(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> crate::engine::git_ops::PlanMarkerSnapshot {
        crate::engine::git_ops::plan_marker_snapshot(&self.pool, repo_root, branch_name).await
    }

    pub(crate) async fn plan_marker_files(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> Result<Vec<String>, String> {
        crate::engine::git_ops::plan_marker_files(&self.pool, repo_root, branch_name).await
    }

    pub async fn approve_plan(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
        caller_thread: Option<&str>,
        plan_only: bool,
    ) -> Result<
        Option<crate::engine::git_ops::ApprovedPlan>,
        crate::engine::git_ops::PlanApprovalError,
    > {
        if let Some(thread) = caller_thread.and_then(|id| uuid::Uuid::parse_str(id).ok()) {
            return crate::engine::plan_lifecycle::approve(
                &self.event_bus,
                thread,
                repo_root,
                branch_name,
                plan_only,
            )
            .await;
        }
        if plan_only {
            crate::engine::git_ops::approve_plan_for(
                &self.pool,
                repo_root,
                branch_name,
                caller_thread,
                true,
            )
            .await
        } else {
            crate::engine::git_ops::approve_plan(&self.pool, repo_root, branch_name, caller_thread)
                .await
        }
    }

    pub fn event_store(&self) -> &EventStore {
        &self.event_store
    }

    pub fn changes(&self) -> &crate::core::changes_projection::ChangesProjection {
        self.event_bus.changes_projection()
    }

    pub fn current_provider(&self) -> Arc<dyn crate::llm::LlmProvider> {
        self.llm
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn current_web_search(&self) -> Arc<crate::llm::WebSearchChain> {
        self.web_search
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn model_registry(&self) -> &crate::llm::ModelRegistry {
        &self.model_registry
    }

    pub async fn aux_call(
        &self,
        purpose: crate::engine::ContextPurpose,
    ) -> crate::engine::aux_purpose::AuxCall {
        crate::engine::aux_purpose::AuxCall::resolve(
            &self.pool,
            &self.model_registry,
            self.current_provider(),
            purpose,
        )
        .await
        .reporting_to(&self.event_bus, &self.pool, &self.model_registry, purpose)
    }

    pub async fn chat_model_and_window(&self) -> (String, usize) {
        let offer =
            crate::runtime::intelligence_ladder::ChatOffer::load(&self.pool, self.model_routes())
                .await;
        let model = crate::runtime::intelligence_ladder::default_chat_selection(&self.pool, &offer)
            .await
            .model;
        let window = self.context_window_for(&model, None);
        (model, window)
    }

    /// INVARIANT: Context window (tokens) for `model`: the window declared on the *route*
    /// that will serve it, else the id-shape guess in
    /// [`crate::llm::model_registry::context_window_from_prefix`].
    ///
    /// Every context-budget and `ContextCaptured` site goes through here rather
    /// than calling the prefix map directly — the prefix map has no rule for
    /// OpenRouter / xAI / Gemini / local ids and silently hands them 200k, which is
    /// what made the trim loop evict context at ~8% of kimi-k3's real 1M window.
    ///
    /// It resolves the route exactly as the router will: the turn's `chosen`
    /// provider, then the row's preference, then the first configured route.
    /// So a model served by two backends is budgeted for the one that answers.
    pub fn context_window_for(
        &self,
        model: &str,
        chosen: Option<crate::llm::ProviderKind>,
    ) -> usize {
        let configured = self.current_provider().configured_providers();
        crate::llm::model_registry::context_window_for(
            &self.model_registry,
            model,
            chosen,
            |kind| configured.as_ref().is_none_or(|set| set.contains(&kind)),
        )
    }

    /// INVARIANT: Input/output limits belong to the selected wire model, including a
    /// user-created alias whose registry identity has a different name.
    pub fn request_budget_model_for(
        &self,
        model: &str,
        chosen: Option<crate::llm::ProviderKind>,
    ) -> String {
        let configured = self.current_provider().configured_providers();
        crate::llm::model_registry::resolve_route(&self.model_registry, model, chosen, |kind| {
            configured.as_ref().is_none_or(|set| set.contains(&kind))
        })
        .map(|route| route.wire_id)
        .unwrap_or_else(|_| model.to_string())
    }

    pub fn llm_configured(&self) -> bool {
        self.current_provider().is_configured()
    }

    pub fn configured_providers(&self) -> Option<Vec<String>> {
        self.current_provider()
            .configured_providers()
            .map(|kinds| kinds.iter().map(|k| k.as_str().to_string()).collect())
    }

    pub fn configured_provider_kinds(&self) -> Vec<crate::llm::model_registry::ProviderKind> {
        self.current_provider()
            .configured_providers()
            .unwrap_or_else(|| crate::llm::model_registry::ProviderKind::ALL.to_vec())
    }

    pub fn model_routes(&self) -> crate::runtime::intelligence_ladder::ModelRoutes {
        let provider = self.current_provider();
        crate::runtime::intelligence_ladder::ModelRoutes {
            registry: self.model_registry().clone(),
            configured: provider
                .configured_providers()
                .unwrap_or_else(|| crate::llm::model_registry::ProviderKind::ALL.to_vec()),
            provider: Some(provider),
        }
    }

    /// INVARIANT: Run the one-time upgrade from the registry's `enabled` flag to the chat
    /// intelligence ladder. A boot step: it needs the registry loaded.
    pub async fn upgrade_chat_ladder_from_enabled(&self) {
        crate::runtime::chat_ladder::upgrade_from_enabled(
            self.pool(),
            &self.event_bus,
            &self.model_routes(),
        )
        .await;
    }
}

/// INVARIANT: Normalize an LLM-supplied path to a workspace `data/`-relative path, or reject
/// it. Pure string logic (no filesystem); callers join the result onto the
/// workspace `data/` dir. Rules:
///
/// - `..` / absolute paths are rejected (path traversal).
/// - A leading `data/` is stripped — LLMs often pass the full workspace-relative path.
/// - `.lucidos/…` is handled by [`normalize_lucidos_path`] BEFORE the untyped
///   default, because it names the ephemeral scratch tree outside `data/`.
/// - A data prefix ([`crate::core::data_prefixes`]) is kept as-is.
/// - An *untyped* bare path (no `data/` prefix) defaults under `artifacts/`, the
///   catch-all content store — so `write_file('report.md')` lands sensibly at
///   `artifacts/report.md`.
/// - But an explicit `data/<x>` whose `<x>` is not a typed subdirectory is
///   **rejected**: the `data/` root holds only the typed subdirs plus gitignored
///   config (`.env`, `postgres/`), and the file tools git-commit everything they
///   write. Silently routing `data/.env` to `artifacts/.env` would commit a
///   secret into the tracked artifacts repo. Loose data-root files are written
///   with `run_python` instead.
///
/// The data route reads the same list, so the file tools and the HTTP API
/// cannot disagree about where a folder's paths land.
fn normalize_data_path(relative_path: &str) -> Result<String, String> {
    if crate::core::is_path_traversal(relative_path) {
        return Err("Path traversal not allowed".to_string());
    }
    let had_data_prefix = relative_path.starts_with("data/");
    let stripped = relative_path.strip_prefix("data/").unwrap_or(relative_path);

    // INVARIANT: Must come before the untyped default below, which would otherwise route
    // `.lucidos/tmp/x` to `artifacts/.lucidos/tmp/x`. That is the same hazard as
    // the `data/<untyped>` refusal further down, one prefix over, except that
    // this one was live rather than hypothetical: it git-committed 94 scratch
    // files into tracked artifacts repos while the matching reads failed
    // against a path nobody had asked for.
    if stripped == ".lucidos" || stripped.starts_with(".lucidos/") {
        return normalize_lucidos_path(had_data_prefix, stripped);
    }

    if crate::core::is_known_data_prefix(stripped) {
        return Ok(stripped.to_string());
    }
    if had_data_prefix {
        return Err(format!(
            "'data/{stripped}' is not writable by file tools. The data/ root holds only typed \
             subdirectories ({}). For a loose data-root file like data/.env (gitignored config), \
             use run_python: open('data/.env', 'w'). For content, target a typed subdir, e.g. \
             artifacts/{stripped}.",
            crate::core::known_data_prefixes_text()
        ));
    }
    Ok(format!("artifacts/{stripped}"))
}

/// INVARIANT: Decide what a `.lucidos/…` path means to the file tools. `stripped` is the
/// path with any leading `data/` already removed, and `had_data_prefix` records
/// whether it carried one.
///
/// Exactly one subtree is addressable: [`crate::core::TMP_DIR`], the ephemeral
/// scratch the engine's own tools write into and then name back to the LLM
/// (`http_request(temp_path)` answers `[SAVED] .lucidos/tmp/<f>`, `git_clone`'s
/// tmp route answers `CLONED TO TMP: …` and tells the agent to extract from it
/// with `copy_file`). Reads land there; writes are refused one layer up in
/// `engine::tools::files::read_only_reason`, because the file tools commit
/// everything they write and this tree is gitignored.
///
/// Everything else under `.lucidos/` stays unaddressable in both directions:
/// `worktrees/` holds entire source checkouts, `exhaust/` is engine runtime
/// scratch, and `engine.pid` / `engine.lock` / `ports` / `cc-commands.json` are
/// process state.
/// None of it is "a file in the workspace" in the sense the tool advertises.
fn normalize_lucidos_path(had_data_prefix: bool, stripped: &str) -> Result<String, String> {
    let tmp = crate::core::TMP_DIR;
    if had_data_prefix {
        return Err(format!(
            "'data/{stripped}' does not exist: .lucidos/ sits beside data/ at the workspace \
             root, not inside it. Drop the prefix and use '{stripped}'."
        ));
    }
    if crate::core::is_tmp_path(stripped) && !stripped.ends_with('/') {
        return Ok(stripped.to_string());
    }
    Err(format!(
        "'{stripped}' is not addressable by the file tools. Under .lucidos/ only \
         '{tmp}/<file>' is (the ephemeral scratch http_request and git_clone write to); the \
         rest is engine runtime state. To CREATE a scratch file use run_python, whose cwd is \
         the workspace root: open('{tmp}/notes.json', 'w')."
    ))
}

fn symlink_escape(normalized: &str) -> String {
    format!("'{normalized}' resolves outside its tree through a symlink")
}

/// INVARIANT: Join an already-normalized [`crate::core::TMP_DIR`] path onto the workspace
/// root, refusing a target that escapes the scratch tree through a symlink.
///
/// `git_clone`'s tmp route drops whatever symlinks a cloned repository carries
/// into this directory, and `is_path_traversal` is a string check that cannot
/// see them. A missing target is not an escape, so the caller reports its
/// ordinary "file not found".
fn resolve_tmp_path(
    workspace_path: &std::path::Path,
    normalized: &str,
) -> Result<std::path::PathBuf, String> {
    let full_path = workspace_path.join(normalized);
    let tmp_root = workspace_path.join(crate::core::TMP_DIR);
    if crate::core::contained_path::resolves_outside(&tmp_root, &full_path) {
        return Err(format!(
            "'{}' resolves outside {}/ through a symlink",
            normalized,
            crate::core::TMP_DIR
        ));
    }
    Ok(full_path)
}

#[cfg(test)]
mod resolve_tmp_path_tests {
    use super::resolve_tmp_path;

    #[test]
    fn resolves_a_real_scratch_file() {
        let ws = tempfile::tempdir().unwrap();
        let tmp = ws.path().join(crate::core::TMP_DIR);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("readme.md"), "hi").unwrap();

        let got = resolve_tmp_path(ws.path(), ".lucidos/tmp/readme.md").unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap(), "hi");
    }

    #[test]
    fn refuses_a_symlink_that_escapes_the_scratch_tree() {
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "private").unwrap();

        let ws = tempfile::tempdir().unwrap();
        let tmp = ws.path().join(crate::core::TMP_DIR);
        std::fs::create_dir_all(&tmp).unwrap();
        std::os::unix::fs::symlink(&secret, tmp.join("escape.txt")).unwrap();

        let err = resolve_tmp_path(ws.path(), ".lucidos/tmp/escape.txt").unwrap_err();
        assert!(err.contains("symlink"), "got: {err}");
    }

    #[test]
    fn refuses_a_new_file_under_a_directory_link_out_of_the_scratch_tree() {
        let outside = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let tmp = ws.path().join(crate::core::TMP_DIR);
        std::fs::create_dir_all(&tmp).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.join("repo")).unwrap();

        let err = resolve_tmp_path(ws.path(), ".lucidos/tmp/repo/new.txt").unwrap_err();
        assert!(err.contains("symlink"), "got: {err}");
    }

    #[test]
    fn allows_a_symlink_that_stays_inside_the_scratch_tree() {
        let ws = tempfile::tempdir().unwrap();
        let tmp = ws.path().join(crate::core::TMP_DIR);
        std::fs::create_dir_all(tmp.join("repo")).unwrap();
        std::fs::write(tmp.join("repo/real.md"), "inside").unwrap();
        std::os::unix::fs::symlink(tmp.join("repo/real.md"), tmp.join("link.md")).unwrap();

        let got = resolve_tmp_path(ws.path(), ".lucidos/tmp/link.md").unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap(), "inside");
    }

    #[test]
    fn a_missing_file_is_not_reported_as_a_symlink_escape() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(crate::core::TMP_DIR)).unwrap();

        let got = resolve_tmp_path(ws.path(), ".lucidos/tmp/nope.md").unwrap();
        assert!(!got.exists());
    }

    #[test]
    fn an_absent_scratch_dir_is_not_reported_as_a_symlink_escape() {
        let ws = tempfile::tempdir().unwrap();
        let got = resolve_tmp_path(ws.path(), ".lucidos/tmp/nope.md").unwrap();
        assert!(!got.exists());
    }
}

#[cfg(test)]
mod normalize_data_path_tests {
    use super::normalize_data_path;

    #[test]
    fn keeps_typed_prefixes_as_is() {
        for p in [
            "artifacts/report.md",
            "apps/x/index.html",
            "knowhow/foo.md",
            "triggers/t.md",
            "scripts/shared/run.py",
            "config/apis.json",
            "auth-modules/m.wasm",
            "system-knowhow/best-practices.md",
        ] {
            assert_eq!(normalize_data_path(p).unwrap(), p);
        }
    }

    #[test]
    fn every_data_prefix_resolves_to_itself() {
        for prefix in crate::core::data_prefixes::known_data_prefixes() {
            let path = format!("{prefix}x.json");
            assert_eq!(normalize_data_path(&path).unwrap(), path);
            assert_eq!(normalize_data_path(&format!("data/{path}")).unwrap(), path);
        }
        assert_eq!(
            normalize_data_path("themes/phosphor.json").unwrap(),
            "themes/phosphor.json"
        );
    }

    #[test]
    fn strips_leading_data_prefix_for_typed_paths() {
        assert_eq!(
            normalize_data_path("data/artifacts/report.md").unwrap(),
            "artifacts/report.md"
        );
        assert_eq!(
            normalize_data_path("data/knowhow/foo.md").unwrap(),
            "knowhow/foo.md"
        );
        assert_eq!(
            normalize_data_path("data/scripts/shared/run.py").unwrap(),
            "scripts/shared/run.py"
        );
    }

    #[test]
    fn untyped_bare_path_defaults_under_artifacts() {
        assert_eq!(
            normalize_data_path("report.md").unwrap(),
            "artifacts/report.md"
        );
        assert_eq!(
            normalize_data_path("research/notes.md").unwrap(),
            "artifacts/research/notes.md"
        );
    }

    #[test]
    fn explicit_data_root_untyped_path_is_rejected() {
        let err = normalize_data_path("data/.env").unwrap_err();
        assert!(err.contains("not writable"), "got: {err}");
        assert!(err.contains("run_python"), "got: {err}");

        assert!(normalize_data_path("data/postgres/x").is_err());
        assert!(normalize_data_path("data/report.md").is_err());
        assert!(normalize_data_path("data/").is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(normalize_data_path("../secret").is_err());
        assert!(normalize_data_path("/etc/passwd").is_err());
        assert!(normalize_data_path("artifacts/../../escape").is_err());
    }

    #[test]
    fn tmp_scratch_paths_survive_normalization() {
        for p in [
            ".lucidos/tmp/t3code_readme.md",
            ".lucidos/tmp/oura_data.json",
            ".lucidos/tmp/some-repo/README.md",
            ".lucidos/tmp/plugins/uploads/abc/x.lucidos-plugin",
        ] {
            assert_eq!(normalize_data_path(p).unwrap(), p, "path {p}");
        }
    }

    #[test]
    fn no_lucidos_path_can_reach_artifacts() {
        for p in [
            ".lucidos",
            ".lucidos/",
            ".lucidos/tmp",
            ".lucidos/tmp/",
            ".lucidos/tmp/x.md",
            ".lucidos/exhaust/y",
            ".lucidos/worktrees/thread-x/src/main.rs",
            ".lucidos/engine.pid",
            ".lucidos/engine.lock",
            "data/.lucidos/tmp/x.md",
        ] {
            match normalize_data_path(p) {
                Ok(n) => assert!(!n.starts_with("artifacts/"), "{p} normalized to {n}"),
                Err(e) => assert!(!e.contains("artifacts/.lucidos"), "{p} suggested {e}"),
            }
        }
    }

    #[test]
    fn only_the_tmp_subtree_is_addressable() {
        for p in [
            ".lucidos/worktrees/thread-x/src/main.rs",
            ".lucidos/exhaust/run.log",
            ".lucidos/engine.pid",
            ".lucidos/engine.lock",
            ".lucidos/ports",
            ".lucidos/cc-commands.json",
        ] {
            let err = normalize_data_path(p).unwrap_err();
            assert!(err.contains("not addressable"), "{p} got: {err}");
            assert!(err.contains(".lucidos/tmp/<file>"), "{p} got: {err}");
        }
    }

    #[test]
    fn tmp_directory_itself_is_not_a_file() {
        for p in [".lucidos", ".lucidos/", ".lucidos/tmp", ".lucidos/tmp/"] {
            assert!(normalize_data_path(p).is_err(), "{p} should be refused");
        }
    }

    #[test]
    fn data_prefixed_lucidos_path_is_corrected_not_rerouted() {
        let err = normalize_data_path("data/.lucidos/tmp/x.json").unwrap_err();
        assert!(err.contains("beside data/"), "got: {err}");
        assert!(err.contains("'.lucidos/tmp/x.json'"), "got: {err}");
    }

    #[test]
    fn refusals_name_the_route_that_works() {
        let err = normalize_data_path(".lucidos/exhaust/y").unwrap_err();
        assert!(err.contains("run_python"), "got: {err}");
    }

    #[test]
    fn a_dotfile_that_merely_starts_with_lucidos_is_not_scratch() {
        assert_eq!(
            normalize_data_path(".lucidosrc").unwrap(),
            "artifacts/.lucidosrc"
        );
    }

    #[test]
    fn the_file_tools_cannot_address_a_permission_grant_file() {
        for file in crate::core::GrantFile::ALL {
            for path in [
                format!(".lucidos/{}", file.file_name()),
                format!("data/.lucidos/{}", file.file_name()),
            ] {
                assert!(
                    normalize_data_path(&path).is_err(),
                    "{path} must be unreachable: a permission file the agent can \
                     rewrite is not a permission file"
                );
            }
        }
    }
}

impl LucidosEngine {
    pub async fn change_claim_holder(
        &self,
        thread_id: Uuid,
    ) -> Option<crate::engine::types::ChangeClaim> {
        self.agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .and_then(|s| s.change_claim)
    }

    pub(crate) async fn clear_change_claim(
        &self,
        thread_id: Uuid,
        claimant: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) {
        crate::engine::release_change_claim(
            &mut *self.agent_sessions.lock().await,
            thread_id,
            claimant,
        );
    }

    pub async fn thread_coding_agent(&self, thread_id: Uuid) -> crate::runtime::CodingAgent {
        let stored: Option<String> =
            sqlx::query_scalar("SELECT coding_agent FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or_else(|e| {
                    crate::log_legacy!(
                "[AgentSession] thread_coding_agent({}) DB error: {} — defaulting to ClaudeCode",
                thread_id,
                e
            );
                    None
                })
                .flatten();
        stored
            .map(|s| crate::runtime::CodingAgent::parse(&s))
            .unwrap_or(crate::runtime::CodingAgent::ClaudeCode)
    }

    pub async fn cc_thread_settings(&self, thread_id: Uuid) -> (Option<String>, Option<String>) {
        self.cc_thread_settings_where(thread_id, None).await
    }

    pub async fn cc_thread_settings_on(
        &self,
        thread_id: Uuid,
        coding_agent: crate::runtime::CodingAgent,
    ) -> (Option<String>, Option<String>) {
        self.cc_thread_settings_where(thread_id, Some(coding_agent))
            .await
    }

    async fn cc_thread_settings_where(
        &self,
        thread_id: Uuid,
        coding_agent: Option<crate::runtime::CodingAgent>,
    ) -> (Option<String>, Option<String>) {
        let tid = thread_id.to_string();
        let rows: Vec<(serde_json::Value,)> = match sqlx::query_as(
            "SELECT payload FROM events \
             WHERE aggregate_id = $1 AND event_type = 'CodingAgentSettingsChanged' \
               AND ($2::text IS NULL \
                    OR COALESCE(payload->>'coding_agent', payload->>'agent', 'claude-code') = $2) \
             ORDER BY created DESC LIMIT 10",
        )
        .bind(&tid)
        .bind(coding_agent.map(crate::runtime::CodingAgent::as_str))
        .fetch_all(self.pool())
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log_legacy!(
                    "[ClaudeCode] cc_thread_settings({}) DB error: {} — falling back to no per-thread overrides",
                    thread_id,
                    e
                );
                Vec::new()
            }
        };
        let mut model: Option<String> = None;
        let mut effort: Option<String> = None;
        for (payload,) in &rows {
            if model.is_none() {
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    model = Some(m.to_string());
                }
            }
            if effort.is_none() {
                if let Some(e) = payload.get("reasoning_effort").and_then(|v| v.as_str()) {
                    effort = Some(e.to_string());
                }
            }
            if model.is_some() && effort.is_some() {
                break;
            }
        }
        (model, effort)
    }

    pub async fn coding_agent_rung(
        &self,
        thread_id: Uuid,
        coding_agent: crate::runtime::CodingAgent,
        requested_model: Option<String>,
        requested_effort: Option<String>,
    ) -> (Option<String>, Option<String>) {
        let (requested_model, requested_effort) =
            if requested_model.is_none() && requested_effort.is_none() {
                self.pending_handoff_rung(thread_id, coding_agent).await
            } else {
                (requested_model, requested_effort)
            };
        let (event_model, event_effort) = if requested_model.is_none() || requested_effort.is_none()
        {
            self.cc_thread_settings_on(thread_id, coding_agent).await
        } else {
            (None, None)
        };
        let default_implementer =
            crate::runtime::intelligence_ladder::default_implementer(self.pool(), coding_agent)
                .await;
        crate::runtime::intelligence_ladder::fill_pick(
            coding_agent,
            requested_model.or(event_model),
            requested_effort.or(event_effort),
            default_implementer.as_ref(),
            |model| {
                crate::runtime::intelligence_ladder::model_default_effort(
                    coding_agent,
                    self.model_registry(),
                    model,
                )
            },
        )
    }

    pub async fn swap_fallback_effort(
        &self,
        coding_agent: crate::runtime::CodingAgent,
        served: &str,
    ) -> Option<String> {
        let default_implementer =
            crate::runtime::intelligence_ladder::default_implementer(self.pool(), coding_agent)
                .await;
        crate::runtime::intelligence_ladder::fallback_effort(
            coding_agent,
            served,
            default_implementer.as_ref(),
            |model| {
                crate::runtime::intelligence_ladder::model_default_effort(
                    coding_agent,
                    self.model_registry(),
                    model,
                )
            },
        )
    }

    /// INVARIANT: The model and effort a pending handoff names for `thread_id`'s
    /// successor session, when it runs on `coding_agent`. A user's message may
    /// start that session before the engine does.
    async fn pending_handoff_rung(
        &self,
        thread_id: Uuid,
        coding_agent: crate::runtime::CodingAgent,
    ) -> (Option<String>, Option<String>) {
        match crate::engine::pending_handoff::pending_handoff(self.pool(), thread_id).await {
            Ok(Some(handoff)) if handoff.coding_agent == coding_agent => {
                (handoff.coding_agent_model, handoff.reasoning_effort)
            }
            Ok(_) => (None, None),
            Err(e) => {
                log_legacy!("[ClaudeCode] pending handoff of {}: {}", thread_id, e);
                (None, None)
            }
        }
    }

    /// INVARIANT: The model and effort a coding-agent thread runs on. The model is its
    /// latest setting. The effort is its *effort in force*, never a pick no
    /// call ran at.
    pub async fn cc_thread_runs_on(
        &self,
        thread_id: Uuid,
    ) -> Result<(Option<String>, Option<String>), sqlx::Error> {
        let (model, _) = self.cc_thread_settings(thread_id).await;
        let effort =
            crate::engine::handoff_estimate::thread_effort_in_force(self.pool(), thread_id).await?;
        Ok((model, effort))
    }
}

impl LucidosEngine {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_notification(
        &self,
        title: &str,
        message: &str,
        app_id: Option<&str>,
        link_thread_id: Option<uuid::Uuid>,
        link_event_id: Option<uuid::Uuid>,
        mut tap: crate::scheduler::notifications::Tap,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error + Send + Sync>> {
        if title.trim().is_empty() {
            return Err("title is required".into());
        }
        if message.trim().is_empty() {
            return Err("message is required".into());
        }
        // INVARIANT: Belt-and-braces, like the two guards above. The surfaces that take a
        // caller-written tap already settled it. So this is a no-op for them,
        // and for every engine-internal producer building one from typed uuids.
        // What it buys is that a FUTURE producer cannot write a thread tap the
        // page is unable to resolve. `None`: this is a chokepoint, not a turn,
        // so there is no ambient thread an alias could mean here.
        crate::scheduler::notifications::resolve_thread_tap_id(&mut tap, None)?;

        let notification_id = uuid::Uuid::new_v4();
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::NotificationCreated {
                    id: notification_id.to_string(),
                    title: title.to_string(),
                    message: message.to_string(),
                    task_id: None,
                    app_id: app_id.map(str::to_string),
                    thread_id: link_thread_id.map(|t| t.to_string()),
                    event_id: link_event_id.map(|e| e.to_string()),
                    tap: tap.clone(),
                    actor,
                    audience: None,
                    push_group: None,
                },
            ))
            .await
            .map_err(|e| format!("failed to create notification: {}", e))?;

        crate::scheduler::push::send_push_to_all_with_app(
            &self.clone_arc(),
            title,
            message,
            Some(notification_id),
            app_id,
            link_thread_id,
            link_event_id,
            tap,
        );

        Ok(notification_id)
    }
}
