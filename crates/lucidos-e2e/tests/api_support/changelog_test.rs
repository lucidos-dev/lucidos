use crate::support::{base_url, http_client};

#[tokio::test]
async fn changelog_returns_every_release_newest_first() {
    let client = http_client();
    let url = format!("{}/api/v1/engine/changelog", base_url());

    let resp = client
        .get(&url)
        .send()
        .await
        .expect("Changelog request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON");
    let releases = body["releases"]
        .as_array()
        .expect("`releases` must be an array");
    assert!(
        releases.len() > 1,
        "the changelog carries many releases; got {}",
        releases.len()
    );

    let versions: Vec<&str> = releases
        .iter()
        .filter_map(|r| r["version"].as_str())
        .collect();
    let release = include_str!("../../../../RELEASE").trim_ascii_end();
    assert!(
        versions.contains(&release),
        "the release this engine reports running ({}) should have notes; got {versions:?}",
        release
    );

    for release in releases {
        assert!(
            release["version"].is_string(),
            "every release needs a version: {release}"
        );
        let notes = release["notes"]
            .as_str()
            .unwrap_or_else(|| panic!("every release needs notes: {release}"));
        assert!(!notes.trim().is_empty(), "empty notes for {release}");
        assert!(
            !notes.starts_with("## v"),
            "notes must exclude their own heading: {release}"
        );
    }
}

#[tokio::test]
async fn changelog_answers_the_same_twice() {
    let client = http_client();
    let url = format!("{}/api/v1/engine/changelog", base_url());

    let first: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("Changelog request failed")
        .json()
        .await
        .expect("Invalid JSON");
    let second: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("Changelog request failed")
        .json()
        .await
        .expect("Invalid JSON");

    assert_eq!(first, second);
}
