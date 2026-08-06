use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::diff::{parse_diff_output, DiffFile, RepoDiff};
use super::AppState;
use crate::core::repositories::{Repository, RepositoryStore};

pub async fn list_repositories(
    State(state): State<AppState>,
) -> Result<Json<Vec<Repository>>, (StatusCode, String)> {
    RepositoryStore::list(&state.pool)
        .await
        .map(Json)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list repositories: {}", e),
            )
        })
}

#[derive(Deserialize)]
pub struct AddRepositoryRequest {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub description: Option<String>,
}

pub async fn add_repository(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AddRepositoryRequest>,
) -> Result<(StatusCode, Json<Repository>), (StatusCode, String)> {
    let expanded_path = crate::core::home_path::expand(&req.path);

    // Validate path exists
    let path = std::path::Path::new(&expanded_path);
    if !path.exists() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Path does not exist: {}", expanded_path),
        ));
    }

    // Validate it's a git repo
    let git_check = tokio::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to check git repo: {}", e),
            )
        })?;

    if !git_check.status.success() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Not a git repository: {}", expanded_path),
        ));
    }

    // Intrinsic identity comes from the repo's root-commit SHA (read from disk),
    // so re-registering the same history is idempotent. None (no commits) falls
    // back to a path-derived id inside `add`.
    let root_commit_sha = crate::engine::git_ops::root_commit_sha(path).await;

    // `register` emits `RepositoryAdded` itself, so this handler resolves the
    // device actor up front instead of going through `emit_user_system`. The
    // emit is not the caller's to make (see `RepositoryStore`'s type doc).
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    let repo = RepositoryStore::register(
        &state.pool,
        &state.engine.event_bus,
        &req.name,
        &expanded_path,
        req.description.as_deref(),
        root_commit_sha.as_deref(),
        actor,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::CONFLICT,
            format!("Failed to add repository: {}", e),
        )
    })?;

    Ok((StatusCode::CREATED, Json(repo)))
}

pub async fn remove_repository(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let actor = crate::api::actor::user_actor_resolved(&headers, &state.pool, None).await;
    match RepositoryStore::unregister(&state.pool, &state.engine.event_bus, id, actor).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((StatusCode::NOT_FOUND, "Repository not found".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove repository: {}", e),
        )),
    }
}

// --- Repo File Explorer endpoints ---

#[derive(Deserialize)]
pub struct RepoFilesQuery {
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
}

pub async fn list_repo_files(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Query(query): Query<RepoFilesQuery>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let repo = RepositoryStore::get(&state.pool, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
        .ok_or((StatusCode::NOT_FOUND, "Repository not found".into()))?;

    let git_ref = query.git_ref.as_deref().unwrap_or("HEAD");

    if super::is_dangerous_git_ref(git_ref) {
        return Err((StatusCode::BAD_REQUEST, "Invalid ref".into()));
    }

    let output = crate::engine::git_ops::git_cmd(
        &["ls-tree", "-r", "--name-only", git_ref],
        std::path::Path::new(&repo.path),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((
            StatusCode::BAD_REQUEST,
            format!("git ls-tree failed: {stderr}"),
        ));
    }

    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();

    Ok(Json(files))
}

#[derive(Deserialize)]
pub struct RepoFileQuery {
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub path: String,
}

pub async fn get_repo_file(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Query(query): Query<RepoFileQuery>,
) -> Response {
    let repo = match RepositoryStore::get(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "Repository not found").into_response(),
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")).into_response()
        }
    };

    git_show_file(
        std::path::Path::new(&repo.path),
        query.git_ref.as_deref().unwrap_or("HEAD"),
        &query.path,
    )
    .await
}

/// Run `git show {ref}:{path}` and return the file body with a content-type
/// inferred from the extension. Validates path-traversal and dangerous refs.
async fn git_show_file(repo_root: &std::path::Path, git_ref: &str, path: &str) -> Response {
    if super::is_path_traversal(path) {
        return (StatusCode::BAD_REQUEST, "Invalid path").into_response();
    }
    if super::is_dangerous_git_ref(git_ref) {
        return (StatusCode::BAD_REQUEST, "Invalid ref").into_response();
    }

    let object = format!("{git_ref}:{path}");
    let output = match tokio::process::Command::new("git")
        .args(["show", &object])
        .current_dir(repo_root)
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("git error: {e}")).into_response()
        }
    };

    if !output.status.success() {
        return (StatusCode::NOT_FOUND, "File not found at ref").into_response();
    }

    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let content_type = super::content_type_for_ext(&ext);
    ([(header::CONTENT_TYPE, content_type)], output.stdout).into_response()
}

#[derive(Deserialize)]
pub struct RepoDiffQuery {
    pub branch: String,
}

pub async fn get_repo_diff(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Query(query): Query<RepoDiffQuery>,
) -> Result<Json<RepoDiff>, (StatusCode, String)> {
    let repo = RepositoryStore::get(&state.pool, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
        .ok_or((StatusCode::NOT_FOUND, "Repository not found".into()))?;

    if super::is_dangerous_git_ref(&query.branch) {
        return Err((StatusCode::BAD_REQUEST, "Invalid branch name".into()));
    }

    // Resolve the real default branch (`origin/<default>` / `main` / `master` /
    // `HEAD`), never a hardcoded `main` — external repos legitimately use a
    // different default, and diffing against a phantom `main` 400s with
    // "unknown revision". Mirrors `get_change_diff` / `get_thread_cc_diff`.
    let base = crate::engine::git_ops::default_diff_base(std::path::Path::new(&repo.path)).await;
    let range = format!("{}...{}", base, query.branch);
    let output = crate::engine::git_ops::git_cmd(
        &["diff", &range, "--no-color"],
        std::path::Path::new(&repo.path),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((
            StatusCode::BAD_REQUEST,
            format!("git diff failed: {stderr}"),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files = parse_diff_output(&stdout);

    Ok(Json(RepoDiff { files }))
}

/// GET /api/v1/changes/:id/diff — compute diff for any change (pending or applied)
///
/// Mirrors `get_thread_cc_diff` for app coding-agent changes: resolves the
/// pending base via `default_diff_base` (the workspace's actual default branch,
/// not a hardcoded `main`) and scopes the diff to `data/apps/<id>/` via
/// `lookup_app_pathspec`, so the change-row Diff matches the thread-level Diff
/// and never leaks files the agent touched outside the app folder. Registered
/// Lucidos-source / external changes resolve `default_diff_base` to their
/// `main` / `origin/main`, so their diff is unchanged.
pub async fn get_change_diff(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<RepoDiff>, (StatusCode, String)> {
    let change = state
        .engine
        .changes()
        .get_by_id(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
        .ok_or((StatusCode::NOT_FOUND, "Change not found".into()))?;

    let repo_root = std::path::Path::new(&change.repo_root);
    let range = if change.status == "pending" {
        if super::is_dangerous_git_ref(&change.branch_name) {
            return Err((StatusCode::BAD_REQUEST, "Invalid branch name".into()));
        }
        let base = crate::engine::git_ops::default_diff_base(repo_root).await;
        format!("{}...{}", base, change.branch_name)
    } else {
        let pre_sha = change.pre_merge_sha.as_deref().ok_or((
            StatusCode::BAD_REQUEST,
            "No merge SHA recorded for this change — it was applied before SHA tracking was added"
                .into(),
        ))?;
        let post_sha = change
            .post_merge_sha
            .as_deref()
            .ok_or((StatusCode::BAD_REQUEST, "No post-merge SHA recorded".into()))?;
        format!("{}..{}", pre_sha, post_sha)
    };

    // App coding-agent changes live under `data/apps/<id>/` in the workspace
    // repo; scope the diff there. None for Lucidos-source / external changes
    // (and for changes with no thread_id), leaving the diff unscoped as before.
    let pathspec = match change.thread_id {
        Some(thread_id) => lookup_app_pathspec(&state.pool, thread_id).await,
        None => None,
    };
    let files = run_git_diff(repo_root, &range, pathspec.as_deref()).await?;
    Ok(Json(RepoDiff { files }))
}

#[derive(Serialize)]
pub struct ThreadCcDiff {
    pub files: Vec<DiffFile>,
    pub repo_root: String,
    pub branch_name: String,
    pub base_ref: String,
}

/// GET /api/v1/threads/:thread_id/cc-diff — 3-dot diff of the CC worktree's
/// branch vs the repo's default remote branch. Used for external-repo CC
/// sessions that never create a Lucidos `Change` row.
///
/// Falls back to the registered repo + branch ref when the recorded worktree
/// is gone — pre-May-2026 `agent_recovery` removed worktrees for idle
/// external-repo sessions without emitting `WorktreeCleaned`, so the
/// historical state is `coding_agent_proposed=true` + branch alive + worktree dir
/// missing.
pub async fn get_thread_cc_diff(
    State(state): State<AppState>,
    axum::extract::Path(thread_id): axum::extract::Path<Uuid>,
) -> Result<Json<ThreadCcDiff>, (StatusCode, String)> {
    // For app coding-agent threads the worktree's git root is the workspace
    // and the relevant changes live under `data/apps/<id>/`. Scoping the diff
    // to that pathspec keeps stray writes outside the app folder out of the
    // Diff button's response — the user asked for the diff scoped to the
    // app, not to whatever the agent happened to touch.
    let app_pathspec = lookup_app_pathspec(&state.pool, thread_id).await;
    if let Some(worktree_path) =
        crate::engine::agent_session::resume::lookup_latest_worktree_path(&state.pool, thread_id)
            .await
            .filter(|p| p.exists())
    {
        return diff_via_worktree(&worktree_path, app_pathspec.as_deref())
            .await
            .map(Json);
    }
    diff_via_branch_ref(&state.pool, thread_id, app_pathspec.as_deref())
        .await
        .map(Json)
}

/// Look up the in-app pathspec for an app coding-agent thread, e.g.
/// `data/apps/habit-tracker`. Returns None for Lucidos-source and
/// external-repo threads (and for legacy app rows with a NULL folder, in
/// which case the unscoped diff is the best we can do).
///
/// Validates the folder shape matches `<…>/data/apps/<id>` exactly — same
/// contract the frontend's `appIdFromFolder` enforces. Without this, a
/// corrupted row with `kind = 'app'` but `folder = '/ws/projects/foo'`
/// would silently scope the diff to a phantom `data/apps/foo` and the
/// user would see an empty diff with no error.
async fn lookup_app_pathspec(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT coding_agent_kind, coding_agent_folder FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let (kind, folder) = row?;
    if kind.as_deref() != Some("app") {
        return None;
    }
    let folder = folder?;
    let app_id = extract_app_id(&folder)?;
    Some(format!("data/apps/{app_id}"))
}

/// Extract the app id from a `coding_agent_folder` value. Mirrors the
/// frontend's `appIdFromFolder` validation: requires the folder to end in
/// `…/data/apps/<id>` with a non-empty, non-traversal id, with `data`
/// immediately before `apps`.
fn extract_app_id(folder: &str) -> Option<String> {
    let segments: Vec<&str> = folder.split('/').filter(|s| !s.is_empty()).collect();
    let apps_idx = segments.iter().rposition(|s| *s == "apps")?;
    if apps_idx == segments.len() - 1 {
        return None;
    }
    if apps_idx == 0 || segments[apps_idx - 1] != "data" {
        return None;
    }
    let id = segments[apps_idx + 1];
    if id.is_empty() || id == "." || id == ".." {
        return None;
    }
    Some(id.to_string())
}

async fn diff_via_worktree(
    worktree_path: &std::path::Path,
    pathspec: Option<&str>,
) -> Result<ThreadCcDiff, (StatusCode, String)> {
    let (branch_opt, base_ref, repo_root_res) = tokio::join!(
        crate::engine::git_ops::worktree_current_branch(worktree_path),
        crate::engine::git_ops::default_diff_base(worktree_path),
        resolve_worktree_repo_root(worktree_path),
    );
    let branch_name = branch_opt.ok_or((
        StatusCode::BAD_REQUEST,
        "Worktree has detached HEAD — cannot diff".into(),
    ))?;
    let repo_root = repo_root_res
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "git worktree list returned no main worktree entry".into(),
        ))?;

    let files = run_git_diff(worktree_path, &format!("{}...HEAD", base_ref), pathspec).await?;
    Ok(ThreadCcDiff {
        files,
        repo_root,
        branch_name,
        base_ref,
    })
}

async fn diff_via_branch_ref(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    pathspec: Option<&str>,
) -> Result<ThreadCcDiff, (StatusCode, String)> {
    let (repo_id_str, branch_name) =
        crate::engine::agent_session::resume::lookup_latest_session_repo_and_branch(
            pool, thread_id,
        )
        .await
        .ok_or((
            StatusCode::NOT_FOUND,
            "No worktree on disk and no SessionStarted recorded for this thread".into(),
        ))?;
    if super::is_dangerous_git_ref(&branch_name) {
        return Err((StatusCode::BAD_REQUEST, "Invalid branch name".into()));
    }
    let repo_id = Uuid::parse_str(&repo_id_str).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "SessionStarted.repo_id is not a UUID".into(),
        )
    })?;
    let repo = RepositoryStore::get(pool, repo_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to look up repository: {e}"),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            "Worktree gone and recorded repository no longer registered".into(),
        ))?;

    let repo_root = std::path::PathBuf::from(&repo.path);
    let base_ref = crate::engine::git_ops::default_diff_base(&repo_root).await;
    let branch_name =
        resolve_recorded_branch(pool, thread_id, &repo_root, branch_name, &base_ref).await?;
    let files = run_git_diff(
        &repo_root,
        &format!("{}...{}", base_ref, branch_name),
        pathspec,
    )
    .await?;
    Ok(ThreadCcDiff {
        files,
        repo_root: repo.path,
        branch_name,
        base_ref,
    })
}

/// The branch to diff when there is no worktree left to ask.
///
/// `SessionStarted.branch` is a spawn-time snapshot, and the branch belongs to
/// git inside a worktree the coding agent can write to: a skill running
/// `git branch -m` leaves the recorded name pointing at a ref that no longer
/// exists. Every live path re-reads the worktree instead (`diff_via_worktree`
/// above diffs `base...HEAD`, and the idle adopts the real branch), but this
/// fallback runs precisely when the worktree has been reclaimed, so it has to
/// find the work another way: the thread's last known commit, and the branch
/// that still contains it.
///
/// Without this the Diff button answered a renamed thread with git's raw
/// `fatal: ambiguous argument 'main...<gone-ref>'` as a 400, for a branch that
/// was sitting right there under its new name.
async fn resolve_recorded_branch(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    repo_root: &std::path::Path,
    recorded: String,
    base_ref: &str,
) -> Result<String, (StatusCode, String)> {
    // `or_unknown(true)`: an unanswerable probe keeps the recorded name, so a
    // busy host produces git's own error rather than a wrong-branch diff.
    if crate::engine::git_ops::local_branch_exists(repo_root, &recorded)
        .await
        .or_unknown(true)
    {
        return Ok(recorded);
    }
    let head_sha =
        crate::engine::agent_session::resume::lookup_latest_worktree_head_sha(pool, thread_id)
            .await
            .ok_or((
                StatusCode::NOT_FOUND,
                format!(
                    "Branch '{recorded}' no longer exists in this repository, and the thread \
                     recorded no commit to locate its work by."
                ),
            ))?;
    // Exclude the diff base AND the local default: `base_ref` is
    // `origin/<default>` whenever the local default has diverged, and
    // `for-each-ref refs/heads/` never lists a remote-tracking ref, so on its
    // own it would leave the local default a candidate for work that was merged
    // and whose branch was then deleted.
    let local_default = crate::engine::git_ops::default_local_branch(repo_root).await;
    match crate::engine::git_ops::sole_branch_containing(
        repo_root,
        &head_sha,
        &[base_ref, &local_default],
    )
    .await
    {
        Some(found) => {
            crate::log!(
                "[Repositories] Thread {} recorded branch '{}' is gone; diffing '{}', which contains its last commit",
                thread_id,
                recorded,
                found
            );
            Ok(found)
        }
        None => Err((
            StatusCode::NOT_FOUND,
            format!(
                "Branch '{recorded}' no longer exists in this repository, and no single branch \
                 contains this thread's last commit ({}). It was renamed and deleted, or its \
                 work was never committed.",
                &head_sha[..head_sha.floor_char_boundary(9)]
            ),
        )),
    }
}

async fn run_git_diff(
    cwd: &std::path::Path,
    range: &str,
    pathspec: Option<&str>,
) -> Result<Vec<DiffFile>, (StatusCode, String)> {
    let mut args: Vec<&str> = vec!["diff", range, "--no-color"];
    if let Some(p) = pathspec {
        args.push("--");
        args.push(p);
    }
    let output = crate::engine::git_ops::git_cmd(&args, cwd)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if !output.status.success() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    Ok(parse_diff_output(&String::from_utf8_lossy(&output.stdout)))
}

/// Resolve the main worktree's working-tree root for a (possibly linked)
/// worktree. `git worktree list --porcelain` always lists the main worktree
/// first, so we read the first `worktree <path>` line.
async fn resolve_worktree_repo_root(
    worktree_path: &std::path::Path,
) -> Result<Option<String>, String> {
    let output =
        crate::engine::git_ops::git_cmd(&["worktree", "list", "--porcelain"], worktree_path)
            .await?;
    if !output.status.success() {
        return Err(format!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("worktree ").map(str::to_owned)))
}

#[derive(Deserialize)]
pub struct ChangeFileQuery {
    pub path: String,
}

/// GET /api/v1/changes/:id/file?path=X — fetch the full "after" version of a file
/// at the change's branch (pending) or post-merge SHA (applied).
pub async fn get_change_file(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Query(query): Query<ChangeFileQuery>,
) -> Response {
    let change = match state.engine.changes().get_by_id(id).await {
        Ok(Some(c)) => c,
        Ok(None) => return (StatusCode::NOT_FOUND, "Change not found").into_response(),
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")).into_response();
        }
    };

    let git_ref = if change.status == "pending" {
        change.branch_name.clone()
    } else {
        match change.post_merge_sha.as_deref() {
            Some(sha) => sha.to_string(),
            None => return (StatusCode::BAD_REQUEST, "No post-merge SHA recorded").into_response(),
        }
    };

    git_show_file(
        std::path::Path::new(&change.repo_root),
        &git_ref,
        &query.path,
    )
    .await
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    pub path: Option<String>,
}

#[derive(Serialize)]
pub struct BrowseResult {
    pub path: String,
    pub directories: Vec<String>,
    pub is_git_repo: bool,
}

fn is_dangerous_browse_path(path: &str) -> bool {
    path.is_empty() || path.contains("..")
}

pub async fn browse_directories(
    Query(query): Query<BrowseQuery>,
) -> Result<Json<BrowseResult>, (StatusCode, String)> {
    let raw_path = query.path.unwrap_or_else(|| "~".to_string());
    let expanded = crate::core::home_path::expand(&raw_path);

    if is_dangerous_browse_path(&expanded) {
        return Err((StatusCode::BAD_REQUEST, "Invalid path".into()));
    }

    let dir = std::path::Path::new(&expanded);
    if !dir.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Not a directory: {}", expanded),
        ));
    }

    let canonical = dir.canonicalize().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Cannot resolve path: {}", e),
        )
    })?;

    let mut directories = Vec::new();
    match std::fs::read_dir(&canonical) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !name.starts_with('.') {
                                directories.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Cannot read directory: {}", e),
            ));
        }
    }

    directories.sort_by_key(|a| a.to_lowercase());

    let git_dir = canonical.join(".git");
    let is_git_repo = git_dir.exists();

    Ok(Json(BrowseResult {
        path: canonical.to_string_lossy().to_string(),
        directories,
        is_git_repo,
    }))
}

/// Routes for the `/repositories*` and `/browse-directories` surfaces. The
/// change- and thread-shaped diff routes this module also implements
/// register under `changes::router()` / `threads::router()` — grouped by
/// URL surface, not handler location.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/repositories", get(list_repositories).post(add_repository))
        .route("/browse-directories", get(browse_directories))
        .route(
            "/repositories/:id",
            axum::routing::delete(remove_repository),
        )
        .route("/repositories/:id/files", get(list_repo_files))
        .route("/repositories/:id/file", get(get_repo_file))
        .route("/repositories/:id/diff", get(get_repo_diff))
}

#[cfg(test)]
#[path = "repositories_tests.rs"]
mod tests;
