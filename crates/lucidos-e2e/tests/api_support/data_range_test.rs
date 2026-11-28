use crate::support::{base_url, http_client, unique_marker, workspace_path};

fn payload() -> Vec<u8> {
    (0..=255u8).collect()
}

fn artifact_path(rel: &str) -> std::path::PathBuf {
    workspace_path().join("data/artifacts").join(rel)
}

fn write_artifact(rel: &str, bytes: &[u8]) {
    let path = artifact_path(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create parent dirs");
    }
    std::fs::write(&path, bytes).expect("Failed to write test artifact");
}

fn data_url(rel: &str) -> String {
    format!("{}/api/v1/data/artifacts/{}", base_url(), rel)
}

fn header(response: &reqwest::Response, name: &str) -> Option<String> {
    Some(response.headers().get(name)?.to_str().unwrap().to_string())
}

#[tokio::test]
async fn data_route_serves_ranges_and_revalidates() {
    let client = http_client();
    let _tree = crate::support::workspace_tree_lock().read().await;

    let name = format!("{}.mp4", unique_marker("range"));
    let bytes = payload();
    write_artifact(&name, &bytes);
    let url = data_url(&name);

    let full = client
        .get(&url)
        .header("accept-encoding", "gzip, br")
        .send()
        .await
        .expect("full read failed");

    assert_eq!(full.status(), 200);
    assert_eq!(
        header(&full, "accept-ranges").as_deref(),
        Some("bytes"),
        "a compressed media response would have lost this header"
    );
    assert_eq!(
        header(&full, "content-encoding"),
        None,
        "video must not be compressed"
    );
    assert_eq!(header(&full, "content-type").as_deref(), Some("video/mp4"));
    assert_eq!(header(&full, "content-length").as_deref(), Some("256"));
    assert_eq!(header(&full, "cache-control").as_deref(), Some("no-cache"));
    let etag = header(&full, "etag").expect("etag");
    assert!(
        header(&full, "last-modified").is_some(),
        "no-cache needs something to revalidate against"
    );
    assert_eq!(full.bytes().await.unwrap().as_ref(), bytes.as_slice());

    let partial = client
        .get(&url)
        .header("range", "bytes=100-149")
        .send()
        .await
        .expect("range read failed");

    assert_eq!(partial.status(), 206);
    assert_eq!(
        header(&partial, "content-range").as_deref(),
        Some("bytes 100-149/256")
    );
    assert_eq!(header(&partial, "content-length").as_deref(), Some("50"));
    assert_eq!(
        partial.bytes().await.unwrap().as_ref(),
        &bytes[100..=149],
        "the slice must be the bytes the range named"
    );

    let tail = client
        .get(&url)
        .header("range", "bytes=250-")
        .send()
        .await
        .expect("open-ended range failed");

    assert_eq!(tail.status(), 206);
    assert_eq!(
        header(&tail, "content-range").as_deref(),
        Some("bytes 250-255/256")
    );
    assert_eq!(tail.bytes().await.unwrap().as_ref(), &bytes[250..]);

    let past = client
        .get(&url)
        .header("range", "bytes=900-1000")
        .send()
        .await
        .expect("unsatisfiable range failed");

    assert_eq!(past.status(), 416);
    assert_eq!(
        header(&past, "content-range").as_deref(),
        Some("bytes */256")
    );

    let unchanged = client
        .get(&url)
        .header("if-none-match", &etag)
        .send()
        .await
        .expect("conditional read failed");

    assert_eq!(unchanged.status(), 304);
    assert!(unchanged.bytes().await.unwrap().is_empty());

    let rebuilt: Vec<u8> = (0..=255u8).rev().chain(0..=127u8).collect();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    write_artifact(&name, &rebuilt);

    let after = client
        .get(&url)
        .header("if-none-match", &etag)
        .send()
        .await
        .expect("post-rebuild read failed");

    assert_eq!(after.status(), 200, "a rebuilt file must not answer 304");
    assert_ne!(header(&after, "etag").as_deref(), Some(etag.as_str()));
    assert_eq!(after.bytes().await.unwrap().as_ref(), rebuilt.as_slice());

    std::fs::remove_file(artifact_path(&name)).ok();
}

#[tokio::test]
async fn text_on_the_data_route_still_compresses() {
    let client = http_client();
    let _tree = crate::support::workspace_tree_lock().read().await;

    let name = format!("{}.md", unique_marker("compressible"));
    write_artifact(
        &name,
        &"# heading\n\nrepeated body text. ".repeat(200).into_bytes(),
    );

    let response = client
        .get(data_url(&name))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .expect("markdown read failed");

    assert_eq!(response.status(), 200);
    assert_eq!(
        header(&response, "content-encoding").as_deref(),
        Some("gzip")
    );

    std::fs::remove_file(artifact_path(&name)).ok();
}
