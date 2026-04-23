use crate::support::{base_url, git, git_in, http_client, unique_marker, workspace_path};
use std::path::Path;
use uuid::Uuid;

/// POST /api/repositories for the given path with the given label, returning the repo ID.
async fn register_repo(client: &reqwest::Client, path: &Path, label: &str) -> String {
    let body = serde_json::json!({
        "name": unique_marker(label),
        "path": path.to_str().unwrap(),
        "description": format!("{} test repo", label),
    });
    let resp = client
        .post(format!("{}/api/repositories", base_url()))
        .json(&body)
        .send()
        .await
        .expect("Register repo failed");
    assert_eq!(resp.status().as_u16(), 201, "Expected 201 Created");
    let repo: serde_json::Value = resp.json().await.expect("Invalid JSON");
    repo["id"].as_str().unwrap().to_string()
}

/// Register the e2e workspace as a test repository, returning its ID.
async fn register_test_repo(client: &reqwest::Client) -> String {
    register_repo(client, &workspace_path(), "e2e-repo").await
}

#[tokio::test]
#[ignore]
async fn list_repo_files_returns_file_list() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!("{}/api/repositories/{}/files", base_url(), repo_id))
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
#[ignore]
async fn list_repo_files_with_ref() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/repositories/{}/files?ref=HEAD",
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
#[ignore]
async fn list_repo_files_rejects_invalid_ref() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/repositories/{}/files?ref=HEAD;rm+-rf+/",
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
            "{}/api/repositories/{}/files?ref=../../etc/passwd",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp2.status().as_u16(), 400, "Should reject ref with ..");
}

#[tokio::test]
#[ignore]
async fn get_repo_file_content() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/repositories/{}/file?path=.gitignore",
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
#[ignore]
async fn get_repo_file_rejects_path_traversal() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/repositories/{}/file?path=../../etc/passwd",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 400, "Should reject path with ..");
}

#[tokio::test]
#[ignore]
async fn get_repo_file_rejects_invalid_ref() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/repositories/{}/file?path=.gitignore&ref=HEAD|cat",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 400, "Should reject ref with pipe");
}

#[tokio::test]
#[ignore]
async fn get_repo_file_not_found() {
    let client = http_client();
    let repo_id = register_test_repo(&client).await;

    let resp = client
        .get(format!(
            "{}/api/repositories/{}/file?path=nonexistent-file-xyz.txt",
            base_url(),
            repo_id
        ))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
#[ignore]
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
            "{}/api/repositories/{}/diff?branch={}",
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

#[tokio::test]
#[ignore]
async fn repo_not_found_returns_404() {
    let client = http_client();
    let fake_id = Uuid::new_v4();

    let resp = client
        .get(format!("{}/api/repositories/{}/files", base_url(), fake_id))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(resp.status().as_u16(), 404);
}
