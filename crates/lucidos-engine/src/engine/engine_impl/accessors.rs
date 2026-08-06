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

    /// The Google Cloud project id resolved at boot for Vertex AI (env
    /// `VERTEX_PROJECT_ID` → ADC file → gcloud config). Empty when Vertex is
    /// not configured. Reused by the builtin `vertex` proxy so an app can reach
    /// Vertex without knowing the project id (`api::proxy_builtin`).
    pub fn vertex_project_id(&self) -> &str {
        &self.vertex_project_id
    }

    /// Live Vertex AI region handle (tracks the `vertex_region` preference via
    /// `spawn_vertex_region_subscriber`). Reused by the builtin `vertex` proxy
    /// to build the engine-owned URL prefix.
    pub fn vertex_location(&self) -> &crate::llm::vertex::LocationHandle {
        &self.vertex_location
    }

    /// The shared Vertex access-token cache (present iff Vertex was configured
    /// at boot). Reused by the builtin `vertex` proxy so proxied requests share
    /// warm access tokens with the Vertex LLM provider.
    pub fn vertex_token_cache(&self) -> Option<crate::llm::vertex::TokenCache> {
        self.vertex_token_cache.clone()
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

        // Ephemeral scratch sits beside `data/` at the workspace root, so it
        // joins onto the root rather than onto `data/`. [`normalize_data_path`]
        // has already narrowed `.lucidos/` to the readable tmp subtree.
        if crate::core::is_tmp_path(&normalized) {
            let full_path = resolve_tmp_path(&self.workspace_path, &normalized)?;
            return Ok((normalized, full_path));
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

    /// `Arc<Self>` when the self-reference has been installed (`set_self_arc`,
    /// done once in `main.rs`), else `None`. Unlike [`Self::clone_arc`] this
    /// never panics — for `&self` side effects that only make sense on a fully
    /// wired engine (the dev background rebuild) and must simply no-op in unit
    /// tests that construct a bare `LucidosEngine`.
    pub(crate) fn try_clone_arc(&self) -> Option<Arc<LucidosEngine>> {
        self.self_arc.get().and_then(std::sync::Weak::upgrade)
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

    /// Get the engine-shipped system knowhow directory (the staged
    /// `LUCIDOS_SYSTEM_KNOWHOW_DIR` on packaged builds, `<repo_root>/system-knowhow/`
    /// on a dev checkout).
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

    /// The user's IANA timezone (e.g. "Europe/Oslo"), loaded at construction
    /// from the `timezone` preference and refreshed by the `Timezone` preference
    /// side-effect. Empty when unset. Read by the scheduler to register the
    /// backup cron in the user's timezone (the same way triggers schedule).
    pub(crate) async fn user_timezone(&self) -> String {
        self.user_timezone.read().await.clone()
    }

    /// Record that `(repo_root, branch_name)` has been hardened at `head_sha`.
    /// Called by the `/api/v1/internal/mark-hardened` endpoint that
    /// `lucidos hardened mark` hits from `/harden` Phase 5.
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

    /// The currently-installed web-search chain. Same short-read-guard clone
    /// contract as [`Self::current_provider`] — the credential subscriber swaps
    /// this handle too, and a search must not hold the lock across its `.await`.
    pub fn current_web_search(&self) -> Arc<crate::llm::WebSearchChain> {
        self.web_search
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Context window (tokens) for `model`: the window declared on its `models`
    /// registry row, else the id-shape guess in
    /// [`crate::engine::context::context_window_from_prefix`].
    ///
    /// Every context-budget and `ContextCaptured` site goes through here rather
    /// than calling the prefix map directly — the prefix map has no rule for
    /// OpenRouter / Gemini / local ids and silently hands them 200k, which is
    /// what made the trim loop evict context at ~8% of kimi-k3's real 1M window.
    pub(crate) fn context_window_for(&self, model: &str) -> usize {
        crate::llm::model_registry::context_window_for(&self.model_registry, model)
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
/// - `.lucidos/…` is handled by [`normalize_lucidos_path`] BEFORE the untyped
///   default, because it names the ephemeral scratch tree outside `data/`.
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

    // Must come before the untyped default below, which would otherwise route
    // `.lucidos/tmp/x` to `artifacts/.lucidos/tmp/x`. That is the same hazard as
    // the `data/<untyped>` refusal further down, one prefix over, except that
    // this one was live rather than hypothetical: it git-committed 94 scratch
    // files into tracked artifacts repos while the matching reads failed
    // against a path nobody had asked for.
    if stripped == ".lucidos" || stripped.starts_with(".lucidos/") {
        return normalize_lucidos_path(had_data_prefix, stripped);
    }

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

/// Decide what a `.lucidos/…` path means to the file tools. `stripped` is the
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
/// scratch, and `engine.pid` / `ports` / `cc-commands.json` are process state.
/// None of it is "a file in the workspace" in the sense the tool advertises.
fn normalize_lucidos_path(had_data_prefix: bool, stripped: &str) -> Result<String, String> {
    let tmp = crate::core::TMP_DIR;
    if had_data_prefix {
        return Err(format!(
            "'data/{stripped}' does not exist: .lucidos/ sits beside data/ at the workspace \
             root, not inside it. Drop the prefix and use '{stripped}'."
        ));
    }
    // `is_tmp_path` anchors on the separator; the extra emptiness check rejects
    // a bare `.lucidos/tmp/`, which names the directory rather than a file.
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

/// Join an already-normalized [`crate::core::TMP_DIR`] path onto the workspace
/// root, refusing a target that escapes the scratch tree through a symlink.
///
/// `git_clone`'s tmp route drops whatever symlinks a cloned repository carries
/// into this directory, and `is_path_traversal` is a string check that cannot
/// see them, so string validation alone does not bound where a read lands.
/// Canonicalizing both sides does.
///
/// Only a PROVEN escape refuses. When the target does not exist,
/// `canonicalize` fails and the path is returned unchanged, so the caller
/// reports its ordinary "file not found" rather than a misleading security
/// error. Both sides are canonicalized because the workspace root itself may
/// sit behind a symlink, in which case comparing a canonical target against a
/// non-canonical root would reject every legitimate read.
fn resolve_tmp_path(
    workspace_path: &std::path::Path,
    normalized: &str,
) -> Result<std::path::PathBuf, String> {
    let full_path = workspace_path.join(normalized);
    let tmp_root = workspace_path.join(crate::core::TMP_DIR);
    if let (Ok(root), Ok(target)) = (tmp_root.canonicalize(), full_path.canonicalize()) {
        if !target.starts_with(&root) {
            return Err(format!(
                "'{}' resolves outside {}/ through a symlink",
                normalized,
                crate::core::TMP_DIR
            ));
        }
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
        // The `git_clone` hazard: a cloned repo carries a symlink pointing out
        // of the workspace entirely.
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
        // Canonicalization fails for a path that isn't there; the caller must
        // still get its plain "file not found", not a security error.
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(crate::core::TMP_DIR)).unwrap();

        let got = resolve_tmp_path(ws.path(), ".lucidos/tmp/nope.md").unwrap();
        assert!(!got.exists());
    }

    #[test]
    fn an_absent_scratch_dir_is_not_reported_as_a_symlink_escape() {
        // A workspace that has never used scratch has no .lucidos/tmp at all,
        // so the ROOT is what fails to canonicalize. Still not an escape.
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

    // ── .lucidos/ (ephemeral scratch, outside data/) ─────────────────────────

    #[test]
    fn tmp_scratch_paths_survive_normalization() {
        // The regression: these used to come back as `artifacts/.lucidos/…`,
        // so a read of a path `http_request` had just printed missed, and a
        // write git-committed scratch into the tracked artifacts repo.
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
        // Whether it resolves or is refused, no `.lucidos/` input may ever
        // produce a data/-relative path. That rewrite is the whole bug.
        for p in [
            ".lucidos",
            ".lucidos/",
            ".lucidos/tmp",
            ".lucidos/tmp/",
            ".lucidos/tmp/x.md",
            ".lucidos/exhaust/y",
            ".lucidos/worktrees/thread-x/src/main.rs",
            ".lucidos/engine.pid",
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
        // Everything else under .lucidos/ is engine runtime state: worktrees
        // hold entire source checkouts, exhaust/ is internal scratch.
        for p in [
            ".lucidos/worktrees/thread-x/src/main.rs",
            ".lucidos/exhaust/run.log",
            ".lucidos/engine.pid",
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
        // `.lucidos/tmp` and `.lucidos/tmp/` name the directory, not a file in
        // it, so they are refused rather than resolved to a path a read would
        // fail on with a confusing "is a directory".
        for p in [".lucidos", ".lucidos/", ".lucidos/tmp", ".lucidos/tmp/"] {
            assert!(normalize_data_path(p).is_err(), "{p} should be refused");
        }
    }

    #[test]
    fn data_prefixed_lucidos_path_is_corrected_not_rerouted() {
        // The chat system prompt warns against this exact form. Say where
        // .lucidos/ actually lives instead of routing it under data/.
        let err = normalize_data_path("data/.lucidos/tmp/x.json").unwrap_err();
        assert!(err.contains("beside data/"), "got: {err}");
        assert!(err.contains("'.lucidos/tmp/x.json'"), "got: {err}");
    }

    #[test]
    fn refusals_name_the_route_that_works() {
        // A refusal the model can act on: run_python is how scratch is created.
        let err = normalize_data_path(".lucidos/exhaust/y").unwrap_err();
        assert!(err.contains("run_python"), "got: {err}");
    }

    #[test]
    fn a_dotfile_that_merely_starts_with_lucidos_is_not_scratch() {
        // Prefix matching is on the whole `.lucidos` segment, so an artifact
        // named `.lucidosrc` keeps defaulting under artifacts/.
        assert_eq!(
            normalize_data_path(".lucidosrc").unwrap(),
            "artifacts/.lucidosrc"
        );
    }
}
