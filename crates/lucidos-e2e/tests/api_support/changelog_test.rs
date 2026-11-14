//! `GET /api/v1/engine/changelog`: the notes behind Settings > System > What's
//! New.

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

    // The panel marks "you are running this" by matching /health's `release`
    // against a version in this list, so the list has to CONTAIN it. Nothing
    // else holds the two together, and a drift would leave the panel silently
    // marking nothing.
    //
    // Contains, not equals-the-newest. The endpoint serves the newest changelog
    // it can reach, which on a branch cut before the last release is ahead of
    // this binary. That is the point of the panel, and it is the same property
    // `changelog::select_releases` trusts a fresher source on.
    let versions: Vec<&str> = releases
        .iter()
        .filter_map(|r| r["version"].as_str())
        .collect();
    assert!(
        versions.contains(&lucidos_engine::LUCIDOS_RELEASE),
        "the release this engine reports running ({}) should have notes; got {versions:?}",
        lucidos_engine::LUCIDOS_RELEASE
    );

    for release in releases {
        assert!(
            release["version"].is_string(),
            "every release needs a version: {release}"
        );
        // Raw markdown, per `.claude/rules/rust.md`: the frontend converts.
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

/// The endpoint needs no workspace and no database, and asking twice must not
/// answer twice differently. It does read the checkout, and it may fetch the
/// published changelog. So repeatability is a property of the cache in front of
/// that fetch, rather than of a constant. A second call that disagreed would
/// mean the cache is not holding, and the panel would flicker between two
/// histories.
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
