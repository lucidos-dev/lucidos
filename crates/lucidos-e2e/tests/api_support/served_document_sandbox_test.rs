//! A workspace document served inline runs sandboxed, on every mount that
//! serves `data/` files (ADR 0322).
//!
//! Unit tests in `api/file_response_tests.rs` cover the header helper. Only a
//! booted engine shows it is wired into all three mounts, which live in three
//! different routers: the `/data` static mount, `GET /api/v1/data/*path`, and
//! `/app/:id/artifacts/*path`. A mount that misses it serves an uploaded page
//! at the workspace origin, with the shell's full authority.

use crate::support::{base_url, http_client, unique_marker, workspace_path};

fn write_data_file(rel: &str, body: &[u8]) -> std::path::PathBuf {
    let path = workspace_path().join("data").join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create parent dirs");
    }
    std::fs::write(&path, body).expect("Failed to write test file");
    path
}

async fn csp_of(url: &str) -> Option<String> {
    let resp = http_client()
        .get(url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("request to {url} failed: {e}"));
    assert_eq!(resp.status(), 200, "{url}");
    resp.headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// The three URLs one artifact is reachable at.
fn mounts(rel_in_artifacts: &str) -> [String; 3] {
    [
        format!("{}/data/artifacts/{rel_in_artifacts}", base_url()),
        format!("{}/api/v1/data/artifacts/{rel_in_artifacts}", base_url()),
        format!("{}/app/any-app/artifacts/{rel_in_artifacts}", base_url()),
    ]
}

/// One test, because it writes into the workspace tree and holds its lock for
/// the whole sequence.
#[tokio::test]
async fn every_data_mount_sandboxes_documents_and_leaves_other_files_alone() {
    let _tree = crate::support::workspace_tree_lock().read().await;

    let dir = unique_marker("sandbox");
    let documents = ["page.html", "drawing.svg", "feed.xml"];
    let others = ["notes.md", "table.csv", "data.json", "pixel.png"];
    let written: Vec<_> = documents
        .iter()
        .chain(others.iter())
        .map(|name| write_data_file(&format!("artifacts/{dir}/{name}"), b"<x/>"))
        .collect();

    for name in documents {
        for url in mounts(&format!("{dir}/{name}")) {
            let csp = csp_of(&url)
                .await
                .unwrap_or_else(|| panic!("{url} served a document with no sandbox"));
            assert!(csp.starts_with("sandbox "), "{url}: {csp}");
            assert!(csp.contains("allow-scripts"), "{url}: {csp}");
            assert!(!csp.contains("allow-same-origin"), "{url}: {csp}");
        }
    }
    for name in others {
        for url in mounts(&format!("{dir}/{name}")) {
            assert_eq!(csp_of(&url).await, None, "{url} gained a CSP");
        }
    }

    for path in written {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_dir(workspace_path().join("data/artifacts").join(&dir));
}

/// The pass the host's artifact preview stamps into its base. Direct to an
/// engine there is no gateway to prove anything to, so it mints none.
#[tokio::test]
async fn the_artifact_preview_pass_is_null_with_no_gateway_in_front() {
    let resp = http_client()
        .get(format!("{}/api/v1/artifact-preview-capability", base_url()))
        .send()
        .await
        .expect("request failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert!(body["capability"].is_null(), "{body}");
    assert!(body["renew_after_secs"].as_i64().unwrap_or(0) > 0, "{body}");
}
