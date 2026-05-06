use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AppState;
use crate::core::repositories::{Repository, RepositoryStore};

fn expand_tilde(path: &str) -> String {
    if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            format!("{home}/{rest}")
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    }
}

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
    Json(req): Json<AddRepositoryRequest>,
) -> Result<(StatusCode, Json<Repository>), (StatusCode, String)> {
    let expanded_path = expand_tilde(&req.path);

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

    RepositoryStore::add(
        &state.pool,
        &req.name,
        &expanded_path,
        req.description.as_deref(),
    )
    .await
    .map(|repo| (StatusCode::CREATED, Json(repo)))
    .map_err(|e| {
        (
            StatusCode::CONFLICT,
            format!("Failed to add repository: {}", e),
        )
    })
}

pub async fn remove_repository(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    match RepositoryStore::remove(&state.pool, id).await {
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

    let output = tokio::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", git_ref])
        .current_dir(&repo.path)
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("git error: {e}")))?;

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

#[derive(Serialize)]
pub struct RepoDiff {
    pub files: Vec<DiffFile>,
}

#[derive(Serialize)]
pub struct DiffFile {
    pub path: String,
    pub status: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Serialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Serialize)]
pub struct DiffLine {
    #[serde(rename = "type")]
    pub line_type: String,
    pub content: String,
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

    let range = format!("main...{}", query.branch);
    let output = tokio::process::Command::new("git")
        .args(["diff", &range, "--no-color"])
        .current_dir(&repo.path)
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("git error: {e}")))?;

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

fn parse_diff_output(output: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current_file: Option<DiffFile> = None;
    let mut current_hunk: Option<DiffHunk> = None;

    for line in output.lines() {
        if line.starts_with("diff --git") {
            if let Some(mut f) = current_file.take() {
                if let Some(h) = current_hunk.take() {
                    f.hunks.push(h);
                }
                files.push(f);
            }
            // Extract path from "diff --git a/path b/path" as fallback for renames
            let fallback_path = line.split(" b/").last().unwrap_or("").to_string();
            current_file = Some(DiffFile {
                path: fallback_path,
                status: "modified".into(),
                hunks: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            if let Some(ref mut f) = current_file {
                f.path = rest.to_string();
            }
        } else if line.starts_with("--- /dev/null") {
            if let Some(ref mut f) = current_file {
                f.status = "added".into();
            }
        } else if line.starts_with("+++ /dev/null") {
            if let Some(ref mut f) = current_file {
                f.status = "deleted".into();
            }
        } else if let Some(rest) = line.strip_prefix("--- a/") {
            if let Some(ref mut f) = current_file {
                if f.path.is_empty() {
                    f.path = rest.to_string();
                }
            }
        } else if line.starts_with("@@ ") {
            if let Some(ref mut f) = current_file {
                if let Some(h) = current_hunk.take() {
                    f.hunks.push(h);
                }
            }
            if let Some(hunk) = parse_hunk_header(line) {
                current_hunk = Some(hunk);
            }
        } else if let Some(ref mut hunk) = current_hunk {
            if let Some(rest) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine {
                    line_type: "addition".into(),
                    content: rest.to_string(),
                });
            } else if let Some(rest) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine {
                    line_type: "deletion".into(),
                    content: rest.to_string(),
                });
            } else if let Some(rest) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine {
                    line_type: "context".into(),
                    content: rest.to_string(),
                });
            }
        }
    }

    if let Some(mut f) = current_file {
        if let Some(h) = current_hunk {
            f.hunks.push(h);
        }
        files.push(f);
    }

    files
}

fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let old = parts[1].trim_start_matches('-');
    let new = parts[2].trim_start_matches('+');

    let (old_start, old_count) = parse_range(old);
    let (new_start, new_count) = parse_range(new);

    Some(DiffHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
    })
}

fn parse_range(s: &str) -> (u32, u32) {
    if let Some((start, count)) = s.split_once(',') {
        (start.parse().unwrap_or(0), count.parse().unwrap_or(0))
    } else {
        (s.parse().unwrap_or(0), 1)
    }
}

/// GET /api/changes/:id/diff — compute diff for any change (pending or applied)
pub async fn get_change_diff(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
) -> Result<Json<RepoDiff>, (StatusCode, String)> {
    let change = state
        .engine
        .changes()
        .get_by_id(id)
        .await
        .ok_or((StatusCode::NOT_FOUND, "Change not found".into()))?;

    let repo_root = &change.repo_root;

    let diff_args = if change.status == "pending" {
        if super::is_dangerous_git_ref(&change.branch_name) {
            return Err((StatusCode::BAD_REQUEST, "Invalid branch name".into()));
        }
        vec![
            "diff".to_string(),
            format!("main...{}", change.branch_name),
            "--no-color".to_string(),
        ]
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
        vec![
            "diff".to_string(),
            format!("{}..{}", pre_sha, post_sha),
            "--no-color".to_string(),
        ]
    };

    let output = tokio::process::Command::new("git")
        .args(&diff_args)
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("git error: {e}")))?;

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

#[derive(Deserialize)]
pub struct ChangeFileQuery {
    pub path: String,
}

/// GET /api/changes/:id/file?path=X — fetch the full "after" version of a file
/// at the change's branch (pending) or post-merge SHA (applied).
pub async fn get_change_file(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Query(query): Query<ChangeFileQuery>,
) -> Response {
    let change = match state.engine.changes().get_by_id(id).await {
        Some(c) => c,
        None => return (StatusCode::NOT_FOUND, "Change not found").into_response(),
    };

    let git_ref = if change.status == "pending" {
        change.branch_name.clone()
    } else {
        match change.post_merge_sha.as_deref() {
            Some(sha) => sha.to_string(),
            None => {
                return (StatusCode::BAD_REQUEST, "No post-merge SHA recorded").into_response()
            }
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
    let expanded = expand_tilde(&raw_path);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_modified_file_diff() {
        let input = concat!(
            "diff --git a/src/main.rs b/src/main.rs\n",
            "--- a/src/main.rs\n",
            "+++ b/src/main.rs\n",
            "@@ -1,3 +1,4 @@\n",
            " fn main() {\n",
            "-    println!(\"old\");\n",
            "+    println!(\"new\");\n",
            "+    println!(\"extra\");\n",
            " }\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].status, "modified");
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].old_start, 1);
        assert_eq!(files[0].hunks[0].old_count, 3);
        assert_eq!(files[0].hunks[0].new_start, 1);
        assert_eq!(files[0].hunks[0].new_count, 4);
        // 2 context + 1 deletion + 2 additions = 5
        assert_eq!(files[0].hunks[0].lines.len(), 5);
    }

    #[test]
    fn parse_added_file_diff() {
        let input = concat!(
            "diff --git a/new.rs b/new.rs\n",
            "new file mode 100644\n",
            "--- /dev/null\n",
            "+++ b/new.rs\n",
            "@@ -0,0 +1,2 @@\n",
            "+fn new_fn() {\n",
            "+}\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, "added");
        assert_eq!(files[0].path, "new.rs");
    }

    #[test]
    fn parse_deleted_file_diff() {
        let input = concat!(
            "diff --git a/old.rs b/old.rs\n",
            "deleted file mode 100644\n",
            "--- a/old.rs\n",
            "+++ /dev/null\n",
            "@@ -1,2 +0,0 @@\n",
            "-fn old_fn() {\n",
            "-}\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, "deleted");
        assert_eq!(files[0].path, "old.rs");
    }

    #[test]
    fn parse_multiple_files() {
        let input = concat!(
            "diff --git a/a.rs b/a.rs\n",
            "--- a/a.rs\n",
            "+++ b/a.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n",
            "diff --git a/b.rs b/b.rs\n",
            "--- a/b.rs\n",
            "+++ b/b.rs\n",
            "@@ -1 +1 @@\n",
            "-x\n",
            "+y\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.rs");
        assert_eq!(files[1].path, "b.rs");
    }

    #[test]
    fn parse_empty_diff() {
        let files = parse_diff_output("");
        assert!(files.is_empty());
    }

    #[test]
    fn parse_binary_file_skipped() {
        // Binary files have no hunks — parser should produce a file with empty hunks
        let input = concat!(
            "diff --git a/image.png b/image.png\n",
            "new file mode 100644\n",
            "Binary files /dev/null and b/image.png differ\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 1);
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn parse_rename_diff() {
        let input = concat!(
            "diff --git a/old_name.rs b/new_name.rs\n",
            "similarity index 100%\n",
            "rename from old_name.rs\n",
            "rename to new_name.rs\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 1);
        // Path extracted from "diff --git a/old b/new" header
        assert_eq!(files[0].path, "new_name.rs");
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn parse_multiple_hunks() {
        let input = concat!(
            "diff --git a/lib.rs b/lib.rs\n",
            "--- a/lib.rs\n",
            "+++ b/lib.rs\n",
            "@@ -1,2 +1,2 @@\n",
            "-old1\n",
            "+new1\n",
            " same\n",
            "@@ -10,3 +10,4 @@\n",
            " ctx\n",
            "-old2\n",
            "+new2\n",
            "+extra\n",
            " ctx\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[0].old_start, 1);
        assert_eq!(files[0].hunks[0].lines.len(), 3); // 1 del + 1 add + 1 ctx
        assert_eq!(files[0].hunks[1].old_start, 10);
        // 1 ctx + 1 del + 2 add + 1 ctx = 5
        assert_eq!(files[0].hunks[1].lines.len(), 5);
    }

    #[test]
    fn parse_no_newline_at_eof() {
        // git diff outputs "\ No newline at end of file" — parser should skip it
        let input = concat!(
            "diff --git a/f.rs b/f.rs\n",
            "--- a/f.rs\n",
            "+++ b/f.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "\\ No newline at end of file\n",
            "+new\n",
            "\\ No newline at end of file\n",
        );
        let files = parse_diff_output(input);
        assert_eq!(files.len(), 1);
        // Only the -old and +new lines, not the backslash lines
        assert_eq!(files[0].hunks[0].lines.len(), 2);
    }

    #[test]
    fn parse_hunk_header_no_count() {
        // Single-line hunk: @@ -5 +5 @@ (no comma count)
        let hunk = parse_hunk_header("@@ -5 +5 @@").unwrap();
        assert_eq!(hunk.old_start, 5);
        assert_eq!(hunk.old_count, 1);
        assert_eq!(hunk.new_start, 5);
        assert_eq!(hunk.new_count, 1);
    }

    #[test]
    fn parse_hunk_header_with_function_context() {
        // Real git often appends function name: @@ -10,3 +10,4 @@ fn main()
        let hunk = parse_hunk_header("@@ -10,3 +10,4 @@ fn main()").unwrap();
        assert_eq!(hunk.old_start, 10);
        assert_eq!(hunk.old_count, 3);
        assert_eq!(hunk.new_start, 10);
        assert_eq!(hunk.new_count, 4);
    }

    #[test]
    fn parse_hunk_header_malformed() {
        // Too few tokens — need at least @@ -x +y @@
        assert!(parse_hunk_header("@@").is_none());
        assert!(parse_hunk_header("@@ -1").is_none());
    }

    #[test]
    fn parse_range_basic() {
        assert_eq!(parse_range("5,3"), (5, 3));
        assert_eq!(parse_range("1"), (1, 1));
        assert_eq!(parse_range("0,0"), (0, 0));
    }

    #[test]
    fn browse_rejects_path_traversal() {
        assert!(super::is_dangerous_browse_path("../etc"));
        assert!(super::is_dangerous_browse_path("/foo/../bar"));
        assert!(super::is_dangerous_browse_path(""));
        assert!(!super::is_dangerous_browse_path("/Users/me/projects"));
        assert!(!super::is_dangerous_browse_path("/tmp"));
    }

    #[test]
    fn expand_tilde_handles_all_cases() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(super::expand_tilde("~"), home);
        assert!(super::expand_tilde("~/projects").starts_with(&home));
        assert_eq!(
            super::expand_tilde("~/projects"),
            format!("{}/projects", home)
        );
        assert_eq!(super::expand_tilde("/absolute/path"), "/absolute/path");
        assert_eq!(super::expand_tilde("relative"), "relative");
    }
}
