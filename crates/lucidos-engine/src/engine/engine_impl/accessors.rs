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
    /// the absolute filesystem path. Paths without a known prefix are assumed to be under artifacts/.
    pub(crate) fn resolve_data_path(
        &self,
        relative_path: &str,
    ) -> Result<(String, std::path::PathBuf), String> {
        if crate::api::is_path_traversal(relative_path) {
            return Err("Path traversal not allowed".to_string());
        }
        // Strip leading "data/" if the LLM included the full workspace-relative path
        let relative_path = relative_path.strip_prefix("data/").unwrap_or(relative_path);
        let known_prefixes = [
            "artifacts/",
            "apps/",
            "knowhow/",
            "triggers/",
            "config/",
            "auth-modules/",
            "system-knowhow/",
        ];
        let normalized = if known_prefixes.iter().any(|p| relative_path.starts_with(p)) {
            relative_path.to_string()
        } else {
            format!("artifacts/{}", relative_path)
        };

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

}
