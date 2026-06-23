//! Accessors: workspace/repo paths, pools, caches, knowhow dirs, hardening markers.
//!
//! Part of the `LucidosEngine` inherent impl, split from engine_impl.rs.

use super::super::*;

impl LucidosEngine {
    /// Update a per-repo entry in cc_commands_cache and persist to disk if changed.
    pub(crate) async fn upsert_cc_commands_cache(&self, repo_key: String, info: CcCommandsInfo) {
        let json = {
            let mut cache = self.cc_commands_cache.write().await;
            if cache.get(&repo_key) == Some(&info) {
                return;
            }
            cache.insert(repo_key, info);
            match serde_json::to_string(&*cache) {
                Ok(json) => Some(json),
                Err(e) => {
                    log!("[ClaudeCode] Failed to serialize CC commands cache: {}", e);
                    None
                }
            }
        };
        if let Some(json) = json {
            let path = self.workspace_path.join(".lucidos/cc-commands.json");
            if let Err(e) = std::fs::write(&path, &json) {
                log!("[ClaudeCode] Failed to write CC commands cache: {}", e);
            }
        }
    }

    /// Resolve a data-relative path, returning both the normalized data-relative path and
    /// the absolute filesystem path. Normalization rules live in [`normalize_data_path`].
    pub(crate) fn resolve_data_path(
        &self,
        relative_path: &str,
    ) -> Result<(String, std::path::PathBuf), String> {
        let normalized = normalize_data_path(relative_path)?;

        // System knowhow lives in the engine repo, not the workspace.
        if let Some(rel) = normalized.strip_prefix("system-knowhow/") {
            let dir = self
                .system_knowhow_dir
                .as_deref()
                .ok_or_else(|| "System knowhow is not available".to_string())?;
            return Ok((normalized.clone(), dir.join(rel)));
        }

        let full_path = self.workspace_path.join("data").join(&normalized);

        // For knowhow paths: if file doesn't exist locally, fall back to shared.
        if normalized.starts_with("knowhow/") && !full_path.exists() {
            let kh_relative = normalized.strip_prefix("knowhow/").unwrap();
            if let Some(shared_dir) = self.shared_knowhow_dir() {
                let shared_path = shared_dir.join(kh_relative);
                if shared_path.exists() {
                    return Ok((normalized, shared_path));
                }
            }
        }

        Ok((normalized, full_path))
    }

    /// Set the self-reference after wrapping in Arc. Must be called once after Arc::new.
    pub fn set_self_arc(&self, arc: &Arc<LucidosEngine>) {
        self.self_arc.set(Arc::downgrade(arc)).ok();
        // The Thread Queue executes admitted entries through the engine —
        // wire it the moment the Arc exists so no submission can race a
        // missing executor.
        self.thread_queue.attach_engine(arc);
    }

    /// Clone the Arc<Self> for spawning background tasks.
    pub(crate) fn clone_arc(&self) -> Arc<LucidosEngine> {
        self.self_arc
            .get()
            .expect("self_arc not initialized")
            .upgrade()
            .expect("engine dropped while in use")
    }

    /// Get the workspace path
    pub fn workspace_path(&self) -> &std::path::Path {
        &self.workspace_path
    }

    /// Locate the on-disk worktree path for an app coding-agent thread, so
    /// the WIP-preview app-UI route can stream files from the worktree
    /// instead of from the live workspace copy. Returns `None` when no live
    /// agent session is recorded for `thread_id` or when its worktree dir
    /// has been removed (Apply / Discard cleanup) — caller serves a 404.
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
        // Fall back to the central resume resolver — works after the live
        // session has exited but before any cleanup pass removes the dir
        // (Apply leaves the worktree on disk and resets it to main).
        let resolved = super::super::agent_session::resume::resolve_worktree_path(
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

    /// Hold across any write+commit on the workspace repo, so
    /// `change_ops::apply_change`'s dirty check (which also holds it) never
    /// observes a half-written file from a commit-in-flight.
    pub(crate) async fn lock_workspace_repo(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.workspace_repo_lock.lock().await
    }

    /// Get the Lucidos source repo root (resolved at startup).
    pub fn repo_root(&self) -> &std::path::Path {
        &self.repo_root
    }

    /// Shared `script_handshake` token cache. Both the HTTP proxy and the
    /// `proxy_request` LLM tool use this so the handshake script runs once
    /// per expiry window across all callers.
    pub fn proxy_token_cache(&self) -> &crate::api::proxy_token_cache::ProxyTokenCache {
        &self.proxy_token_cache
    }

    /// Shared token cache as a clonable `Arc`. Used by the pipeline
    /// builder to hand `ScriptHandshakeLayer` a handle on the same cache
    /// every other caller uses.
    pub fn proxy_token_cache_arc(&self) -> Arc<crate::api::proxy_token_cache::ProxyTokenCache> {
        self.proxy_token_cache.clone()
    }

    /// Compiled WASM signer modules registry. The Phase-9 reload endpoint
    /// writes the lock; pipeline builds clone the `Arc<CompiledModule>`
    /// out and finish the request — in-flight calls keep their old `Arc`
    /// while new requests see the new map.
    pub fn proxy_modules(
        &self,
    ) -> &Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, Arc<crate::api::proxy_wasm_signer::CompiledModule>>,
        >,
    > {
        &self.proxy_modules
    }

    /// Shared wasmtime engine. Use this for any module compilation or
    /// instantiation — wasmtime forbids cross-engine instantiation.
    pub fn wasm_engine(&self) -> &Arc<wasmtime::Engine> {
        &self.wasm_engine
    }

    /// Get the shared knowhow directory (~/.lucidos/knowhow), if available
    pub fn shared_knowhow_dir(&self) -> Option<PathBuf> {
        self.user_dir.as_ref().map(|ud| ud.join("knowhow"))
    }

    /// Get the engine-shipped system knowhow directory (`<repo_root>/system-knowhow/`).
    pub fn system_knowhow_dir(&self) -> Option<&std::path::Path> {
        self.system_knowhow_dir.as_deref()
    }

    /// Bundle the user-curated knowhow search directories (shared + local + apps + triggers).
    /// `apps` enables app-scoped id resolution (`<app_id>/<rest>` →
    /// `data/apps/<app_id>/knowhow/<rest>.md`) for the validator and loader.
    /// `triggers` enables trigger-scoped id resolution (`triggers/<slug>/<rest>` →
    /// `data/triggers/<slug>/knowhow/<rest>.md`); the leading `triggers/`
    /// prefix disambiguates from the bare `<app>/<rest>` namespace.
    /// System knowhow is loaded separately via [`crate::core::SystemKnowhowStore`].
    pub fn knowhow_dirs(&self) -> crate::core::knowhow::KnowhowDirs {
        crate::core::knowhow::KnowhowDirs {
            shared: self.shared_knowhow_dir(),
            local: self.workspace_path.join(crate::core::KNOWHOW_DIR),
            apps: Some(self.workspace_path.join(crate::core::APPS_DIR)),
            triggers: Some(self.workspace_path.join(crate::core::TRIGGERS_DIR)),
        }
    }

    /// Get the user-level Lucidos directory (~/.lucidos), if available
    pub fn user_dir(&self) -> Option<&std::path::Path> {
        self.user_dir.as_deref()
    }

    /// Get a human-readable workspace name (last path component, or full path if root)
    pub fn workspace_name(&self) -> String {
        self.workspace_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.workspace_path.to_string_lossy().to_string())
    }

    /// Get reference to the app manager
    pub fn app_manager(&self) -> &AppManager {
        &self.app_manager
    }

    /// Get the shared database connection pool
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    /// Record that `(repo_root, branch_name)` has been hardened at `head_sha`.
    /// Called by the `/api/v1/internal/mark-hardened` endpoint that the
    /// `mark-harden.sh` hook hits via `lucidos hardened mark`.
    pub async fn record_hardened(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
        head_sha: &str,
    ) -> Result<(), sqlx::Error> {
        crate::engine::git_ops::record_hardened(&self.pool, repo_root, branch_name, head_sha).await
    }

    pub(crate) async fn harden_marker_state(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> crate::engine::git_ops::HardenMarkerState {
        crate::engine::git_ops::harden_marker_state(&self.pool, repo_root, branch_name).await
    }

    /// Record a Planned marker (the `implementation-plan` enforcement floor).
    /// Called by the `/api/v1/internal/mark-planned` endpoint that the
    /// `lucidos planned mark` CLI hits.
    pub(crate) async fn record_planned(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
        kind: crate::engine::git_ops::PlanMarkerKind,
        plan_path: Option<&str>,
        reason: Option<&str>,
        head_sha: &str,
    ) -> Result<(), sqlx::Error> {
        crate::engine::git_ops::record_planned(
            &self.pool,
            repo_root,
            branch_name,
            kind,
            plan_path,
            reason,
            head_sha,
        )
        .await
    }

    pub(crate) async fn plan_marker_state(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> crate::engine::git_ops::PlanMarkerState {
        crate::engine::git_ops::plan_marker_state(&self.pool, repo_root, branch_name).await
    }

    /// Approve a `Proposed` plan, flipping it to `Planned` so the gate passes.
    /// Returns whether a proposed row was flipped. Called by the
    /// `/api/v1/internal/approve-plan` endpoint that `lucidos planned approve`
    /// hits after the user approves the plan in chat.
    pub(crate) async fn approve_plan(
        &self,
        repo_root: &std::path::Path,
        branch_name: &str,
    ) -> Result<bool, sqlx::Error> {
        crate::engine::git_ops::approve_plan(&self.pool, repo_root, branch_name).await
    }

    /// Borrow the event store for sharing with read-only handlers.
    pub fn event_store(&self) -> &EventStore {
        &self.event_store
    }

    /// In-memory event-sourced projection of changes (pending + applied +
    /// discarded + reverted). Backed by the EventBus emit path; rebuilt on
    /// startup from the events table.
    pub fn changes(&self) -> &crate::core::changes_projection::ChangesProjection {
        self.event_bus.changes_projection()
    }

    /// The currently-installed LLM provider. Clones the inner `Arc` out under a
    /// short read guard so callers can `.await` on it without holding the lock —
    /// the credential subscriber may swap the handle at any time, and a chat
    /// path pins this one `Arc` for the whole response. Poison-tolerant: a
    /// panicked writer (the subscriber) can't wedge reads.
    pub fn current_provider(&self) -> Arc<dyn crate::llm::LlmProvider> {
        self.llm
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Whether the installed LLM provider can actually serve calls. `false` only
    /// when the engine has no provider configured (the `UnconfiguredProvider`
    /// sentinel — packaged first run, before a credential is added). Reflects a
    /// runtime swap. Surfaced by `/health` as `llm_configured` so the frontend
    /// shows provider onboarding instead of letting the user chat into a
    /// guaranteed error.
    pub fn llm_configured(&self) -> bool {
        self.current_provider().is_configured()
    }

    /// Which provider backends are actually configured (`vertex`/`anthropic`/
    /// `openai`/`openrouter`/`local`), or `None` to mean "don't filter" (mock /
    /// no routing). Reflects a runtime swap (reads the live provider). Surfaced
    /// by `/health` as `configured_providers` so the frontend filters the model
    /// picker to providers the user has set up.
    pub fn configured_providers(&self) -> Option<Vec<String>> {
        self.current_provider()
            .configured_providers()
            .map(|kinds| kinds.iter().map(|k| k.as_str().to_string()).collect())
    }
}

/// Typed `data/` subdirectories the file tools write into. Anything else under
/// the `data/` root (`.env`, `postgres/`, …) is gitignored config the tools must
/// not touch — see [`normalize_data_path`].
const KNOWN_DATA_PREFIXES: [&str; 8] = [
    "artifacts/",
    "apps/",
    "knowhow/",
    "triggers/",
    "scripts/",
    "config/",
    "auth-modules/",
    "system-knowhow/",
];

/// Normalize an LLM-supplied path to a workspace `data/`-relative path, or reject
/// it. Pure string logic (no filesystem); callers join the result onto the
/// workspace `data/` dir. Rules:
///
/// - `..` / absolute paths are rejected (path traversal).
/// - A leading `data/` is stripped — LLMs often pass the full workspace-relative path.
/// - A known typed prefix ([`KNOWN_DATA_PREFIXES`]) is kept as-is.
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
/// The typed set matches `api/data_api.rs`'s `MUTABLE_PREFIXES` (the HTTP data
/// surface) plus `system-knowhow/` (engine-repo, read-only): both must recognize
/// the same `data/` subdirs or the file tools and the HTTP API disagree on where
/// `scripts/` etc. land.
fn normalize_data_path(relative_path: &str) -> Result<String, String> {
    if crate::api::is_path_traversal(relative_path) {
        return Err("Path traversal not allowed".to_string());
    }
    let had_data_prefix = relative_path.starts_with("data/");
    let stripped = relative_path.strip_prefix("data/").unwrap_or(relative_path);

    if KNOWN_DATA_PREFIXES.iter().any(|p| stripped.starts_with(p)) {
        return Ok(stripped.to_string());
    }
    if had_data_prefix {
        return Err(format!(
            "'data/{stripped}' is not writable by file tools — the data/ root holds only typed \
             subdirectories (artifacts/, apps/, knowhow/, triggers/, scripts/, config/, auth-modules/). \
             For a loose data-root file like data/.env (gitignored config), use run_python: \
             open('data/.env', 'w'). For content, target a typed subdir, e.g. artifacts/{stripped}."
        ));
    }
    Ok(format!("artifacts/{stripped}"))
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
    fn strips_leading_data_prefix_for_typed_paths() {
        assert_eq!(
            normalize_data_path("data/artifacts/report.md").unwrap(),
            "artifacts/report.md"
        );
        assert_eq!(
            normalize_data_path("data/knowhow/foo.md").unwrap(),
            "knowhow/foo.md"
        );
        // Top-level shared scripts are a typed subdir, so the explicit
        // data/scripts/ form succeeds rather than misrouting to artifacts/.
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
        // The footgun: data/.env must NOT silently become artifacts/.env.
        let err = normalize_data_path("data/.env").unwrap_err();
        assert!(err.contains("not writable"), "got: {err}");
        assert!(err.contains("run_python"), "got: {err}");

        // Any non-typed data-root target is refused, not just .env.
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
}
