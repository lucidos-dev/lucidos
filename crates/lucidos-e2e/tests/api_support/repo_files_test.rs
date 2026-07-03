use crate::support::{base_url, git, git_in, http_client, register_repo, workspace_path};
use uuid::Uuid;

/// Register the e2e workspace as a test repository, returning its ID.
async fn register_test_repo(client: &reqwest::Client) -> String {
    register_repo(client, &workspace_path(), "e2e-repo").await
}

#[tokio::test]
async fn list_repo_files_returns_file_list() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!("{}/api/v1/repositories/{}/files", base_url(), repo_id))
        .send()
        .await
        .expect("List files failed");
    assert_eq!(resp.status().as_u16(), 200);
    let files: Vec<String> = resp.json().await.expect("Invalid JSON");
    assert!(!files.is_empty(), "Expected at least one file");
    // The e2e workspace should have .gitignore
    assert!(
        files.iter().any(|f| f == ".gitignore"),
        "Expected .gitignore in file list, got: {:?}",
        &files[..5.min(files.len())]
    );
}

#[tokio::test]
async fn list_repo_files_with_ref() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/files?ref=HEAD",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("List files with ref failed");
    assert_eq!(resp.status().as_u16(), 200);
    let files: Vec<String> = resp.json().await.expect("Invalid JSON");
    assert!(!files.is_empty());
}

#[tokio::test]
async fn list_repo_files_rejects_invalid_ref() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/files?ref=HEAD;rm+-rf+/",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(
        resp.status().as_u16(),
        400,
        "Should reject ref with semicolon"
    );

    let resp2 = client
        .get(format!(
            "{}/api/v1/repositories/{}/files?ref=../../etc/passwd",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp2.status().as_u16(), 400, "Should reject ref with ..");
}

#[tokio::test]
async fn get_repo_file_content() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/file?path=.gitignore",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Get file failed");
    assert_eq!(resp.status().as_u16(), 200);
    let content = resp.text().await.expect("Failed to read body");
    assert!(!content.is_empty(), ".gitignore should not be empty");
}

#[tokio::test]
async fn get_repo_file_rejects_path_traversal() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/file?path=../../etc/passwd",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 400, "Should reject path with ..");
}

#[tokio::test]
async fn get_repo_file_rejects_invalid_ref() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/file?path=.gitignore&ref=HEAD|cat",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 400, "Should reject ref with pipe");
}

#[tokio::test]
async fn get_repo_file_not_found() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/file?path=nonexistent-file-xyz.txt",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn get_repo_diff_with_branch() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;
    let ws = workspace_path();

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let branch = format!("e2e-test/repo-diff-{}", suffix);
    let file = format!("e2e-diff-{}.txt", suffix);
    let wt_dir = std::env::temp_dir().join(format!("e2e-wt-diff-{}", suffix));

    // Use a worktree to create the branch without touching the main working tree
    git(&[
        "worktree",
        "add",
        wt_dir.to_str().unwrap(),
        "-b",
        &branch,
        "main",
    ]);
    std::fs::write(wt_dir.join(&file), "diff test content").unwrap();
    git_in(&wt_dir, &["add", &file]);
    git_in(&wt_dir, &["commit", "-m", "e2e diff test"]);

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/diff?branch={}",
            base_url(),
            repo_id,
            &branch
        ))
        .send()
        .await
        .expect("Diff request failed");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let files = body["files"].as_array().expect("files should be array");
    assert!(
        files.iter().any(|f| f["path"].as_str() == Some(&file)),
        "Diff should include {}, got: {:?}",
        file,
        files.iter().map(|f| f["path"].as_str()).collect::<Vec<_>>()
    );
    let diff_file = files
        .iter()
        .find(|f| f["path"].as_str() == Some(&file))
        .unwrap();
    assert_eq!(diff_file["status"].as_str(), Some("added"));

    // Cleanup
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", wt_dir.to_str().unwrap()])
        .current_dir(&ws)
        .output();
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", &branch])
        .current_dir(&ws)
        .output();
}

/// Set up a branch in the e2e workspace with a single file whose name contains
/// an emoji. Returns `(branch, file, wt_dir)`. Caller is responsible for
/// teardown via `cleanup_utf8_branch`.
fn create_utf8_branch(name_prefix: &str) -> (String, String, std::path::PathBuf) {
    let suffix = &Uuid::new_v4().as_simple().to_string()[..8];
    let branch = format!("e2e-test/utf8-{}-{}", name_prefix, suffix);
    let file = format!("e2e-7_🧮_HLL-{}.py", suffix);
    let wt_dir = std::env::temp_dir().join(format!("e2e-wt-utf8-{}-{}", name_prefix, suffix));

    git(&[
        "worktree",
        "add",
        wt_dir.to_str().unwrap(),
        "-b",
        &branch,
        "main",
    ]);
    std::fs::write(wt_dir.join(&file), "utf8 test").unwrap();
    git_in(&wt_dir, &["add", &file]);
    git_in(&wt_dir, &["commit", "-m", "e2e utf8 test"]);

    (branch, file, wt_dir)
}

fn cleanup_utf8_branch(branch: &str, wt_dir: &std::path::Path) {
    let ws = workspace_path();
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", wt_dir.to_str().unwrap()])
        .current_dir(&ws)
        .output();
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(&ws)
        .output();
}

/// Regression: paths with non-ASCII bytes (emoji, accented letters) must come
/// back as raw UTF-8, not git's default `"...\NNN..."` C-quoted form. Without
/// `-c core.quotepath=false` on the underlying `git ls-tree` / `git diff`
/// invocations the file tree renders folders like `"streamlit` and the diff
/// row shows the raw `diff --git ...` header instead of the filename.
#[tokio::test]
async fn list_repo_files_returns_raw_utf8_paths() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;
    let (branch, file, wt_dir) = create_utf8_branch("files");

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/files?ref={}",
            base_url(),
            repo_id,
            &branch
        ))
        .send()
        .await
        .expect("List files failed");
    assert_eq!(resp.status().as_u16(), 200);
    let files: Vec<String> = resp.json().await.expect("Invalid JSON");

    assert!(
        files.iter().any(|f| f == &file),
        "Expected raw UTF-8 path {:?} in file list; got entries with emoji-related bytes: {:?}",
        file,
        files.iter().filter(|f| f.contains("🧮") || f.contains("\\360")).collect::<Vec<_>>()
    );
    assert!(
        files.iter().all(|f| !f.starts_with('"') && !f.ends_with('"') && !f.contains("\\360")),
        "No path should be C-quoted; offenders: {:?}",
        files.iter().filter(|f| f.starts_with('"') || f.ends_with('"') || f.contains("\\360")).collect::<Vec<_>>()
    );

    cleanup_utf8_branch(&branch, &wt_dir);
}

#[tokio::test]
async fn get_repo_diff_returns_raw_utf8_paths() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;
    let (branch, file, wt_dir) = create_utf8_branch("diff");

    let resp = client
        .get(format!(
            "{}/api/v1/repositories/{}/diff?branch={}",
            base_url(),
            repo_id,
            &branch
        ))
        .send()
        .await
        .expect("Diff request failed");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let files = body["files"].as_array().expect("files should be array");
    let paths: Vec<&str> = files.iter().filter_map(|f| f["path"].as_str()).collect();

    assert!(
        paths.iter().any(|p| *p == file),
        "Diff path should be raw UTF-8 {:?}, got: {:?}",
        file,
        paths
    );
    assert!(
        paths.iter().all(|p| !p.contains("diff --git")),
        "Diff path must never contain the raw `diff --git` header line; got: {:?}",
        paths
    );
    assert!(
        paths.iter().all(|p| !p.starts_with('"') && !p.ends_with('"') && !p.contains("\\360")),
        "Diff path must not be C-quoted; got: {:?}",
        paths
    );

    cleanup_utf8_branch(&branch, &wt_dir);
}

#[tokio::test]
async fn repo_not_found_returns_404() {
    let client = http_client();
    let fake_id = Uuid::new_v4();

    let resp = client
        .get(format!("{}/api/v1/repositories/{}/files", base_url(), fake_id))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 404);
}
