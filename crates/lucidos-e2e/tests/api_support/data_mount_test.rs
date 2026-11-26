use crate::support::{base_url, http_client, unique_marker, workspace_path};

fn write_data_file(rel: &str, body: &str) -> std::path::PathBuf {
    let path = workspace_path().join("data").join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create parent dirs");
    }
    std::fs::write(&path, body).expect("Failed to write test file");
    path
}

#[tokio::test]
async fn the_data_mount_serves_artifacts_and_refuses_everything_else() {
    let client = http_client();
    let _tree = crate::support::workspace_tree_lock().read().await;

    let marker = unique_marker("mount");
    let allowed = write_data_file(&format!("artifacts/{marker}.txt"), "served");
    let refused = write_data_file(&format!("blobs/{marker}/secret.txt"), "not served");

    let resp = client
        .get(format!("{}/data/artifacts/{marker}.txt", base_url()))
        .send()
        .await
        .expect("allowed read failed");
    assert_eq!(resp.status(), 200, "an artifact must still be served");
    assert_eq!(resp.text().await.unwrap(), "served");

    let resp = client
        .get(format!("{}/data/blobs/{marker}/secret.txt", base_url()))
        .send()
        .await
        .expect("refused read failed");
    assert_eq!(
        resp.status(),
        404,
        "a file under a non-allowlisted prefix must not be served, even though \
         it is on disk"
    );

    let _ = std::fs::remove_file(&allowed);
    let _ = std::fs::remove_file(&refused);
    let _ = std::fs::remove_dir(refused.parent().unwrap());
}
